// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy)]
pub struct BodyConst<'a> {
    pub values: &'a HashMap<ConstKey, ConstValue>,
    pub typed_values: &'a HashMap<ConstKey, TypedConstValue>,
    pub array_lengths: &'a HashMap<nia_ids::GlobalConstExprId, u64>,
}

impl<'a> BodyConst<'a> {
    pub fn from_phases(
        values: &'a ConstValues,
        array_lengths: &'a ConstArrayLengths,
        typed_facts: &'a ConstTypedFacts,
    ) -> Self {
        Self {
            values: &values.values,
            typed_values: &typed_facts.typed_values,
            array_lengths: &array_lengths.values,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ProgramConstMaps<'a> {
    pub values: &'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>,
    pub array_lengths: &'a dyn Fn(ModuleId) -> Option<Arc<ConstArrayLengths>>,
    pub module: &'a dyn Fn(ModuleId) -> Option<Arc<ResolvedConstModule>>,
}

impl fmt::Debug for ProgramConstMaps<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgramConstMaps")
            .field("values", &true)
            .field("array_lengths", &true)
            .field("module", &true)
            .finish()
    }
}

impl ProgramConstMaps<'_> {
    pub fn empty() -> Self {
        Self {
            values: &no_program_const_values,
            array_lengths: &no_program_const_array_lengths,
            module: &no_program_const_module,
        }
    }
}

fn no_program_const_values(_: ModuleId) -> Option<Arc<ConstValues>> {
    None
}

fn no_program_const_array_lengths(_: ModuleId) -> Option<Arc<ConstArrayLengths>> {
    None
}

fn no_program_const_module(_: ModuleId) -> Option<Arc<ResolvedConstModule>> {
    None
}

#[derive(Debug, Clone, Copy, Default)]
pub enum BodyCheckFilter<'a> {
    #[default]
    All,
    ReachableFunctions(&'a HashSet<GlobalDefId>),
    ReachableItems {
        functions: &'a HashSet<GlobalDefId>,
        globals: &'a HashSet<GlobalDefId>,
        already_checked_functions: Option<&'a HashSet<GlobalDefId>>,
        already_checked_globals: Option<&'a HashSet<GlobalDefId>>,
    },
}

type ExtensionMethodsNamed<'a> = &'a dyn Fn(&SymbolId) -> Vec<ExtensionMethod>;

#[derive(Clone, Copy)]
pub struct BodyProgramContext<'a> {
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
    pub module_source_path: Option<&'a dyn Fn(ModuleId) -> Option<SourcePath>>,
    pub type_normalizations: Option<&'a dyn Fn(ModuleId) -> Option<Arc<TypeNormalization>>>,
    pub extension_type_normalizations:
        Option<&'a dyn Fn(ModuleId) -> Option<Arc<TypeNormalization>>>,
    pub signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    pub layouts: Option<&'a dyn Fn(ModuleId) -> Option<Arc<Layouts>>>,
    pub visible_extensions: Option<&'a dyn Fn(ModuleId) -> Option<VisibleExtensionMethods>>,
    pub extension_method_by_id: Option<&'a dyn Fn(GlobalDefId) -> Option<ExtensionMethod>>,
    pub extension_methods_named: Option<ExtensionMethodsNamed<'a>>,
}

impl BodyProgramContext<'_> {
    pub fn empty() -> Self {
        Self {
            defs: None,
            module_source_path: None,
            type_normalizations: None,
            extension_type_normalizations: None,
            signatures: None,
            layouts: None,
            visible_extensions: None,
            extension_method_by_id: None,
            extension_methods_named: None,
        }
    }
}

impl fmt::Debug for BodyProgramContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyProgramContext")
            .field("defs", &self.defs.is_some())
            .field("module_source_path", &self.module_source_path.is_some())
            .field("type_normalizations", &self.type_normalizations.is_some())
            .field(
                "extension_type_normalizations",
                &self.extension_type_normalizations.is_some(),
            )
            .field("signatures", &self.signatures.is_some())
            .field("layouts", &self.layouts.is_some())
            .field(
                "extension_method_by_id",
                &self.extension_method_by_id.is_some(),
            )
            .field(
                "extension_methods_named",
                &self.extension_methods_named.is_some(),
            )
            .finish()
    }
}

