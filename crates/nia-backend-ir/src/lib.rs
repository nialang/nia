// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated, target-independent program representation consumed by codegen.
//!
//! Backend lowering publishes modules independently into [`BackendModuleStore`].
//! The immutable owner directory and single-consumer readiness stream let
//! parallel codegen discover exact definition owners without making completion
//! order observable in stable partition identities.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    ops::Index,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use nia_function_ir::{FunctionBody, FunctionInstanceKey};
use nia_ids::{
    ClosureId, GlobalConstExprId, GlobalDefId, InternedTyId, LocalId, ModuleId, ReceiverKind,
};
use nia_layout::{Layouts, StructLayout, StructLayoutKey, TypeLayout};
use nia_source::SourceIdentity;
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_symbol::SymbolId;
use nia_ty::{ConstGenericArg, TraitId};

const SOURCE_CODEGEN_BUCKETS: usize = 4;
const SOURCE_CODEGEN_SPLIT_THRESHOLD: usize = 8;

#[derive(Debug, PartialEq)]
/// Complete backend program backed by a fully published module store.
pub struct BackendProgram {
    /// Modules in their stable source order.
    pub modules: BackendModules,
}

impl BackendProgram {
    /// Builds and synchronously publishes a program from ordered modules.
    pub fn new(modules: Vec<BackendModule>) -> Self {
        let module_ids = modules.iter().map(|module| module.id).collect::<Vec<_>>();
        let store = Arc::new(BackendModuleStore::new(module_ids));
        for module in modules {
            store.publish(module);
        }
        Self::from_module_store(store)
    }

    /// Wraps a module store after asserting every planned owner was published.
    pub fn from_module_store(store: Arc<BackendModuleStore>) -> Self {
        assert!(
            store.is_complete(),
            "Nia ICE: backend program requires a complete module store"
        );
        Self {
            modules: BackendModules { store },
        }
    }

    /// Returns shared access to the immutable published module store.
    pub fn module_store(&self) -> Arc<BackendModuleStore> {
        Arc::clone(&self.modules.store)
    }

    /// Derives the deterministic codegen partition plan.
    pub fn codegen_partition_plan(&self) -> CodegenPartitionPlan {
        CodegenPartitionPlan::from_modules(&self.modules)
    }

    /// Resolves and validates the source module owning `partition`.
    pub fn module_for_partition(&self, partition: &CodegenPartition) -> &BackendModule {
        let module_id = match partition.id {
            CodegenUnitId::SourceModule { module_id, .. } => module_id,
            CodegenUnitId::CompilerBuiltins => {
                panic!("Nia ICE: compiler builtins partition has no backend module")
            }
        };
        let module = self.modules.store.get(module_id).unwrap_or_else(|| {
            panic!(
                "Nia ICE: codegen partition {:?} references missing backend module {module_id:?}",
                partition.id
            )
        });
        assert_eq!(
            partition.id,
            CodegenUnitId::source_module(module.id, partition.ordinal())
        );
        assert_eq!(
            partition.key,
            CodegenUnitKey::source_module(module.source_identity.clone(), partition.ordinal()),
            "Nia ICE: codegen partition stable key does not match its backend module"
        );
        module
    }
}

#[derive(Clone)]
/// Stable source-ordered view over a shared backend module store.
pub struct BackendModules {
    store: Arc<BackendModuleStore>,
}

impl BackendModules {
    /// Returns the number of planned module positions.
    pub fn len(&self) -> usize {
        self.store.module_ids.len()
    }

    /// Reports whether the program has no source modules.
    pub fn is_empty(&self) -> bool {
        self.store.module_ids.is_empty()
    }

    /// Returns the published module at `position`.
    pub fn get(&self, position: usize) -> Option<&BackendModule> {
        self.store.get_at(position)
    }

    /// Iterates modules in stable source order.
    pub fn iter(&self) -> BackendModulesIter<'_> {
        BackendModulesIter {
            modules: self,
            position: 0,
        }
    }
}

impl From<Vec<BackendModule>> for BackendModules {
    fn from(modules: Vec<BackendModule>) -> Self {
        BackendProgram::new(modules).modules
    }
}

impl fmt::Debug for BackendModules {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for BackendModules {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl Index<usize> for BackendModules {
    type Output = BackendModule;

