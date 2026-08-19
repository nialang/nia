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
/// Identity of a code-generation unit during one compiler invocation.
///
/// Source units retain the owning module and a deterministic bucket ordinal;
/// [`CompilerBuiltins`](Self::CompilerBuiltins) is reserved for runtime support
/// emitted outside any source module.
pub enum CodegenUnitId {
    /// Partition belonging to a source module.
    SourceModule {
        /// Transient owner module id.
        module_id: ModuleId,
        /// Deterministic partition bucket ordinal.
        ordinal: u32,
    },
    /// Runtime/compiler support unit outside source modules.
    CompilerBuiltins,
}

impl CodegenUnitId {
    fn source_module(module_id: ModuleId, ordinal: u32) -> Self {
        Self::SourceModule { module_id, ordinal }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Persistable identity used to sort and match code-generation units.
///
/// A source key deliberately uses [`SourceIdentity`] instead of the transient
/// [`ModuleId`], so clean and incremental builds agree even when module slots
/// are allocated in a different order.
pub enum CodegenUnitKey {
    /// Stable source partition identity.
    SourceModule {
        /// Stable source identity of the owning module.
        source_identity: SourceIdentity,
        /// Deterministic partition bucket ordinal.
        ordinal: u32,
    },
    /// Stable identity for compiler-provided support code.
    CompilerBuiltins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Stable two-word fingerprint associated with one code-generation unit.
pub struct CodegenUnitFingerprint([u64; 2]);

impl CodegenUnitFingerprint {
    /// Creates a fingerprint from the already-canonical hash parts.
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    /// Returns the hash parts for persistence or comparison.
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
/// Canonical set of modules needed before a code-generation unit can lower.
///
/// Construction sorts and deduplicates module ids, making dependency
/// comparisons independent of discovery order. The caller must include the
/// unit's owner; an empty set would make readiness accounting unsound.
pub struct CodegenUnitDependencies {
    unit: CodegenUnitId,
    modules: Vec<ModuleId>,
}

impl CodegenUnitDependencies {
    /// Creates a canonical, non-empty dependency set.
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

    /// Returns the unit whose dependencies are described.
    pub fn unit(&self) -> CodegenUnitId {
        self.unit
    }

    /// Returns module ids in ascending order without duplicates.
    pub fn modules(&self) -> &[ModuleId] {
        &self.modules
    }

    /// Tests membership using the canonical ordering.
    pub fn contains(&self, module_id: ModuleId) -> bool {
        self.modules.binary_search(&module_id).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Canonical set of modules still missing before a unit may be emitted.
pub struct CodegenUnitPendingModules {
    unit: CodegenUnitId,
    modules: Vec<ModuleId>,
}

impl CodegenUnitPendingModules {
    /// Creates a non-empty pending set after sorting and deduplication.
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

    /// Returns the blocked code-generation unit.
    pub fn unit(&self) -> CodegenUnitId {
        self.unit
    }

    /// Returns the missing module ids in ascending order.
    pub fn modules(&self) -> &[ModuleId] {
        &self.modules
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One stable code-generation unit and its reusable object/fingerprint pair.
pub struct IncrementalLinkInput<T> {
    /// Stable partition identity used for ordering and cache lookup.
    pub key: CodegenUnitKey,
    /// Fingerprint of the partition contents and dependencies.
    pub fingerprint: CodegenUnitFingerprint,
    /// Reusable object payload associated with the fingerprint.
    pub object: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Ordered incremental linker inputs.
///
/// The constructor enforces strictly ascending [`CodegenUnitKey`] values. This
/// turns duplicate or nondeterministically ordered cache entries into an ICE
/// before they can alter linker input order.
pub struct IncrementalLinkInputs<T> {
    inputs: Vec<IncrementalLinkInput<T>>,
}

impl<T> IncrementalLinkInputs<T> {
    /// Validates and stores already-sorted incremental inputs.
    pub fn new(inputs: Vec<IncrementalLinkInput<T>>) -> Self {
        for pair in inputs.windows(2) {
            assert!(
                pair[0].key < pair[1].key,
                "Nia ICE: incremental link inputs must have unique stable keys in ascending order"
            );
        }
        Self { inputs }
    }

    /// Borrows the stable input sequence.
    pub fn as_slice(&self) -> &[IncrementalLinkInput<T>] {
        &self.inputs
    }

    /// Returns the number of inputs.
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// Reports whether no inputs are present.
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// Consumes the wrapper and returns its validated vector.
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
/// Deterministic mapping from source modules to code-generation partitions.
///
/// Each non-empty module is one unit until it reaches the split threshold;
/// larger modules use four stable buckets. Definitions are sorted by stable
/// identity before bucketing, and vtables are kept in bucket zero so their
/// emission remains available to every partition.
pub struct CodegenPartitionPlan {
    partitions: Vec<CodegenPartition>,
}

impl CodegenPartitionPlan {
    fn from_modules(modules: &BackendModules) -> Self {
        Self::from_module_iter(modules)
    }

    /// Builds a plan for one module that has just become ready.
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

    /// Returns partitions in ascending stable-key order.
    pub fn partitions(&self) -> &[CodegenPartition] {
        &self.partitions
    }

    /// Asserts that this plan exactly matches the program's current modules.
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
/// Definitions assigned to one source code-generation partition.
///
/// Public accessors expose indexes into the owning [`BackendModule`]. The
/// partition id and key must describe the same module and ordinal; consumers
/// should resolve the owner through [`BackendProgram::module_for_partition`].
pub struct CodegenPartition {
    /// Transient module/ordinal identity used for owner lookup.
    pub id: CodegenUnitId,
    /// Stable identity used for sorting and incremental matching.
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

    /// Returns non-extern global indexes assigned to this partition.
    pub fn global_definitions(&self) -> &[usize] {
        &self.definitions.globals
    }

    /// Returns all materialized global-instance indexes assigned here.
    pub fn global_instance_definitions(&self) -> &[usize] {
        &self.definitions.global_instances
    }

    /// Returns monomorphic, body-bearing function indexes assigned here.
    pub fn function_definitions(&self) -> &[usize] {
        &self.definitions.functions
    }

    /// Returns body-bearing function-instance indexes assigned here.
    pub fn function_instance_definitions(&self) -> &[usize] {
        &self.definitions.function_instances
    }

    /// Returns generated closure-entry indexes assigned here.
    pub fn closure_entry_definitions(&self) -> &[usize] {
        &self.definitions.closure_entries
    }

    /// Returns trait-object-vtable indexes assigned to this partition.
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
                    let owner_symbol = module
                        .function_instances
                        .iter()
                        .find(|instance| {
                            instance.def_id == owner.def_id
                                && instance.arg_module_id == owner.arg_module_id
                                && instance.self_arg == owner.self_arg
                                && instance.args == owner.args
                                && instance.const_args == owner.const_args
                        })
                        .map(|instance| instance.symbol.as_str())
                        // Partition planning precedes backend validation. Keep
                        // malformed direct/cached IR deterministic here so the
                        // validator can diagnose the dangling owner before LLVM.
                        .unwrap_or(&entry.symbol);
                    stable_symbol_bucket(owner_symbol)
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

/// Fully lowered payload for one source module.
#[derive(Debug, PartialEq)]
pub struct BackendModule {
    /// Transient owner id used during this compilation.
    pub id: ModuleId,
    /// Stable source identity used in partition and cache keys.
    pub source_identity: SourceIdentity,
    /// Human-readable module name retained for diagnostics.
    pub name: String,
    /// Const-evaluation facts consumed by backend lowering.
    pub const_eval: BackendConstFacts,
    /// Target layout facts copied across the backend boundary.
    pub layouts: BackendLayouts,
    /// Nominal struct definitions owned by this module.
    pub structs: Vec<BackendStruct>,
    /// Nominal union definitions owned by this module.
    pub unions: Vec<BackendUnion>,
    /// Materialized generic struct instances.
    pub struct_instances: Vec<BackendStructInstance>,
    /// Materialized generic union instances.
    pub union_instances: Vec<BackendUnionInstance>,
    /// Enum definitions with evaluated discriminants.
    pub enums: Vec<BackendEnum>,
    /// Non-generic globals, including extern declarations.
    pub globals: Vec<BackendGlobal>,
    /// Concrete global instances required by reachable code.
    pub global_instances: Vec<BackendGlobalInstance>,
    /// Function definitions, optionally carrying a body.
    pub functions: Vec<BackendFunction>,
    /// Concrete function instances, optionally carrying a body.
    pub function_instances: Vec<BackendFunctionInstance>,
    /// Generated closure entry points and hidden-state ABI.
    ///
    /// Keys must be unique within the program; codegen validation rejects
    /// duplicate definitions before they can alias one LLVM symbol/index slot.
    pub closure_entries: Vec<BackendClosureEntry>,
    /// Materialized trait-object dispatch tables.
    pub trait_object_vtables: Vec<BackendTraitObjectVtable>,
    /// Generic definitions instantiated while lowering this module.
    pub generic_instantiations: Vec<BackendGenericInstantiation>,
}

/// The concrete function whose substitutions determine a generated closure
/// entry. The closure itself retains its source `ClosureId`; this owner key
/// distinguishes entries materialized for separate generic instances.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendClosureEntryOwner {
    /// A source function owns the closure entry directly.
    Source(GlobalDefId),
    /// A concrete function instance supplies its substitutions.
    FunctionInstance(FunctionInstanceKey),
}

/// Stable identity of a generated closure entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendClosureEntryKey {
    /// Source closure identity shared by generic materializations.
    pub closure_id: ClosureId,
    /// Concrete owner that determines captured substitutions.
    ///
    /// Its source `def_id` must equal `closure_id.owner`, and the referenced
    /// source function or concrete function instance must be present in the
    /// backend program before code generation. The entry is published in the
    /// same [`BackendModule`] as that concrete owner.
    pub owner: BackendClosureEntryOwner,
}

/// Backend-visible ABI of a generated closure entry.
///
/// `state_type` is a [`TyKind::ClosureState`](nia_ty::TyKind::ClosureState)
/// whose identity, parameter types, and return type match the entry key and
/// the remaining ABI fields. Backend validation checks that relation before
/// LLVM type construction.
///
/// `state_pointer_type` is the hidden first parameter. User parameters follow
/// in source order and the entry is never variadic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendClosureEntryAbi {
    /// Type of the closure state value.
    pub state_type: InternedTyId,
    /// Pointer type used by the hidden state parameter.
    pub state_pointer_type: InternedTyId,
    /// User-visible parameter types in source order.
    pub params: Vec<InternedTyId>,
    /// Return type of the generated entry point.
    pub return_type: InternedTyId,
}

/// Lowered generated function that invokes one closure body.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendClosureEntry {
    /// Stable closure/owner identity used for deduplication.
    pub key: BackendClosureEntryKey,
    /// Backend symbol emitted for the entry point.
    pub symbol: String,
    /// ABI including the hidden state pointer.
    pub abi: BackendClosureEntryAbi,
    /// Local id bound to the hidden state pointer.
    pub state_param: LocalId,
    /// Local ids for user parameters in ABI order.
    pub params: Vec<LocalId>,
    /// Names retained for diagnostics and debug metadata.
    pub local_names: HashMap<LocalId, String>,
    /// Lowered function body for this concrete entry.
    pub function_body: FunctionBody,
    /// Source span of the closure expression.
    pub span: Span,
}

/// Evaluated constants needed by backend type construction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackendConstFacts {
    /// Evaluated array lengths keyed by canonical const expression.
    pub array_lengths: HashMap<GlobalConstExprId, u64>,
}

/// Target-specific layouts required to lower one module.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendLayouts {
    /// Target pointer width, alignment and endian layout.
    pub target: nia_layout::TargetDataLayout,
    /// Layouts for interned types used by this module.
    pub types: Vec<(InternedTyId, TypeLayout)>,
    /// Layouts for nominal struct definitions.
    pub structs: Vec<(GlobalDefId, StructLayout)>,
    /// Layouts for nominal union definitions.
    pub unions: Vec<(GlobalDefId, StructLayout)>,
    /// Layouts for enum definitions.
    pub enums: Vec<(GlobalDefId, nia_layout::EnumLayout)>,
    /// Layouts for materialized struct instances.
    pub struct_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
    /// Layouts for materialized union instances.
    pub union_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
}

/// Identity of one materialized struct or union layout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendStructInstanceKey {
    /// Defining nominal type identity.
    pub def_id: GlobalDefId,
    /// Type arguments in canonical generic-parameter order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with the type arguments.
    pub const_args: Vec<ConstGenericArg>,
}

impl BackendLayouts {
    /// Copies module-local layout keys into program-wide backend identities.
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
    /// Qualifies a module-local layout key with its owning module.
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

/// Nominal struct definition after semantic validation.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendStruct {
    /// Program-wide definition identity.
    pub def_id: GlobalDefId,
    /// Interned source name.
    pub name: SymbolId,
    /// Declared generic parameters in source order.
    pub generics: Vec<SymbolId>,
    /// Fields in declaration order.
    pub fields: Vec<BackendField>,
    /// Whether storage is supplied by foreign code.
    pub is_extern: bool,
    /// Source declaration span.
    pub span: Span,
}

/// Nominal union definition after semantic validation.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnion {
    /// Program-wide definition identity.
    pub def_id: GlobalDefId,
    /// Interned source name.
    pub name: SymbolId,
    /// Declared generic parameters in source order.
    pub generics: Vec<SymbolId>,
    /// Union fields in declaration order.
    pub fields: Vec<BackendField>,
    /// Whether storage is supplied by foreign code.
    pub is_extern: bool,
    /// Source declaration span.
    pub span: Span,
}

/// Concrete struct instance with substitutions and a backend symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendStructInstance {
    /// Defining nominal type identity.
    pub def_id: GlobalDefId,
    /// Interned instantiated name.
    pub name: SymbolId,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
    /// Stable linker symbol for this instance.
    pub symbol: String,
    /// Instantiated fields in declaration order.
    pub fields: Vec<BackendField>,
    /// Whether storage is supplied by foreign code.
    pub is_extern: bool,
    /// Source declaration span.
    pub span: Span,
}

/// Concrete union instance with substitutions and a backend symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnionInstance {
    /// Defining nominal type identity.
    pub def_id: GlobalDefId,
    /// Interned instantiated name.
    pub name: SymbolId,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
    /// Stable linker symbol for this instance.
    pub symbol: String,
    /// Instantiated fields in declaration order.
    pub fields: Vec<BackendField>,
    /// Whether storage is supplied by foreign code.
    pub is_extern: bool,
    /// Source declaration span.
    pub span: Span,
}

/// One validated field shared by nominal definitions and instances.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendField {
    /// Program-wide field definition identity.
    pub def_id: GlobalDefId,
    /// Interned source name.
    pub name: SymbolId,
    /// Lowered field type.
    pub ty: InternedTyId,
    /// Source field span.
    pub span: Span,
}

/// Enum definition with its target backing type.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnum {
    /// Program-wide definition identity.
    pub def_id: GlobalDefId,
    /// Interned source name.
    pub name: SymbolId,
    /// Integer type used to encode discriminants.
    pub backing_type: InternedTyId,
    /// Variants in declaration order.
    pub variants: Vec<BackendEnumVariant>,
    /// Source declaration span.
    pub span: Span,
}

