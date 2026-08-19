// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy)]
/// Const products made available while checking function bodies.
pub struct BodyConst<'a> {
    /// Evaluated const values keyed by global/local identity.
    pub values: &'a HashMap<ConstKey, ConstValue>,
    /// Const values paired with inferred runtime types.
    pub typed_values: &'a HashMap<ConstKey, TypedConstValue>,
    /// Array lengths computed by the const prerequisite phase.
    pub array_lengths: &'a HashMap<nia_ids::GlobalConstExprId, u64>,
}

impl<'a> BodyConst<'a> {
    /// Builds a body-check view from cached const phases.
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
/// Lazy cross-module const maps used by body checking.
pub struct ProgramConstMaps<'a> {
    /// Loads const values for another module.
    pub values: &'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>,
    /// Loads array lengths for another module.
    pub array_lengths: &'a dyn Fn(ModuleId) -> Option<Arc<ConstArrayLengths>>,
    /// Loads resolved const IR for another module.
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
    /// Creates a context whose cross-module providers all return unavailable.
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
/// Selects body-check work by product and executable reachability.
pub enum BodyCheckFilter<'a> {
    /// Checks all active items.
    #[default]
    All,
    /// Checks only const declarations.
    ConstDeclarations,
    /// Checks reachable functions only.
    ReachableFunctions(&'a HashSet<GlobalDefId>),
    /// Checks selected functions/globals with prior-product reuse markers.
    ReachableItems {
        /// Reachable function identities.
        functions: &'a HashSet<GlobalDefId>,
        /// Reachable global identities.
        globals: &'a HashSet<GlobalDefId>,
        /// Functions already covered by a prior product.
        already_checked_functions: Option<&'a HashSet<GlobalDefId>>,
        /// Globals already covered by a prior product.
        already_checked_globals: Option<&'a HashSet<GlobalDefId>>,
    },
}

type ExtensionMethodsNamed<'a> = &'a dyn Fn(&SymbolId) -> Vec<ExtensionMethod>;

#[derive(Clone, Copy)]
/// Optional program-wide providers used during body checking.
pub struct BodyProgramContext<'a> {
    /// Loads definitions for another module.
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
    /// Loads another module's source path.
    pub module_source_path: Option<&'a dyn Fn(ModuleId) -> Option<SourcePath>>,
    /// Loads normalized types for another module.
    pub type_normalizations: Option<&'a dyn Fn(ModuleId) -> Option<Arc<TypeNormalization>>>,
    /// Loads extension-module normalized types.
    pub extension_type_normalizations:
        Option<&'a dyn Fn(ModuleId) -> Option<Arc<TypeNormalization>>>,
    /// Loads item signatures for another module.
    pub signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    /// Loads layouts for another module.
    pub layouts: Option<&'a dyn Fn(ModuleId) -> Option<Arc<Layouts>>>,
    /// Loads visible extension methods.
    pub visible_extensions: Option<&'a dyn Fn(ModuleId) -> Option<VisibleExtensionMethods>>,
    /// Resolves one extension method by stable identity.
    pub extension_method_by_id: Option<&'a dyn Fn(GlobalDefId) -> Option<ExtensionMethod>>,
    /// Resolves extension methods by source name.
    pub extension_methods_named: Option<ExtensionMethodsNamed<'a>>,
}

impl BodyProgramContext<'_> {
    /// Creates a context with every optional provider unavailable.
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
/// Visible extension methods and an optional lazy refresh provider.
pub struct BodyVisibleExtensions<'a> {
    /// Current visible extension set.
    pub methods: &'a VisibleExtensionMethods,
    /// Optional provider used when visibility is queried lazily.
    pub lazy: Option<&'a dyn Fn() -> VisibleExtensionMethods>,
}