#[derive(Clone, Copy)]
pub struct BodyVisibleExtensions<'a> {
    pub methods: &'a VisibleExtensionMethods,
    pub lazy: Option<&'a dyn Fn() -> VisibleExtensionMethods>,
}

#[derive(Clone)]
pub struct BodyCheckInput<'a> {
    pub type_store: &'a nia_ty::TypeStore,
    pub source_version: Option<SourceVersion>,
    pub source_path: &'a SourcePath,
    pub symbols: &'a SymbolTable,
    pub origins: &'a NodeOriginTable,
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub lowered: &'a TypeLowering,
    pub signatures: BodyLocalSignatures<'a>,
    pub const_signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub seed: Option<BodyCheckSeed<'a>>,
    pub target: &'a TargetConfig,
    pub const_eval: BodyConst<'a>,
    pub const_module: &'a ResolvedConstModule,
    pub layouts: &'a Layouts,
    pub extensions: &'a VisibleExtensionMethods,
    pub lazy_extensions: Option<&'a dyn Fn() -> VisibleExtensionMethods>,
    pub program_extension_methods: &'a ExtensionMethods,
    pub program: BodyProgramContext<'a>,
    pub program_signatures: ProgramSignatureContext<'a>,
    pub function_scope: FunctionCheckScope,
    pub program_const: ProgramConstMaps<'a>,
    pub filter: BodyCheckFilter<'a>,
    pub product: BodyCheckProduct,
    pub prechecked: Option<PrecheckedBodyCheck>,
}

#[derive(Clone, Copy)]
pub struct BodyCheckSeed<'a> {
    pub facts: &'a SemanticFacts,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyLocalSignatures<'a> {
    pub functions: &'a HashMap<DefId, FunctionSignature>,
    pub globals: &'a HashMap<DefId, GlobalSignature>,
    pub consts: &'a HashMap<DefId, ConstSignature>,
    pub structs: &'a HashMap<DefId, StructSignature>,
    pub unions: &'a HashMap<DefId, UnionSignature>,
    pub enums: &'a HashMap<DefId, EnumSignature>,
    pub type_aliases: &'a HashMap<DefId, TypeAliasSignature>,
    pub traits: &'a HashMap<DefId, TraitSignature>,
    pub trait_impls: &'a [TraitImplSignature],
}

impl<'a> BodyLocalSignatures<'a> {
    pub fn from_item_signatures(signatures: &'a ItemSignatures) -> Self {
        Self {
            functions: &signatures.functions,
            globals: &signatures.globals,
            consts: &signatures.consts,
            structs: &signatures.structs,
            unions: &signatures.unions,
            enums: &signatures.enums,
            type_aliases: &signatures.type_aliases,
            traits: &signatures.traits,
            trait_impls: &signatures.trait_impls,
        }
    }
}

#[derive(Clone, Copy)]
pub struct BodyCheckWithProgramSignaturesInput<'a> {
    pub type_store: &'a nia_ty::TypeStore,
    pub source_version: Option<SourceVersion>,
    pub source_path: &'a SourcePath,
    pub symbols: &'a SymbolTable,
    pub origins: &'a NodeOriginTable,
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub target: &'a TargetConfig,
    pub const_eval: BodyConst<'a>,
    pub const_module: &'a ResolvedConstModule,
    pub extensions: &'a VisibleExtensionMethods,
    pub program_extension_methods: &'a ExtensionMethods,
    pub program: BodyProgramContext<'a>,
    pub program_signatures: ProgramSignatureContext<'a>,
    pub function_scope: FunctionCheckScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCheckScope {
    LocalModule,
    ProgramSignatures,
}