/// One enum variant and its optional discriminant/payload.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnumVariant {
    /// Program-wide variant identity.
    pub def_id: GlobalDefId,
    /// Interned source name.
    pub name: SymbolId,
    /// Explicit or evaluated discriminant value.
    pub value: Option<i128>,
    /// Validated payload shape.
    pub payload: BackendEnumVariantPayload,
    /// Source variant span.
    pub span: Span,
}

/// Payload layout of an enum variant.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEnumVariantPayload {
    /// No fields or payload.
    Unit,
    /// Unnamed fields in tuple order.
    Tuple(Vec<InternedTyId>),
    /// Named fields in declaration order.
    Named(Vec<BackendField>),
}

/// Non-generic global declaration or definition.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendGlobal {
    /// Program-wide definition identity.
    pub def_id: GlobalDefId,
    /// Interned source name.
    pub name: SymbolId,
    /// Optional externally visible linker name.
    pub link_name: Option<String>,
    /// Declared storage type.
    pub ty: InternedTyId,
    /// Whether the global is immutable after initialization.
    pub is_let: bool,
    /// Whether storage is supplied by foreign code.
    pub is_extern: bool,
    /// Validated static initializer, if one is present.
    pub init: Option<StaticInit>,
    /// Source declaration span.
    pub span: Span,
}