    fn index(&self, position: usize) -> &Self::Output {
        self.get(position)
            .unwrap_or_else(|| panic!("Nia ICE: backend module position {position} is unavailable"))
    }
}

impl<'a> IntoIterator for &'a BackendModules {
    type Item = &'a BackendModule;
    type IntoIter = BackendModulesIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Source-ordered iterator over published backend modules.
pub struct BackendModulesIter<'a> {
    modules: &'a BackendModules,
    position: usize,
}

impl<'a> Iterator for BackendModulesIter<'a> {
    type Item = &'a BackendModule;

    fn next(&mut self) -> Option<Self::Item> {
        let module = self.modules.get(self.position)?;
        self.position += 1;
        Some(module)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.modules.len().saturating_sub(self.position);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for BackendModulesIter<'_> {}

#[derive(Debug)]
/// Write-once concurrent publication store for planned backend modules.
///
/// Each registered owner has one [`OnceLock`] slot. Publication records a
/// completion under the readiness mutex only after the slot becomes visible;
/// the unique readiness consumer can therefore immediately read every event.
pub struct BackendModuleStore {
    module_ids: Vec<ModuleId>,
    positions: HashMap<ModuleId, usize>,
    slots: Vec<OnceLock<BackendModule>>,
    readiness: Mutex<BackendModuleReadinessState>,
    readiness_changed: Condvar,
    readiness_claimed: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// One published module notification from the readiness stream.
pub struct BackendModuleReady {
    position: usize,
    module_id: ModuleId,
}

impl BackendModuleReady {
    /// Returns the module's stable source position.
    pub fn position(self) -> usize {
        self.position
    }

    /// Returns the published module owner.
    pub fn module_id(self) -> ModuleId {
        self.module_id
    }
}

#[derive(Debug)]
struct BackendModuleReadinessState {
    completions: Vec<BackendModuleReady>,
}

#[derive(Debug)]
/// Unique blocking consumer of module publication events.
pub struct BackendModuleReadiness {
    store: Arc<BackendModuleStore>,
    next: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
/// Immutable mapping from every backend definition identity to its module.
///
/// It is planned before parallel finalization, so codegen dependency discovery
/// never depends on which module happens to publish first.
pub struct BackendModuleOwnerDirectory {
    items: HashMap<GlobalDefId, ModuleId>,
    struct_instances: HashMap<BackendStructInstanceKey, ModuleId>,
    union_instances: HashMap<BackendStructInstanceKey, ModuleId>,
    global_instances: HashMap<BackendGlobalInstanceKey, ModuleId>,
    function_instances: HashMap<FunctionInstanceKey, ModuleId>,
    vtables: HashMap<BackendTraitObjectVtableKey, ModuleId>,
    vtables_by_object_ty: HashMap<InternedTyId, Vec<BackendTraitObjectVtableKey>>,
    vtables_by_trait: HashMap<TraitId, Vec<BackendTraitObjectVtableKey>>,
}

impl BackendModuleOwnerDirectory {
    /// Builds the directory and rejects duplicate definition owners.
    pub fn from_modules<'a>(modules: impl IntoIterator<Item = &'a BackendModule>) -> Self {
        let mut directory = Self::default();
        for module in modules {
            for def_id in module
                .structs
                .iter()
                .map(|item| item.def_id)
                .chain(module.unions.iter().map(|item| item.def_id))
                .chain(module.enums.iter().map(|item| item.def_id))
                .chain(module.globals.iter().map(|item| item.def_id))
                .chain(module.functions.iter().map(|item| item.def_id))
            {
                assert!(
                    directory.items.insert(def_id, module.id).is_none(),
                    "Nia ICE: backend item {def_id:?} has multiple module owners"
                );
            }
            for item in &module.struct_instances {
                let key = BackendStructInstanceKey {
                    def_id: item.def_id,
                    args: item.args.clone(),
                    const_args: item.const_args.clone(),
                };
                assert!(
                    directory.struct_instances.insert(key, module.id).is_none(),
                    "Nia ICE: backend struct instance has multiple module owners"
                );
            }
            for item in &module.union_instances {
                let key = BackendStructInstanceKey {
                    def_id: item.def_id,
                    args: item.args.clone(),
                    const_args: item.const_args.clone(),
                };
                assert!(
                    directory.union_instances.insert(key, module.id).is_none(),
                    "Nia ICE: backend union instance has multiple module owners"
                );
            }
            for item in &module.global_instances {
                let key = BackendGlobalInstanceKey {
                    def_id: item.def_id,
                    arg_module_id: item.arg_module_id,
                    args: item.args.clone(),
                    const_args: item.const_args.clone(),
                };
                assert!(
                    directory.global_instances.insert(key, module.id).is_none(),
                    "Nia ICE: backend global instance has multiple module owners"
                );
            }
            for item in &module.function_instances {
                let key = FunctionInstanceKey {
                    def_id: item.def_id,
                    arg_module_id: item.arg_module_id,
                    self_arg: item.self_arg,
                    args: item.args.clone(),
                    const_args: item.const_args.clone(),
                };
                assert!(
                    directory
                        .function_instances
                        .insert(key, module.id)
                        .is_none(),
                    "Nia ICE: backend function instance has multiple module owners"
                );
            }
            for item in &module.trait_object_vtables {
                assert!(
                    directory
                        .vtables
                        .insert(item.key.clone(), module.id)
                        .is_none(),
                    "Nia ICE: backend vtable has multiple module owners"
                );
                directory
                    .vtables_by_object_ty
                    .entry(item.key.object_ty)
                    .or_default()
                    .push(item.key.clone());
                let mut traits = item
                    .entries
                    .iter()
                    .map(|entry| entry.trait_id)
                    .chain(std::iter::once(item.trait_id))
                    .collect::<HashSet<_>>();
                for trait_id in traits.drain() {
                    directory
                        .vtables_by_trait
                        .entry(trait_id)
                        .or_default()
                        .push(item.key.clone());
                }
            }
        }
        for keys in directory.vtables_by_object_ty.values_mut() {
            keys.sort_by_key(|key| (key.self_ty, key.object_ty));
        }
        for keys in directory.vtables_by_trait.values_mut() {
            keys.sort_by_key(|key| (key.self_ty, key.object_ty));
        }
        directory
    }

    /// Returns the module owning a non-instantiated item definition.
    pub fn item_owner(&self, def_id: GlobalDefId) -> Option<ModuleId> {
        self.items.get(&def_id).copied()
    }

    /// Confirms that every finalized definition was present in the plan.
    pub fn validate_finalized_module(&self, module: &BackendModule) {
        let definitions = Self::from_modules([module]);
        for (def_id, owner) in definitions.items {
            assert_eq!(
                self.items.get(&def_id),
                Some(&owner),
                "Nia ICE: finalized backend item {def_id:?} was absent from its definition manifest"
            );
        }
        for (key, owner) in definitions.struct_instances {
            assert_eq!(
                self.struct_instances.get(&key),
                Some(&owner),
                "Nia ICE: finalized backend struct instance was absent from its definition manifest"
            );
        }
        for (key, owner) in definitions.union_instances {
            assert_eq!(
                self.union_instances.get(&key),
                Some(&owner),
                "Nia ICE: finalized backend union instance was absent from its definition manifest"
            );
        }
        for (key, owner) in definitions.global_instances {
            assert_eq!(
                self.global_instances.get(&key),
                Some(&owner),
                "Nia ICE: finalized backend global instance was absent from its definition manifest"
            );
        }
        for (key, owner) in definitions.function_instances {
            assert_eq!(
                self.function_instances.get(&key),
                Some(&owner),
                "Nia ICE: finalized backend function instance was absent from its definition manifest"
            );
        }
        for (key, owner) in definitions.vtables {
            assert_eq!(
                self.vtables.get(&key),
                Some(&owner),
                "Nia ICE: finalized backend vtable was absent from its definition manifest"
            );
        }
    }

    /// Returns the owner of an exact struct instance.
    pub fn struct_instance_owner(&self, key: &BackendStructInstanceKey) -> Option<ModuleId> {
        self.struct_instances.get(key).copied()
    }

    /// Returns the owner of an exact union instance.
    pub fn union_instance_owner(&self, key: &BackendStructInstanceKey) -> Option<ModuleId> {
        self.union_instances.get(key).copied()
    }

    /// Returns the owner of an exact global instance.
    pub fn global_instance_owner(&self, key: &BackendGlobalInstanceKey) -> Option<ModuleId> {
        self.global_instances.get(key).copied()
    }

    /// Returns the owner of an exact function instance.
    pub fn function_instance_owner(&self, key: &FunctionInstanceKey) -> Option<ModuleId> {
        self.function_instances.get(key).copied()
    }

    /// Returns the owner of an exact trait-object vtable.
    pub fn vtable_owner(&self, key: &BackendTraitObjectVtableKey) -> Option<ModuleId> {
        self.vtables.get(key).copied()
    }

    /// Iterates all planned vtable identities for an erased object type.
    ///
    /// The directory is built before independently finalized modules are
    /// published, so this lookup is stable across readiness completion order.
    pub fn vtable_keys_for_object_ty(
        &self,
        object_ty: InternedTyId,
    ) -> impl Iterator<Item = &BackendTraitObjectVtableKey> {
        self.vtables_by_object_ty
            .get(&object_ty)
            .into_iter()
            .flatten()
    }

    /// Iterates planned vtables containing slots for a trait.
    ///
    /// This index includes supertrait segments, allowing readiness to find the
    /// source vtable retained by an explicitly upcast object view.
    pub fn vtable_keys_for_trait(
        &self,
        trait_id: TraitId,
    ) -> impl Iterator<Item = &BackendTraitObjectVtableKey> {
        self.vtables_by_trait.get(&trait_id).into_iter().flatten()
    }
}

impl BackendModuleStore {
    /// Registers the unique module owners that may later be published.
    pub fn new(module_ids: impl IntoIterator<Item = ModuleId>) -> Self {
        let module_ids = module_ids.into_iter().collect::<Vec<_>>();
        let mut positions = HashMap::with_capacity(module_ids.len());
        for (position, module_id) in module_ids.iter().copied().enumerate() {
            assert!(
                positions.insert(module_id, position).is_none(),
                "Nia ICE: backend module store contains duplicate module owner {module_id:?}"
            );
        }
        Self {
            slots: (0..module_ids.len()).map(|_| OnceLock::new()).collect(),
            module_ids,
            positions,
            readiness: Mutex::new(BackendModuleReadinessState {
                completions: Vec::new(),
            }),
            readiness_changed: Condvar::new(),
            readiness_claimed: AtomicBool::new(false),
        }
    }

    /// Returns registered owners in stable source order.
    pub fn module_ids(&self) -> &[ModuleId] {
        &self.module_ids
    }

    /// Publishes one registered owner exactly once and signals readiness.
    pub fn publish(&self, module: BackendModule) -> &BackendModule {
        let module_id = module.id;
        let position = *self.positions.get(&module_id).unwrap_or_else(|| {
            panic!("Nia ICE: backend module store rejected unregistered owner {module_id:?}")
        });
        assert!(
            self.slots[position].set(module).is_ok(),
            "Nia ICE: backend module store owner {module_id:?} was published twice"
        );
        let published = self.slots[position]
            .get()
            .expect("published backend module slot");
        let mut readiness = self
            .readiness
            .lock()
            .expect("backend module readiness lock poisoned");
        readiness.completions.push(BackendModuleReady {
            position,
            module_id,
        });
        self.readiness_changed.notify_one();
        published
    }

    /// Returns a module only after its slot has been published.
    pub fn get(&self, module_id: ModuleId) -> Option<&BackendModule> {
        self.positions
            .get(&module_id)
            .and_then(|position| self.slots[*position].get())
    }

    fn get_at(&self, position: usize) -> Option<&BackendModule> {
        self.slots.get(position).and_then(OnceLock::get)
    }

    /// Reports whether every registered owner has been published.
    pub fn is_complete(&self) -> bool {
        self.slots.iter().all(|slot| slot.get().is_some())
    }

    /// Claims the store's single readiness consumer.
    pub fn take_readiness(self: &Arc<Self>) -> BackendModuleReadiness {
        assert!(
            self.readiness_claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "Nia ICE: backend module readiness already has a consumer"
        );
        BackendModuleReadiness {
            store: Arc::clone(self),
            next: 0,
        }
    }
}

impl BackendModuleReadiness {
    /// Blocks until the next publication, or returns `None` after completion.
    pub fn wait_next(&mut self) -> Option<BackendModuleReady> {
        let mut readiness = self
            .store
            .readiness
            .lock()
            .expect("backend module readiness lock poisoned");
        loop {
            if let Some(completion) = readiness.completions.get(self.next).copied() {
                self.next += 1;
                return Some(completion);
            }
            if readiness.completions.len() == self.store.module_ids.len() {
                return None;
            }
            readiness = self
                .store
                .readiness_changed
                .wait(readiness)
                .expect("backend module readiness lock poisoned");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CodegenUnitId {
    SourceModule { module_id: ModuleId, ordinal: u32 },
    CompilerBuiltins,
}

impl CodegenUnitId {
    fn source_module(module_id: ModuleId, ordinal: u32) -> Self {
        Self::SourceModule { module_id, ordinal }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CodegenUnitKey {
    SourceModule {
        source_identity: SourceIdentity,
        ordinal: u32,
    },
    CompilerBuiltins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CodegenUnitFingerprint([u64; 2]);

impl CodegenUnitFingerprint {
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

impl CodegenUnitKey {
    fn source_module(source_identity: SourceIdentity, ordinal: u32) -> Self {
        Self::SourceModule {
            source_identity,
            ordinal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenUnitDependencies {
    unit: CodegenUnitId,
    modules: Vec<ModuleId>,
}

impl CodegenUnitDependencies {
    pub fn new(unit: CodegenUnitId, modules: impl IntoIterator<Item = ModuleId>) -> Self {
        let modules = modules.into_iter().collect::<BTreeSet<_>>();
        assert!(
            !modules.is_empty(),
            "Nia ICE: codegen unit dependency modules must include its owner"
        );
        Self {
            unit,
            modules: modules.into_iter().collect(),
        }
    }

    pub fn unit(&self) -> CodegenUnitId {
        self.unit
    }

    pub fn modules(&self) -> &[ModuleId] {
        &self.modules
    }

    pub fn contains(&self, module_id: ModuleId) -> bool {
        self.modules.binary_search(&module_id).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenUnitPendingModules {
    unit: CodegenUnitId,
    modules: Vec<ModuleId>,
}

impl CodegenUnitPendingModules {
    pub fn new(unit: CodegenUnitId, modules: impl IntoIterator<Item = ModuleId>) -> Self {
        let modules = modules.into_iter().collect::<BTreeSet<_>>();
        assert!(
            !modules.is_empty(),
            "Nia ICE: pending codegen unit must wait for at least one module"
        );
        Self {
            unit,
            modules: modules.into_iter().collect(),
        }
    }

    pub fn unit(&self) -> CodegenUnitId {
        self.unit
    }

    pub fn modules(&self) -> &[ModuleId] {
        &self.modules
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalLinkInput<T> {
    pub key: CodegenUnitKey,
    pub fingerprint: CodegenUnitFingerprint,
    pub object: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalLinkInputs<T> {
    inputs: Vec<IncrementalLinkInput<T>>,
}

impl<T> IncrementalLinkInputs<T> {
    pub fn new(inputs: Vec<IncrementalLinkInput<T>>) -> Self {
        for pair in inputs.windows(2) {
            assert!(
                pair[0].key < pair[1].key,
                "Nia ICE: incremental link inputs must have unique stable keys in ascending order"
            );
        }
        Self { inputs }
    }

    pub fn as_slice(&self) -> &[IncrementalLinkInput<T>] {
        &self.inputs
    }

    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    pub fn into_vec(self) -> Vec<IncrementalLinkInput<T>> {
        self.inputs
    }
}

impl<T> Default for IncrementalLinkInputs<T> {
    fn default() -> Self {
        Self { inputs: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenPartitionPlan {
    partitions: Vec<CodegenPartition>,
}

impl CodegenPartitionPlan {
    fn from_modules(modules: &BackendModules) -> Self {
        Self::from_module_iter(modules)
    }

    pub fn for_ready_module(module: &BackendModule) -> Self {
        Self::from_module_iter([module])
    }

    fn from_module_iter<'a>(modules: impl IntoIterator<Item = &'a BackendModule>) -> Self {
        let modules = modules.into_iter().collect::<Vec<_>>();
        let mut vtable_definitions = HashSet::new();
        for module in &modules {
            for vtable in &module.trait_object_vtables {
                assert!(
                    vtable_definitions.insert(vtable.key.clone()),
                    "Nia ICE: backend program contains duplicate trait-object vtable definition {:?}",
                    vtable.key
                );
            }
        }
        let mut partitions = modules
            .into_iter()
            .flat_map(|module| {
                CodegenPartitionDefinitions::for_module(module)
                    .into_iter()
                    .map(move |(ordinal, definitions)| CodegenPartition {
                        id: CodegenUnitId::source_module(module.id, ordinal),
                        key: CodegenUnitKey::source_module(module.source_identity.clone(), ordinal),
                        definitions,
                    })
            })
            .collect::<Vec<_>>();
        partitions.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        for pair in partitions.windows(2) {
            assert_ne!(
                pair[0].key, pair[1].key,
                "Nia ICE: backend program contains duplicate stable codegen partition key"
            );
        }
        Self { partitions }
    }

    pub fn partitions(&self) -> &[CodegenPartition] {
        &self.partitions
    }

    pub fn validate_program(&self, program: &BackendProgram) {
        let modules = &program.modules;
        let expected = Self::from_modules(modules);
        assert_eq!(
            self, &expected,
            "Nia ICE: codegen partition plan does not match the backend program"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenPartition {
    pub id: CodegenUnitId,
    pub key: CodegenUnitKey,
    definitions: CodegenPartitionDefinitions,
}

impl CodegenPartition {
    fn ordinal(&self) -> u32 {
        match (self.id, &self.key) {
            (
                CodegenUnitId::SourceModule { ordinal, .. },
                CodegenUnitKey::SourceModule {
                    ordinal: key_ordinal,
                    ..
                },
            ) if ordinal == *key_ordinal => ordinal,
            _ => panic!("Nia ICE: source codegen partition has inconsistent identities"),
        }
    }

    pub fn global_definitions(&self) -> &[usize] {
        &self.definitions.globals
    }

    pub fn global_instance_definitions(&self) -> &[usize] {
        &self.definitions.global_instances
    }

    pub fn function_definitions(&self) -> &[usize] {
        &self.definitions.functions
    }

    pub fn function_instance_definitions(&self) -> &[usize] {
        &self.definitions.function_instances
    }

    pub fn closure_entry_definitions(&self) -> &[usize] {
        &self.definitions.closure_entries
    }

    pub fn vtable_definitions(&self) -> &[usize] {
        &self.definitions.vtables
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CodegenPartitionDefinitions {
    globals: Vec<usize>,
    global_instances: Vec<usize>,
    functions: Vec<usize>,
    function_instances: Vec<usize>,
    closure_entries: Vec<usize>,
    vtables: Vec<usize>,
}

impl CodegenPartitionDefinitions {
    fn from_module(module: &BackendModule) -> Self {
        let mut definitions = Self {
            globals: module
                .globals
                .iter()
                .enumerate()
                .filter_map(|(index, global)| (!global.is_extern).then_some(index))
                .collect(),
            global_instances: (0..module.global_instances.len()).collect(),
            functions: module
                .functions
                .iter()
                .enumerate()
                .filter_map(|(index, function)| {
                    (function.generics.is_empty() && function.function_body.is_some())
                        .then_some(index)
                })
                .collect(),
            function_instances: module
                .function_instances
                .iter()
                .enumerate()
                .filter_map(|(index, function)| function.function_body.as_ref().map(|_| index))
                .collect(),
            closure_entries: (0..module.closure_entries.len()).collect(),
            vtables: (0..module.trait_object_vtables.len()).collect(),
        };
        definitions
            .globals
            .sort_unstable_by_key(|index| module.globals[*index].def_id.def_id);
        definitions
            .global_instances
            .sort_unstable_by(|left, right| {
                module.global_instances[*left]
                    .symbol
                    .cmp(&module.global_instances[*right].symbol)
            });
        definitions
            .functions
            .sort_unstable_by_key(|index| module.functions[*index].def_id.def_id);
        definitions
            .function_instances
            .sort_unstable_by(|left, right| {
                module.function_instances[*left]
                    .symbol
                    .cmp(&module.function_instances[*right].symbol)
            });
        definitions.closure_entries.sort_unstable_by(|left, right| {
            module.closure_entries[*left]
                .symbol
                .cmp(&module.closure_entries[*right].symbol)
        });
        definitions
    }

    fn is_empty(&self) -> bool {
        self.globals.is_empty()
            && self.global_instances.is_empty()
            && self.functions.is_empty()
            && self.function_instances.is_empty()
            && self.closure_entries.is_empty()
            && self.vtables.is_empty()
    }

    fn len(&self) -> usize {
        self.globals.len()
            + self.global_instances.len()
            + self.functions.len()
            + self.function_instances.len()
            + self.closure_entries.len()
            + self.vtables.len()
    }

    fn for_module(module: &BackendModule) -> Vec<(u32, Self)> {
        let definitions = Self::from_module(module);
        if definitions.is_empty() {
            return Vec::new();
        }
        if definitions.len() < SOURCE_CODEGEN_SPLIT_THRESHOLD {
            return vec![(0, definitions)];
        }

        let mut buckets = (0..SOURCE_CODEGEN_BUCKETS)
            .map(|_| Self::default())
            .collect::<Vec<_>>();
        for index in definitions.globals {
            let bucket = module.globals[index].def_id.def_id.0 as usize % SOURCE_CODEGEN_BUCKETS;
            buckets[bucket].globals.push(index);
        }
        for index in definitions.global_instances {
            let bucket = stable_symbol_bucket(&module.global_instances[index].symbol);
            buckets[bucket].global_instances.push(index);
        }
        for index in definitions.functions {
            let bucket = module.functions[index].def_id.def_id.0 as usize % SOURCE_CODEGEN_BUCKETS;
            buckets[bucket].functions.push(index);
        }
        for index in definitions.function_instances {
            let bucket = stable_symbol_bucket(&module.function_instances[index].symbol);
            buckets[bucket].function_instances.push(index);
        }
        for index in definitions.closure_entries {
            let entry = &module.closure_entries[index];
            let bucket = match &entry.key.owner {
                BackendClosureEntryOwner::Source(def_id) => {
                    def_id.def_id.0 as usize % SOURCE_CODEGEN_BUCKETS
                }
                BackendClosureEntryOwner::FunctionInstance(owner) => {
                    let instance = module
                        .function_instances
                        .iter()
                        .find(|instance| {
                            instance.def_id == owner.def_id
                                && instance.arg_module_id == owner.arg_module_id
                                && instance.self_arg == owner.self_arg
                                && instance.args == owner.args
                                && instance.const_args == owner.const_args
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "Nia ICE: closure entry {:?} has no materialized owner instance",
                                entry.key
                            )
                        });
                    stable_symbol_bucket(&instance.symbol)
                }
            };
            buckets[bucket].closure_entries.push(index);
        }
        buckets[0].vtables = definitions.vtables;

        buckets
            .into_iter()
            .enumerate()
            .filter_map(|(ordinal, definitions)| {
                (!definitions.is_empty()).then_some((ordinal as u32, definitions))
            })
            .collect()
    }
}

fn stable_symbol_bucket(symbol: &str) -> usize {
    nia_symbol::stable_hash(symbol) as usize % SOURCE_CODEGEN_BUCKETS
}

#[derive(Debug, PartialEq)]
pub struct BackendModule {
    pub id: ModuleId,
    pub source_identity: SourceIdentity,
    pub name: String,
    pub const_eval: BackendConstFacts,
    pub layouts: BackendLayouts,
    pub structs: Vec<BackendStruct>,
    pub unions: Vec<BackendUnion>,
    pub struct_instances: Vec<BackendStructInstance>,
    pub union_instances: Vec<BackendUnionInstance>,
    pub enums: Vec<BackendEnum>,
    pub globals: Vec<BackendGlobal>,
    pub global_instances: Vec<BackendGlobalInstance>,
    pub functions: Vec<BackendFunction>,
    pub function_instances: Vec<BackendFunctionInstance>,
    pub closure_entries: Vec<BackendClosureEntry>,
    pub trait_object_vtables: Vec<BackendTraitObjectVtable>,
    pub generic_instantiations: Vec<BackendGenericInstantiation>,
}

/// The concrete function whose substitutions determine a generated closure
/// entry. The closure itself retains its source `ClosureId`; this owner key
/// distinguishes entries materialized for separate generic instances.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendClosureEntryOwner {
    Source(GlobalDefId),
    FunctionInstance(FunctionInstanceKey),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendClosureEntryKey {
    pub closure_id: ClosureId,
    pub owner: BackendClosureEntryOwner,
}

/// Backend-visible ABI of a generated closure entry.
///
/// `state_pointer_type` is the hidden first parameter. User parameters follow
/// in source order and the entry is never variadic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendClosureEntryAbi {
    pub state_type: InternedTyId,
    pub state_pointer_type: InternedTyId,
    pub params: Vec<InternedTyId>,
    pub return_type: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendClosureEntry {
    pub key: BackendClosureEntryKey,
    pub symbol: String,
    pub abi: BackendClosureEntryAbi,
    pub state_param: LocalId,
    pub params: Vec<LocalId>,
    pub local_names: HashMap<LocalId, String>,
    pub function_body: FunctionBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackendConstFacts {
    pub array_lengths: HashMap<GlobalConstExprId, u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLayouts {
    pub target: nia_layout::TargetDataLayout,
    pub types: Vec<(InternedTyId, TypeLayout)>,
    pub structs: Vec<(GlobalDefId, StructLayout)>,
    pub unions: Vec<(GlobalDefId, StructLayout)>,
    pub enums: Vec<(GlobalDefId, nia_layout::EnumLayout)>,
    pub struct_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
    pub union_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendStructInstanceKey {
    pub def_id: GlobalDefId,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
}

impl BackendLayouts {
    pub fn from_module_layouts(module_id: ModuleId, layouts: &Layouts) -> Self {
        Self {
            target: layouts.target,
            types: layouts
                .types
                .iter()
                .map(|(ty, layout)| (*ty, layout.clone()))
                .collect(),
            structs: layouts
                .structs
                .iter()
                .map(|(def_id, layout)| {
                    (
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                        layout.clone(),
                    )
                })
                .collect(),
            unions: layouts
                .unions
                .iter()
                .map(|(def_id, layout)| {
                    (
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                        layout.clone(),
                    )
                })
                .collect(),
            enums: layouts
                .enums
                .iter()
                .map(|(def_id, layout)| {
                    (
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                        layout.clone(),
                    )
                })
                .collect(),
            struct_instances: layouts
                .struct_instances
                .iter()
                .map(|(key, layout)| {
                    (
                        BackendStructInstanceKey::from_module_key(module_id, key),
                        layout.clone(),
                    )
                })
                .collect(),
            union_instances: layouts
                .union_instances
                .iter()
                .map(|(key, layout)| {
                    (
                        BackendStructInstanceKey::from_module_key(module_id, key),
                        layout.clone(),
                    )
                })
                .collect(),
        }
    }
}

impl BackendStructInstanceKey {
    pub fn from_module_key(module_id: ModuleId, key: &StructLayoutKey) -> Self {
        Self {
            def_id: GlobalDefId {
                module_id,
                def_id: key.def_id,
            },
            args: key.args.clone(),
            const_args: key.const_args.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendStruct {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub generics: Vec<SymbolId>,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnion {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub generics: Vec<SymbolId>,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendStructInstance {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
    pub symbol: String,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnionInstance {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
    pub symbol: String,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendField {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnum {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub backing_type: InternedTyId,
    pub variants: Vec<BackendEnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnumVariant {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub value: Option<i128>,
    pub payload: BackendEnumVariantPayload,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackendEnumVariantPayload {
    Unit,
    Tuple(Vec<InternedTyId>),
    Named(Vec<BackendField>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendGlobal {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub link_name: Option<String>,
    pub ty: InternedTyId,
    pub is_let: bool,
    pub is_extern: bool,
    pub init: Option<StaticInit>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendGlobalInstanceKey {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendGlobalInstance {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
    pub symbol: String,
    pub ty: InternedTyId,
    pub is_let: bool,
    pub init: Option<StaticInit>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunction {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub link_name: Option<String>,
    pub generics: Vec<SymbolId>,
    pub params: Vec<BackendParam>,
    pub return_type: InternedTyId,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub attributes: Vec<BackendFunctionAttribute>,
    pub local_names: HashMap<LocalId, String>,
    pub function_body: Option<FunctionBody>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendFunctionAttribute {
    Naked,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunctionInstance {
    pub def_id: GlobalDefId,
    pub name: SymbolId,
    pub arg_module_id: ModuleId,
    pub self_arg: Option<InternedTyId>,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
    pub symbol: String,
    pub params: Vec<BackendParam>,
    pub return_type: InternedTyId,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub attributes: Vec<BackendFunctionAttribute>,
    pub local_names: HashMap<LocalId, String>,
    pub function_body: Option<FunctionBody>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Program-wide identity of a concrete trait-object vtable.
///
/// `object_ty` contains the complete trait instance, including type, const, and
/// associated-type arguments. Keeping it in the key makes distinct concrete
/// trait objects distinct even when they share the same erased receiver type.
pub struct BackendTraitObjectVtableKey {
    pub self_ty: InternedTyId,
    pub object_ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
/// Concrete dispatch metadata for one `(self type, trait-object type)` pair.
///
/// The explicit trait arguments mirror `key.object_ty`. They let validators,
/// fingerprints, and dependency discovery consume the instantiated trait
/// contract without decoding the type interner again.
pub struct BackendTraitObjectVtable {
    pub key: BackendTraitObjectVtableKey,
    pub trait_id: TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<ConstGenericArg>,
    pub entries: Vec<BackendTraitObjectVtableEntry>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// One method slot together with the exact trait segment that owns it.
///
/// A vtable may contain multiple instantiations of the same supertrait, so the
/// trait id alone is not sufficient to identify a slot.
pub struct BackendTraitObjectVtableEntry {
    pub trait_id: TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<ConstGenericArg>,
    pub method_id: GlobalDefId,
    pub method_name: SymbolId,
    pub slot: usize,
    pub function: BackendTraitObjectVtableFunction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendTraitObjectVtableFunction {
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendGenericInstantiation {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub self_arg: Option<InternedTyId>,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
    pub span: Span,
    pub source_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendParam {
    pub local_id: Option<LocalId>,
    pub name: Option<SymbolId>,
    pub receiver: Option<ReceiverKind>,
    pub passing_ty: InternedTyId,
    pub local_ty: InternedTyId,
    pub span: Span,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