#[derive(Clone)]
/// Complete local and program context for one body-check query.
pub struct BodyCheckInput<'a> {
    /// Shared interned type store.
    pub type_store: &'a nia_ty::TypeStore,
    /// Source revision used for identity validation.
    pub source_version: Option<SourceVersion>,
    /// Source path for diagnostics.
    pub source_path: &'a SourcePath,
    /// Symbol table for names and diagnostics.
    pub symbols: &'a SymbolTable,
    /// Node-origin table for versioned AST keys.
    pub origins: &'a NodeOriginTable,
    /// Active item tree selecting the current revision.
    pub active_item_tree: &'a ActiveModuleItemTree,
    /// Definition identities for the local module.
    pub defs: &'a DefCollection,
    /// Resolved value identities.
    pub values: &'a ValueResolution,
    /// Resolved local identities.
    pub locals: &'a LocalResolution,
    /// Semantic use facts.
    pub semantic_uses: &'a SemanticUseTable,
    /// Lowered runtime type information.
    pub lowered: &'a TypeLowering,
    /// Local-module signatures.
    pub signatures: BodyLocalSignatures<'a>,
    /// Const signatures used by initializer checks.
    pub const_signatures: &'a ItemSignatures,
    /// Type normalization service.
    pub normalization: &'a TypeNormalization,
    /// Optional facts from an earlier query.
    pub seed: Option<BodyCheckSeed<'a>>,
    /// Target layout and primitive configuration.
    pub target: &'a TargetConfig,
    /// Cached const evaluation products.
    pub const_eval: BodyConst<'a>,
    /// Resolved const module for local initializers.
    pub const_module: &'a ResolvedConstModule,
    /// Computed layouts.
    pub layouts: &'a Layouts,
    /// Visible extension methods.
    pub extensions: &'a VisibleExtensionMethods,
    /// Optional lazy extension refresh.
    pub lazy_extensions: Option<&'a dyn Fn() -> VisibleExtensionMethods>,
    /// Program-local extension methods.
    pub program_extension_methods: &'a ExtensionMethods,
    /// Optional cross-module providers.
    pub program: BodyProgramContext<'a>,
    /// Program-wide signature scope.
    pub program_signatures: ProgramSignatureContext<'a>,
    /// Function signature visibility scope.
    pub function_scope: FunctionCheckScope,
    /// Lazy cross-module const products.
    pub program_const: ProgramConstMaps<'a>,
    /// Product/reachability filter.
    pub filter: BodyCheckFilter<'a>,
    /// Requested output product.
    pub product: BodyCheckProduct,
    /// Optional prior checked product for reuse.
    pub prechecked: Option<PrecheckedBodyCheck>,
}

#[derive(Clone, Copy)]
/// Seed semantic facts reused by an incremental body-check query.
pub struct BodyCheckSeed<'a> {
    /// Previously collected semantic facts.
    pub facts: &'a SemanticFacts,
}

#[derive(Debug, Clone, Copy)]
/// Local signatures projected from one item-signature product.
pub struct BodyLocalSignatures<'a> {
    /// Function signatures.
    pub functions: &'a HashMap<DefId, FunctionSignature>,
    /// Global signatures.
    pub globals: &'a HashMap<DefId, GlobalSignature>,
    /// Const signatures.
    pub consts: &'a HashMap<DefId, ConstSignature>,
    /// Struct signatures.
    pub structs: &'a HashMap<DefId, StructSignature>,
    /// Union signatures.
    pub unions: &'a HashMap<DefId, UnionSignature>,
    /// Enum signatures.
    pub enums: &'a HashMap<DefId, EnumSignature>,
    /// Type-alias signatures.
    pub type_aliases: &'a HashMap<DefId, TypeAliasSignature>,
    /// Trait signatures.
    pub traits: &'a HashMap<DefId, TraitSignature>,
    /// Trait implementation signatures.
    pub trait_impls: &'a [TraitImplSignature],
}

impl<'a> BodyLocalSignatures<'a> {
    /// Projects all local signature maps from an item-signature product.
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
/// Body-check inputs using one program-wide signature scope.
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
/// Determines whether function lookup is local or program-wide.
pub enum FunctionCheckScope {
    /// Resolve functions from the active module only.
    LocalModule,
    /// Resolve functions through program signatures.
    ProgramSignatures,
}