/// Identity of one concrete generic global instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendGlobalInstanceKey {
    /// Defining global identity.
    pub def_id: GlobalDefId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
}

/// Concrete generic global definition.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendGlobalInstance {
    /// Defining global identity.
    pub def_id: GlobalDefId,
    /// Interned instantiated name.
    pub name: SymbolId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
    /// Stable linker symbol for this instance.
    pub symbol: String,
    /// Instantiated storage type.
    pub ty: InternedTyId,
    /// Whether the global is immutable after initialization.
    pub is_let: bool,
    /// Validated static initializer.
    pub init: Option<StaticInit>,
    /// Source declaration span.
    pub span: Span,
}

/// Non-generic function declaration or definition.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunction {
    /// Program-wide definition identity.
    pub def_id: GlobalDefId,
    /// Interned source name.
    pub name: SymbolId,
    /// Optional externally visible linker name.
    pub link_name: Option<String>,
    /// Declared generic parameters in source order.
    pub generics: Vec<SymbolId>,
    /// ABI parameters in source order.
    pub params: Vec<BackendParam>,
    /// Declared return type.
    pub return_type: InternedTyId,
    /// Whether the function uses a foreign calling convention.
    pub is_extern: bool,
    /// Whether the final parameter is variadic.
    pub is_variadic: bool,
    /// Backend attributes already validated by semantic lowering.
    pub attributes: Vec<BackendFunctionAttribute>,
    /// Names retained for diagnostics and debug metadata.
    pub local_names: HashMap<LocalId, String>,
    /// Lowered body for a definition that is emitted locally.
    pub function_body: Option<FunctionBody>,
    /// Source declaration span.
    pub span: Span,
}

