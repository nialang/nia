// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{BTreeSet, HashMap, HashSet};

use nia_function_ir::FunctionBody;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, LocalId, ModuleId, ReceiverKind};
use nia_layout::{Layouts, StructLayout, StructLayoutKey, TypeLayout};
use nia_source::SourceIdentity;
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_symbol::SymbolId;
use nia_ty::{ConstGenericArg, TraitId};

const SOURCE_CODEGEN_BUCKETS: usize = 4;
const SOURCE_CODEGEN_SPLIT_THRESHOLD: usize = 8;

#[derive(Debug, PartialEq)]
pub struct BackendProgram {
    pub modules: Vec<BackendModule>,
}

impl BackendProgram {
    pub fn codegen_partition_plan(&self) -> CodegenPartitionPlan {
        CodegenPartitionPlan::from_modules(&self.modules)
    }

    pub fn module_for_partition(&self, partition: &CodegenPartition) -> &BackendModule {
        let module = self.modules.get(partition.module_index).unwrap_or_else(|| {
            panic!(
                "Nia ICE: codegen partition {:?} references missing backend module index {}",
                partition.id, partition.module_index
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
    fn from_modules(modules: &[BackendModule]) -> Self {
        let mut vtable_definitions = HashSet::new();
        for module in modules {
            for vtable in &module.trait_object_vtables {
                assert!(
                    vtable_definitions.insert(vtable.key.clone()),
                    "Nia ICE: backend program contains duplicate trait-object vtable definition {:?}",
                    vtable.key
                );
            }
        }
        let mut partitions = modules
            .iter()
            .enumerate()
            .flat_map(|(module_index, module)| {
                CodegenPartitionDefinitions::for_module(module)
                    .into_iter()
                    .map(move |(ordinal, definitions)| CodegenPartition {
                        id: CodegenUnitId::source_module(module.id, ordinal),
                        key: CodegenUnitKey::source_module(module.source_identity.clone(), ordinal),
                        module_index,
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
    module_index: usize,
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
    vtables: Vec<usize>,
}

impl CodegenPartitionDefinitions {
    fn from_module(module: &BackendModule) -> Self {
        Self {
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
            vtables: (0..module.trait_object_vtables.len()).collect(),
        }
    }

    fn is_empty(&self) -> bool {
        self.globals.is_empty()
            && self.global_instances.is_empty()
            && self.functions.is_empty()
            && self.function_instances.is_empty()
            && self.vtables.is_empty()
    }

    fn len(&self) -> usize {
        self.globals.len()
            + self.global_instances.len()
            + self.functions.len()
            + self.function_instances.len()
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
    pub trait_object_vtables: Vec<BackendTraitObjectVtable>,
    pub generic_instantiations: Vec<BackendGenericInstantiation>,
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
    pub span: Span,
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
pub struct BackendTraitObjectVtableKey {
    pub self_ty: InternedTyId,
    pub object_ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendTraitObjectVtable {
    pub key: BackendTraitObjectVtableKey,
    pub trait_id: TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub entries: Vec<BackendTraitObjectVtableEntry>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendTraitObjectVtableEntry {
    pub trait_id: TraitId,
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
mod tests {
    use nia_ids::{DefId, GlobalDefId, ModuleIdAllocator};
    use nia_layout::TargetDataLayout;
    use nia_symbol::SymbolId;
    use nia_ty::PrimitiveTy;

    use super::*;

    fn module_with_global(
        module_id: ModuleId,
        ty: InternedTyId,
        name: &str,
        is_extern: bool,
    ) -> BackendModule {
        BackendModule {
            id: module_id,
            source_identity: SourceIdentity::new(name),
            name: name.to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: TargetDataLayout::LP64,
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: vec![BackendGlobal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: SymbolId::EMPTY,
                link_name: None,
                ty,
                is_let: false,
                is_extern,
                init: None,
                span: Span::default(),
            }],
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }
    }

    #[test]
    fn codegen_partitions_are_definition_filtered_and_stable_key_ordered() {
        let mut module_ids = ModuleIdAllocator::new();
        let first_id = module_ids.allocate();
        let declaration_id = module_ids.allocate();
        let second_id = module_ids.allocate();
        let type_store = nia_ty::TypeStore::new();
        let first_ty = type_store
            .append_for_module(first_id)
            .primitive(PrimitiveTy::I32);
        let declaration_ty = type_store
            .append_for_module(declaration_id)
            .primitive(PrimitiveTy::I32);
        let second_ty = type_store
            .append_for_module(second_id)
            .primitive(PrimitiveTy::I32);
        let program = BackendProgram {
            modules: vec![
                module_with_global(second_id, second_ty, "second", false),
                module_with_global(declaration_id, declaration_ty, "declaration", true),
                module_with_global(first_id, first_ty, "first", false),
            ],
        };

        let plan = program.codegen_partition_plan();
        let partitions = plan.partitions();
        assert_eq!(
            partitions
                .iter()
                .map(|partition| partition.id)
                .collect::<Vec<_>>(),
            vec![
                CodegenUnitId::SourceModule {
                    module_id: first_id,
                    ordinal: 0,
                },
                CodegenUnitId::SourceModule {
                    module_id: second_id,
                    ordinal: 0,
                },
            ]
        );
        assert_eq!(program.module_for_partition(&partitions[0]).name, "first");
        assert_eq!(program.module_for_partition(&partitions[1]).name, "second");
        for partition in partitions {
            assert_eq!(partition.global_definitions(), &[0]);
            assert!(partition.global_instance_definitions().is_empty());
            assert!(partition.function_definitions().is_empty());
            assert!(partition.function_instance_definitions().is_empty());
            assert!(partition.vtable_definitions().is_empty());
        }
        assert_eq!(
            partitions
                .iter()
                .map(|partition| partition.key.clone())
                .collect::<Vec<_>>(),
            vec![
                CodegenUnitKey::SourceModule {
                    source_identity: SourceIdentity::new("first"),
                    ordinal: 0,
                },
                CodegenUnitKey::SourceModule {
                    source_identity: SourceIdentity::new("second"),
                    ordinal: 0,
                },
            ]
        );
    }

    #[test]
    fn codegen_partition_order_does_not_depend_on_module_id_allocation() {
        let mut module_ids = ModuleIdAllocator::new();
        let z_id = module_ids.allocate();
        let a_id = module_ids.allocate();
        let type_store = nia_ty::TypeStore::new();
        let z_ty = type_store
            .append_for_module(z_id)
            .primitive(PrimitiveTy::I32);
        let a_ty = type_store
            .append_for_module(a_id)
            .primitive(PrimitiveTy::I32);
        let program = BackendProgram {
            modules: vec![
                module_with_global(z_id, z_ty, "z", false),
                module_with_global(a_id, a_ty, "a", false),
            ],
        };

        let plan = program.codegen_partition_plan();

        assert_eq!(program.module_for_partition(&plan.partitions()[0]).id, a_id);
        assert_eq!(program.module_for_partition(&plan.partitions()[1]).id, z_id);
    }

    #[test]
    fn large_source_modules_use_stable_bounded_definition_buckets() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = nia_ty::TypeStore::new();
        let ty = type_store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let mut module = module_with_global(module_id, ty, "main", false);
        let template = module.globals[0].clone();
        module.globals = (0..8)
            .map(|index| BackendGlobal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(index),
                },
                ..template.clone()
            })
            .collect();
        let program = BackendProgram {
            modules: vec![module],
        };

        let plan = program.codegen_partition_plan();

        assert_eq!(plan.partitions().len(), SOURCE_CODEGEN_BUCKETS);
        for (ordinal, partition) in plan.partitions().iter().enumerate() {
            assert_eq!(
                partition.id,
                CodegenUnitId::SourceModule {
                    module_id,
                    ordinal: ordinal as u32,
                }
            );
            assert_eq!(partition.global_definitions(), &[ordinal, ordinal + 4]);
        }
    }

    #[test]
    #[should_panic(expected = "duplicate stable codegen partition key")]
    fn codegen_partition_plan_rejects_duplicate_stable_source_keys() {
        let mut module_ids = ModuleIdAllocator::new();
        let first_id = module_ids.allocate();
        let second_id = module_ids.allocate();
        let type_store = nia_ty::TypeStore::new();
        let first_ty = type_store
            .append_for_module(first_id)
            .primitive(PrimitiveTy::I32);
        let second_ty = type_store
            .append_for_module(second_id)
            .primitive(PrimitiveTy::I32);
        let program = BackendProgram {
            modules: vec![
                module_with_global(first_id, first_ty, "same", false),
                module_with_global(second_id, second_ty, "same", false),
            ],
        };

        let _ = program.codegen_partition_plan();
    }

    #[test]
    #[should_panic(expected = "duplicate trait-object vtable definition")]
    fn codegen_partition_plan_rejects_duplicate_vtable_definitions() {
        let mut module_ids = ModuleIdAllocator::new();
        let first_id = module_ids.allocate();
        let second_id = module_ids.allocate();
        let type_store = nia_ty::TypeStore::new();
        let ty = type_store
            .append_for_module(first_id)
            .primitive(PrimitiveTy::I32);
        let trait_id = TraitId::Source(GlobalDefId {
            module_id: first_id,
            def_id: DefId(1),
        });
        let vtable = BackendTraitObjectVtable {
            key: BackendTraitObjectVtableKey {
                self_ty: ty,
                object_ty: ty,
            },
            trait_id,
            trait_args: Vec::new(),
            entries: Vec::new(),
            span: Span::default(),
        };
        let mut first = module_with_global(first_id, ty, "first", false);
        first.trait_object_vtables.push(vtable.clone());
        let mut second = module_with_global(second_id, ty, "second", false);
        second.trait_object_vtables.push(vtable);
        let program = BackendProgram {
            modules: vec![first, second],
        };

        let _ = program.codegen_partition_plan();
    }

    #[test]
    #[should_panic(expected = "codegen partition plan does not match")]
    fn codegen_partition_plan_rejects_definition_membership_mutation() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = nia_ty::TypeStore::new();
        let ty = type_store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let mut program = BackendProgram {
            modules: vec![module_with_global(module_id, ty, "main", false)],
        };
        let plan = program.codegen_partition_plan();
        program.modules[0].globals.clear();

        plan.validate_program(&program);
    }

    #[test]
    fn codegen_unit_dependencies_preserve_unit_and_canonicalize_modules() {
        let mut module_ids = ModuleIdAllocator::new();
        let first_id = module_ids.allocate();
        let second_id = module_ids.allocate();
        let unit = CodegenUnitId::SourceModule {
            module_id: first_id,
            ordinal: 2,
        };

        let dependencies = CodegenUnitDependencies::new(unit, [second_id, first_id, second_id]);

        assert_eq!(dependencies.unit(), unit);
        assert_eq!(dependencies.modules(), &[first_id, second_id]);
        assert!(dependencies.contains(first_id));
        assert!(dependencies.contains(second_id));
    }

    #[test]
    #[should_panic(expected = "dependency modules must include its owner")]
    fn codegen_unit_dependencies_reject_empty_module_sets() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();

        let _ = CodegenUnitDependencies::new(
            CodegenUnitId::SourceModule {
                module_id,
                ordinal: 0,
            },
            [],
        );
    }

    fn incremental_link_input(path: &str, key: CodegenUnitKey) -> IncrementalLinkInput<String> {
        IncrementalLinkInput {
            key,
            fingerprint: CodegenUnitFingerprint::from_parts([1, 2]),
            object: path.to_string(),
        }
    }

    #[test]
    fn incremental_link_inputs_accept_strict_stable_key_order() {
        let inputs = IncrementalLinkInputs::new(vec![
            incremental_link_input(
                "main.o",
                CodegenUnitKey::SourceModule {
                    source_identity: SourceIdentity::new("main.nia"),
                    ordinal: 0,
                },
            ),
            incremental_link_input("builtins.o", CodegenUnitKey::CompilerBuiltins),
        ]);

        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs.as_slice()[0].object, "main.o");
        assert_eq!(inputs.into_vec()[1].object, "builtins.o");
    }

    #[test]
    fn empty_incremental_link_inputs_are_valid() {
        let inputs = IncrementalLinkInputs::<String>::default();

        assert!(inputs.is_empty());
        assert!(inputs.as_slice().is_empty());
    }

    #[test]
    #[should_panic(expected = "unique stable keys in ascending order")]
    fn incremental_link_inputs_reject_duplicate_keys() {
        let _ = IncrementalLinkInputs::new(vec![
            incremental_link_input("first.o", CodegenUnitKey::CompilerBuiltins),
            incremental_link_input("second.o", CodegenUnitKey::CompilerBuiltins),
        ]);
    }

    #[test]
    #[should_panic(expected = "unique stable keys in ascending order")]
    fn incremental_link_inputs_reject_descending_keys() {
        let _ = IncrementalLinkInputs::new(vec![
            incremental_link_input("builtins.o", CodegenUnitKey::CompilerBuiltins),
            incremental_link_input(
                "main.o",
                CodegenUnitKey::SourceModule {
                    source_identity: SourceIdentity::new("main.nia"),
                    ordinal: 0,
                },
            ),
        ]);
    }
}