/// Backend attributes that affect function emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendFunctionAttribute {
    /// Emit without a compiler-generated prologue or epilogue.
    Naked,
}

/// Concrete generic function definition.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunctionInstance {
    /// Defining function identity.
    pub def_id: GlobalDefId,
    /// Interned instantiated name.
    pub name: SymbolId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Optional receiver substitution for methods.
    pub self_arg: Option<InternedTyId>,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
    /// Stable linker symbol for this instance.
    pub symbol: String,
    /// ABI parameters in source order.
    pub params: Vec<BackendParam>,
    /// Instantiated return type.
    pub return_type: InternedTyId,
    /// Whether the function uses a foreign calling convention.
    pub is_extern: bool,
    /// Whether the final parameter is variadic.
    pub is_variadic: bool,
    /// Backend attributes already validated by semantic lowering.
    pub attributes: Vec<BackendFunctionAttribute>,
    /// Names retained for diagnostics and debug metadata.
    pub local_names: HashMap<LocalId, String>,
    /// Lowered body for an emitted local instance.
    pub function_body: Option<FunctionBody>,
    /// Source declaration span.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Program-wide identity of a concrete trait-object vtable.
///
/// `object_ty` contains the complete trait instance, including type, const, and
/// associated-type arguments. Keeping it in the key makes distinct concrete
/// trait objects distinct even when they share the same erased receiver type.
pub struct BackendTraitObjectVtableKey {
    /// Concrete receiver type represented by the table.
    pub self_ty: InternedTyId,
    /// Complete erased trait-object type, including all arguments.
    pub object_ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
/// Concrete dispatch metadata for one `(self type, trait-object type)` pair.
///
/// The explicit trait arguments mirror `key.object_ty`. They let validators,
/// fingerprints, and dependency discovery consume the instantiated trait
/// contract without decoding the type interner again.
pub struct BackendTraitObjectVtable {
    /// Program-wide vtable identity.
    pub key: BackendTraitObjectVtableKey,
    /// Trait segment represented by the table's primary view.
    pub trait_id: TraitId,
    /// Type arguments for `trait_id` in canonical order.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments paired with `trait_args`.
    pub trait_const_args: Vec<ConstGenericArg>,
    /// Method slots, including inherited supertrait segments.
    pub entries: Vec<BackendTraitObjectVtableEntry>,
    /// Source span of the trait-object use that required the table.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// One method slot together with the exact trait segment that owns it.
///
/// A vtable may contain multiple instantiations of the same supertrait, so the
/// trait id alone is not sufficient to identify a slot.
pub struct BackendTraitObjectVtableEntry {
    /// Trait segment owning this slot.
    pub trait_id: TraitId,
    /// Type arguments for the owning trait segment.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments for the owning trait segment.
    pub trait_const_args: Vec<ConstGenericArg>,
    /// Declared method identity represented by the slot.
    pub method_id: GlobalDefId,
    /// Interned method name for diagnostics.
    pub method_name: SymbolId,
    /// ABI slot index in the complete table.
    pub slot: usize,
    /// Direct function or concrete function instance target.
    pub function: BackendTraitObjectVtableFunction,
}

/// Dispatch target stored in a trait-object vtable slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendTraitObjectVtableFunction {
    /// Monomorphic function definition.
    Function(GlobalDefId),
    /// Concrete generic function instance.
    FunctionInstance {
        /// Defining function identity.
        def_id: GlobalDefId,
        /// Module supplying generic argument resolution context.
        arg_module_id: ModuleId,
        /// Optional receiver substitution for methods.
        self_arg: Option<InternedTyId>,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
    },
}

/// Record of one generic definition instantiated by backend reachability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendGenericInstantiation {
    /// Definition being instantiated.
    pub def_id: GlobalDefId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Optional receiver substitution for methods.
    pub self_arg: Option<InternedTyId>,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
    /// Source span of the instantiation request.
    pub span: Span,
    /// Original source definition when this is a propagated request.
    pub source_def_id: Option<GlobalDefId>,
}

/// One validated ABI parameter and its source-local metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendParam {
    /// Local binding id, absent for an unbound parameter.
    pub local_id: Option<LocalId>,
    /// Optional source name.
    pub name: Option<SymbolId>,
    /// Receiver marker for method parameters.
    pub receiver: Option<ReceiverKind>,
    /// Type passed across the backend ABI.
    pub passing_ty: InternedTyId,
    /// Source-level type used inside the body.
    pub local_ty: InternedTyId,
    /// Source parameter span.
    pub span: Span,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
