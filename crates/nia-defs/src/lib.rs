// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

mod extensions;
mod public_surface;

pub use nia_ast::PathSegmentKind;
use nia_ast::{
    BindingItem, Block, EnumItem, ExtendAssociatedType, ExtendAssociatedValue, ExtendItem,
    FunctionItem, GenericParam, Module, StmtKind, StructItem, TraitAssociatedType,
    TraitAssociatedValue, TypeAliasItem, UnionItem, UsingItem, generic_param_identities,
    type_ref_identity, where_clause_identity,
};
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::{DefId, ModuleId, Visibility};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::{NodeId, NodeMap, NodeMapBuilder, NodeStore, NodeStoreId, VersionedNodeKey};
use nia_span::Span;
use nia_symbol::{
    SymbolId, SymbolMap, SymbolText, symbol_identity_key, symbol_text_from_optional_resolver,
};

pub use extensions::{
    AssociatedTypeBindingSignature, ExtensionAssociatedValue, ExtensionAssociatedValues,
    ExtensionMethod, ExtensionMethods, VisibleExtensionAssociatedValue, VisibleExtensionMethod,
    VisibleExtensionMethods, VisibleExtensionTarget, WhereBoundSignature, WherePredicateSignature,
};
pub use public_surface::{
    ModulePublicSurface, ModuleUsingScope, PublicItem, PublicNamespace, PublicSource,
    PublicSurfaceLookup, PublicSurfaces, UsingEntry, UsingScopeLookup,
};

#[derive(Debug, Clone, PartialEq)]
pub struct DefCollection {
    pub module_id: ModuleId,
    pub defs: DefMap,
    pub module_scope: ModuleScope,
    pub scopes: DefScopes,
    pub def_nodes: DefNodeMap,
    pub module_usings: Vec<ModuleUsing>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSurfaceDefFact {
    pub id: DefId,
    pub name: SymbolId,
    pub kind: DefKind,
    pub parent: Option<DefId>,
    pub visibility: Visibility,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublicSurfaceModuleScopeFacts {
    pub modules: Vec<(SymbolId, DefId)>,
    pub types: Vec<(SymbolId, DefId)>,
    pub values: Vec<(SymbolId, DefId)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSurfaceEnumScopeFact {
    pub owner: DefId,
    pub variants: Vec<(SymbolId, DefId)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublicSurfaceModuleFacts {
    pub defs: Vec<PublicSurfaceDefFact>,
    pub module_scope: PublicSurfaceModuleScopeFacts,
    pub enum_scopes: Vec<PublicSurfaceEnumScopeFact>,
    pub module_usings: Vec<ModuleUsing>,
}

impl PublicSurfaceModuleFacts {
    pub fn from_defs(defs: &DefCollection) -> Self {
        let mut def_facts = defs
            .defs
            .iter()
            .map(|(id, def)| PublicSurfaceDefFact {
                id,
                name: def.name,
                kind: def.kind,
                parent: def.parent,
                visibility: def.visibility,
                span: def.span,
            })
            .collect::<Vec<_>>();
        def_facts.sort_by_key(|def| def.id);
        let mut enum_scopes = defs
            .scopes
            .enum_members
            .iter()
            .map(|(owner, scope)| PublicSurfaceEnumScopeFact {
                owner: *owner,
                variants: sorted_name_entries(&scope.variants),
            })
            .collect::<Vec<_>>();
        enum_scopes.sort_by_key(|scope| scope.owner);
        Self {
            defs: def_facts,
            module_scope: PublicSurfaceModuleScopeFacts {
                modules: sorted_name_entries(&defs.module_scope.modules),
                types: sorted_name_entries(&defs.module_scope.types),
                values: sorted_name_entries(&defs.module_scope.values),
            },
            enum_scopes,
            module_usings: defs.module_usings.clone(),
        }
    }

    pub fn materialize_for_public_surface(&self, module_id: ModuleId) -> DefCollection {
        let mut defs = DefMap::default();
        for fact in &self.defs {
            let index = defs.defs.len();
            defs.defs.push(DefEntry {
                id: fact.id,
                identity: DefIdentity::cached(fact.id),
                def: Def {
                    name: fact.name,
                    kind: fact.kind,
                    module_id,
                    parent: fact.parent,
                    generics: Vec::new(),
                    generic_params: Vec::new(),
                    visibility: fact.visibility,
                    span: fact.span,
                },
            });
            defs.by_id.insert(fact.id, index);
        }
        let enum_members = self
            .enum_scopes
            .iter()
            .map(|scope| {
                (
                    scope.owner,
                    EnumScope {
                        variants: name_table_from_fact_entries(&scope.variants),
                    },
                )
            })
            .collect();
        DefCollection {
            module_id,
            defs,
            module_scope: ModuleScope {
                modules: name_table_from_fact_entries(&self.module_scope.modules),
                types: name_table_from_fact_entries(&self.module_scope.types),
                values: name_table_from_fact_entries(&self.module_scope.values),
            },
            scopes: DefScopes {
                struct_members: HashMap::new(),
                union_members: HashMap::new(),
                enum_members,
            },
            def_nodes: DefNodeMap::default(),
            module_usings: self.module_usings.clone(),
            diagnostics: Vec::new(),
        }
    }
}

fn sorted_name_entries(table: &NameTable) -> Vec<(SymbolId, DefId)> {
    let mut entries = table
        .entries()
        .map(|(name, id)| (*name, id))
        .collect::<Vec<_>>();
    entries.sort_unstable();
    entries
}

fn name_table_from_fact_entries(entries: &[(SymbolId, DefId)]) -> NameTable {
    NameTable {
        entries: entries
            .iter()
            .map(|(name, def_id)| {
                (
                    *name,
                    NameEntry {
                        def_id: *def_id,
                        span: Span::default(),
                    },
                )
            })
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleUsing {
    pub visibility: Visibility,
    pub span: Span,
    pub host: Vec<UsingPathSegment>,
    pub selector: UsingSelector,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingPathSegment {
    pub kind: PathSegmentKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsingSelector {
    Single(UsingName),
    Group(Vec<UsingGroupItem>),
    Wildcard { span: Span },
    SelfName,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsingGroupItem {
    Name(UsingName),
    Nested {
        host: Vec<UsingPathSegment>,
        selector: Box<UsingSelector>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingName {
    pub name: SymbolId,
    pub name_span: Span,
    pub alias: Option<SymbolId>,
    pub alias_span: Option<Span>,
}

impl UsingPathSegment {
    fn from_ast(segment: &nia_ast::UsingHostSegment) -> Self {
        Self {
            kind: segment.kind,
            span: segment.span,
        }
    }
}

impl UsingSelector {
    fn from_ast(selector: &nia_ast::UsingSelector) -> Self {
        match selector {
            nia_ast::UsingSelector::Single(name) => Self::Single(UsingName::from_ast(name)),
            nia_ast::UsingSelector::Group(items) => {
                Self::Group(items.iter().map(UsingGroupItem::from_ast).collect())
            }
            nia_ast::UsingSelector::Wildcard { span } => Self::Wildcard { span: *span },
            nia_ast::UsingSelector::SelfName => Self::SelfName,
        }
    }
}

impl UsingGroupItem {
    fn from_ast(item: &nia_ast::UsingGroupItem) -> Self {
        match item {
            nia_ast::UsingGroupItem::Name(name) => Self::Name(UsingName::from_ast(name)),
            nia_ast::UsingGroupItem::Nested { host, selector } => Self::Nested {
                host: host.iter().map(UsingPathSegment::from_ast).collect(),
                selector: Box::new(UsingSelector::from_ast(selector)),
            },
        }
    }
}

impl UsingName {
    fn from_ast(name: &nia_ast::UsingName) -> Self {
        Self {
            name: name.name,
            name_span: name.name_span,
            alias: name.alias,
            alias_span: name.alias_span,
        }
    }
}

pub fn collect_module_defs(module_id: ModuleId, module: &Module) -> DefCollection {
    let item_tree = ModuleItemTree::from_module(module);
    collect_module_defs_from_item_tree(module_id, &item_tree)
}

pub fn collect_module_defs_from_item_tree(
    module_id: ModuleId,
    item_tree: &ModuleItemTree,
) -> DefCollection {
    Collector::new(module_id).collect(&item_tree.items)
}

pub fn collect_module_defs_from_item_tree_with_symbols(
    module_id: ModuleId,
    item_tree: &ModuleItemTree,
    symbols: &dyn SymbolText,
) -> DefCollection {
    Collector::new_with_symbols(module_id, Some(symbols)).collect(&item_tree.items)
}

pub fn collect_module_defs_from_item_tree_with_node_store_and_symbols(
    module_id: ModuleId,
    item_tree: &ModuleItemTree,
    node_store: &NodeStore,
    symbols: &dyn SymbolText,
) -> DefCollection {
    Collector::new_with_node_store_and_symbols(module_id, node_store, Some(symbols))
        .collect(&item_tree.items)
}

pub fn collect_module_defs_from_active_item_tree(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
) -> DefCollection {
    Collector::new(module_id).collect(&item_tree.items)
}

pub fn collect_module_defs_from_active_item_tree_with_symbols(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
    symbols: &dyn SymbolText,
) -> DefCollection {
    Collector::new_with_symbols(module_id, Some(symbols)).collect(&item_tree.items)
}

pub fn collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
    node_store: &NodeStore,
    symbols: &dyn SymbolText,
) -> DefCollection {
    Collector::new_with_node_store_and_symbols(module_id, node_store, Some(symbols))
        .collect(&item_tree.items)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefMap {
    defs: Vec<DefEntry>,
    by_id: HashMap<DefId, usize>,
    by_identity: HashMap<DefIdentity, DefId>,
}

impl DefMap {
    pub fn get(&self, id: DefId) -> Option<&Def> {
        self.by_id
            .get(&id)
            .and_then(|index| self.defs.get(*index))
            .map(|entry| &entry.def)
    }

    pub fn iter(&self) -> impl Iterator<Item = (DefId, &Def)> {
        self.defs.iter().map(|entry| (entry.id, &entry.def))
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    fn push(&mut self, identity: DefIdentity, def: Def) -> DefId {
        let id = DefId(stable_def_id(&identity));
        if let Some(index) = self.by_id.get(&id).copied() {
            let existing_identity = &self.defs[index].identity;
            if existing_identity != &identity {
                panic!(
                    "Nia ICE: stable definition id collision between `{}` and `{}`",
                    existing_identity.display(),
                    identity.display()
                );
            }
            panic!(
                "Nia ICE: duplicate stable definition identity `{}` reached DefMap insertion",
                identity.display()
            );
        }
        if let Some(existing) = self.by_identity.get(&identity).copied() {
            panic!(
                "Nia ICE: duplicate definition identity `{}` reached DefMap insertion as {:?}",
                identity.display(),
                existing
            );
        }
        let index = self.defs.len();
        self.defs.push(DefEntry { id, identity, def });
        self.by_identity
            .insert(self.defs[index].identity.clone(), id);
        self.by_id.insert(id, index);
        id
    }
}

fn stable_def_id(identity: &DefIdentity) -> u64 {
    let mut hash = StableDefHasher::new();
    for segment in &identity.segments {
        hash.segment(segment);
    }
    hash.finish()
}

struct StableDefHasher {
    value: u64,
}

impl StableDefHasher {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;

    fn new() -> Self {
        Self {
            value: Self::OFFSET,
        }
    }

    fn finish(self) -> u64 {
        self.value
    }

    fn segment(&mut self, segment: &DefIdentitySegment) {
        match segment {
            DefIdentitySegment::Top {
                namespace,
                kind,
                name,
            } => {
                self.bytes(b"top");
                self.namespace(*namespace);
                self.kind(*kind);
                self.symbol(*name);
            }
            DefIdentitySegment::Member { kind, name } => {
                self.bytes(b"member");
                self.kind(*kind);
                self.symbol(*name);
            }
            DefIdentitySegment::Extension {
                target,
                trait_ref,
                generics,
                where_clause,
            } => {
                self.bytes(b"extension");
                self.string(target);
                self.optional_string(trait_ref.as_deref());
                self.string_slice(generics);
                self.u64(where_clause.len() as u64);
                for (ty, bounds) in where_clause {
                    self.string(ty);
                    self.string_slice(bounds);
                }
            }
            DefIdentitySegment::Duplicate { ordinal } => {
                self.bytes(b"duplicate");
                self.u64(u64::from(*ordinal));
            }
        }
        self.bytes(b";");
    }

    fn namespace(&mut self, namespace: DefNamespace) {
        self.bytes(match namespace {
            DefNamespace::Module => b"module",
            DefNamespace::Type => b"type",
            DefNamespace::Value => b"value",
        });
    }

    fn kind(&mut self, kind: DefKind) {
        self.bytes(match kind {
            DefKind::Module => b"module",
            DefKind::Function => b"function",
            DefKind::Global => b"global",
            DefKind::Const => b"const",
            DefKind::Struct => b"struct",
            DefKind::StructField => b"struct_field",
            DefKind::Union => b"union",
            DefKind::UnionField => b"union_field",
            DefKind::Trait => b"trait",
            DefKind::TraitAssociatedType => b"trait_associated_type",
            DefKind::TraitMethod => b"trait_method",
            DefKind::Method => b"method",
            DefKind::Enum => b"enum",
            DefKind::EnumVariant => b"enum_variant",
            DefKind::EnumVariantField => b"enum_variant_field",
            DefKind::TypeAlias => b"type_alias",
        });
    }

    fn string_slice(&mut self, values: &[String]) {
        self.u64(values.len() as u64);
        for value in values {
            self.string(value);
        }
    }

    fn optional_string(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.bytes(b"some");
                self.string(value);
            }
            None => self.bytes(b"none"),
        }
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn symbol(&mut self, value: SymbolId) {
        self.u64(value.raw());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.value ^= u64::from(*byte);
            self.value = self.value.wrapping_mul(Self::PRIME);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DefEntry {
    id: DefId,
    identity: DefIdentity,
    def: Def,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DefIdentity {
    segments: Vec<DefIdentitySegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum DefIdentitySegment {
    Top {
        namespace: DefNamespace,
        kind: DefKind,
        name: SymbolId,
    },
    Member {
        kind: DefKind,
        name: SymbolId,
    },
    Extension {
        target: String,
        trait_ref: Option<String>,
        generics: Vec<String>,
        where_clause: Vec<(String, Vec<String>)>,
    },
    Duplicate {
        ordinal: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DefNamespace {
    Module,
    Type,
    Value,
}

impl DefIdentity {
    fn cached(def_id: DefId) -> Self {
        Self {
            segments: vec![DefIdentitySegment::Extension {
                target: format!("cached-def:{:016x}", def_id.0),
                trait_ref: None,
                generics: Vec::new(),
                where_clause: Vec::new(),
            }],
        }
    }

    fn top(namespace: DefNamespace, kind: DefKind, name: &SymbolId) -> Self {
        Self {
            segments: vec![DefIdentitySegment::Top {
                namespace,
                kind,
                name: *name,
            }],
        }
    }

    fn child(&self, kind: DefKind, name: &SymbolId) -> Self {
        let mut segments = self.segments.clone();
        segments.push(DefIdentitySegment::Member { kind, name: *name });
        Self { segments }
    }

    fn extension(extend: &ExtendItem) -> Self {
        Self {
            segments: vec![DefIdentitySegment::Extension {
                target: type_ref_identity(&extend.target),
                trait_ref: extend.trait_ref.as_ref().map(type_ref_identity),
                generics: generic_param_identities(&extend.generics),
                where_clause: where_clause_identity(&extend.where_clause),
            }],
        }
    }

    fn duplicate(&self, ordinal: u32) -> Self {
        let mut segments = self.segments.clone();
        segments.push(DefIdentitySegment::Duplicate { ordinal });
        Self { segments }
    }

    fn display(&self) -> String {
        self.segments
            .iter()
            .map(|segment| match segment {
                DefIdentitySegment::Top {
                    namespace,
                    kind,
                    name,
                } => {
                    format!("{namespace:?}:{kind:?}:{}", symbol_identity_key(*name))
                }
                DefIdentitySegment::Member { kind, name } => {
                    format!("{kind:?}:{}", symbol_identity_key(*name))
                }
                DefIdentitySegment::Extension {
                    target,
                    trait_ref,
                    generics,
                    where_clause,
                } => {
                    format!("extend:{trait_ref:?}:{target}:{generics:?}:{where_clause:?}")
                }
                DefIdentitySegment::Duplicate { ordinal } => format!("duplicate:{ordinal}"),
            })
            .collect::<Vec<_>>()
            .join("::")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Def {
    pub name: SymbolId,
    pub kind: DefKind,
    pub module_id: ModuleId,
    pub parent: Option<DefId>,
    pub generics: Vec<SymbolId>,
    pub generic_params: Vec<GenericParam>,
    pub visibility: Visibility,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    Module,
    Function,
    Global,
    Const,
    Struct,
    StructField,
    Union,
    UnionField,
    Trait,
    TraitAssociatedType,
    TraitMethod,
    Method,
    Enum,
    EnumVariant,
    EnumVariantField,
    TypeAlias,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleScope {
    pub modules: NameTable,
    pub types: NameTable,
    pub values: NameTable,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemberScope {
    pub fields: NameTable,
    pub values: NameTable,
    pub methods: NameTable,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnumScope {
    pub variants: NameTable,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NameTable {
    entries: SymbolMap<NameEntry>,
}

impl NameTable {
    pub fn get(&self, name: &SymbolId) -> Option<DefId> {
        self.entries.get(name).map(|entry| entry.def_id)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&SymbolId, DefId)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name, entry.def_id))
    }

    fn insert(&mut self, name: SymbolId, def_id: DefId, span: Span) -> Result<(), DuplicateName> {
        if let Some(existing) = self.entries.get(&name) {
            return Err(DuplicateName {
                name,
                first_span: existing.span,
                second_span: span,
            });
        }
        self.entries.insert(name, NameEntry { def_id, span });
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateName {
    pub name: SymbolId,
    pub first_span: Span,
    pub second_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefScopes {
    pub struct_members: HashMap<DefId, MemberScope>,
    pub union_members: HashMap<DefId, MemberScope>,
    pub enum_members: HashMap<DefId, EnumScope>,
}

#[derive(Debug, Clone, Default)]
pub struct DefNodeMap {
    nodes: NodeMap<DefId>,
}

#[derive(Debug)]
struct DefNodeMapBuilder {
    nodes: NodeMapBuilder<DefId>,
}

impl PartialEq for DefNodeMap {
    fn eq(&self, other: &Self) -> bool {
        self.nodes == other.nodes
    }
}

impl Eq for DefNodeMap {}

impl DefNodeMap {
    pub fn get(&self, node_key: &VersionedNodeKey) -> Option<DefId> {
        self.nodes.get(node_key).copied()
    }

    pub fn node_id(&self, node_key: &VersionedNodeKey) -> Option<NodeId> {
        self.nodes.node_id(node_key)
    }

    pub fn store_id(&self) -> NodeStoreId {
        self.nodes.store_id()
    }

    pub fn entries(&self) -> impl Iterator<Item = (VersionedNodeKey, DefId)> + '_ {
        self.nodes
            .iter()
            .map(|(node_key, def_id)| (node_key, *def_id))
    }
}

impl DefNodeMapBuilder {
    fn new(store: &NodeStore) -> Self {
        Self {
            nodes: NodeMap::builder(store),
        }
    }

    fn insert(&mut self, node_key: VersionedNodeKey, def_id: DefId) {
        self.nodes.insert(node_key, def_id);
    }

    fn finish(self) -> DefNodeMap {
        DefNodeMap {
            nodes: self.nodes.finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct NameEntry {
    def_id: DefId,
    span: Span,
}

struct MemberDefInput {
    identity: DefIdentity,
    parent: Option<DefId>,
    name: SymbolId,
    kind: DefKind,
    visibility: Visibility,
    span: Span,
    generics: Vec<GenericParam>,
}

struct Collector<'a> {
    module_id: ModuleId,
    symbols: Option<&'a dyn SymbolText>,
    defs: DefMap,
    module_scope: ModuleScope,
    struct_members: HashMap<DefId, MemberScope>,
    union_members: HashMap<DefId, MemberScope>,
    enum_members: HashMap<DefId, EnumScope>,
    def_nodes: DefNodeMapBuilder,
    module_usings: Vec<ModuleUsing>,
    diagnostics: Vec<Diagnostic>,
    duplicate_identities: HashMap<DefIdentity, u32>,
}

impl<'a> Collector<'a> {
    fn new(module_id: ModuleId) -> Self {
        Self::new_with_symbols(module_id, None)
    }

    fn new_with_symbols(module_id: ModuleId, symbols: Option<&'a dyn SymbolText>) -> Self {
        Self::new_with_node_store_and_symbols(module_id, &NodeStore::new(), symbols)
    }

    fn new_with_node_store_and_symbols(
        module_id: ModuleId,
        node_store: &NodeStore,
        symbols: Option<&'a dyn SymbolText>,
    ) -> Self {
        Self {
            module_id,
            symbols,
            defs: DefMap::default(),
            module_scope: ModuleScope::default(),
            struct_members: HashMap::new(),
            union_members: HashMap::new(),
            enum_members: HashMap::new(),
            def_nodes: DefNodeMapBuilder::new(node_store),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
            duplicate_identities: HashMap::new(),
        }
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols, symbol)
    }

    fn collect(mut self, items: &[ItemTreeNode]) -> DefCollection {
        for item in items {
            self.collect_item(item);
        }
        DefCollection {
            module_id: self.module_id,
            defs: self.defs,
            module_scope: self.module_scope,
            scopes: DefScopes {
                struct_members: self.struct_members,
                union_members: self.union_members,
                enum_members: self.enum_members,
            },
            def_nodes: self.def_nodes.finish(),
            module_usings: self.module_usings,
            diagnostics: self.diagnostics,
        }
    }

    fn collect_item(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Module(module) => {
                self.add_module_def(
                    module.name,
                    DefKind::Module,
                    item.visibility,
                    item.span,
                    item.node_key.clone(),
                );
            }
            ItemTreeNodeKind::Using(using) => {
                self.collect_using(item, using);
            }
            ItemTreeNodeKind::Struct(item_struct) => self.collect_struct(item, item_struct),
            ItemTreeNodeKind::Union(item_union) => self.collect_union(item, item_union),
            ItemTreeNodeKind::Trait(item_trait) => self.collect_trait(item, item_trait),
            ItemTreeNodeKind::Extend(extend) => self.collect_extend(item, extend),
            ItemTreeNodeKind::Enum(item_enum) => self.collect_enum(item, item_enum),
            ItemTreeNodeKind::TypeAlias(alias) => self.collect_type_alias(item, alias),
            ItemTreeNodeKind::Function(function) => {
                self.check_duplicate_generics(&function.generics, item.span);
                let function_id = self.add_value_def(
                    function.name,
                    DefKind::Function,
                    item.visibility,
                    item.span,
                    function.node_key.clone(),
                    function.generics.clone(),
                );
                let function_identity =
                    DefIdentity::top(DefNamespace::Value, DefKind::Function, &function.name);
                self.collect_function_local_static_bindings(
                    &function_identity,
                    function_id,
                    function,
                    item.visibility,
                );
            }
            ItemTreeNodeKind::Binding(binding) => {
                self.add_value_def(
                    binding.name,
                    if binding.is_const() {
                        DefKind::Const
                    } else {
                        DefKind::Global
                    },
                    item.visibility,
                    item.span,
                    binding.node_key.clone(),
                    Vec::new(),
                );
            }
        }
    }

    fn collect_using(&mut self, item: &ItemTreeNode, using: &UsingItem) {
        self.module_usings.push(ModuleUsing {
            visibility: item.visibility,
            span: item.span,
            host: using.host.iter().map(UsingPathSegment::from_ast).collect(),
            selector: UsingSelector::from_ast(&using.selector),
        });
    }

    fn collect_struct(&mut self, item: &ItemTreeNode, item_struct: &StructItem) {
        self.check_duplicate_generics(&item_struct.generics, item.span);
        let identity = DefIdentity::top(DefNamespace::Type, DefKind::Struct, &item_struct.name);
        let struct_id = self.add_type_def(
            item_struct.name,
            DefKind::Struct,
            item.visibility,
            item.span,
            item.node_key.clone(),
            item_struct.generics.clone(),
        );
        let mut members = MemberScope::default();
        for field in &item_struct.fields {
            let field_id = self.push_member_def(
                identity.child(DefKind::StructField, &field.name),
                Some(struct_id),
                field.name,
                DefKind::StructField,
                Visibility::Private,
                field.span,
            );
            self.def_nodes.insert(field.node_key.clone(), field_id);
            self.insert_member(
                &mut members.fields,
                field.name,
                field_id,
                field.span,
                "duplicate struct field",
            );
        }
        self.struct_members.insert(struct_id, members);
    }

    fn collect_union(&mut self, item: &ItemTreeNode, item_union: &UnionItem) {
        self.check_duplicate_generics(&item_union.generics, item.span);
        let identity = DefIdentity::top(DefNamespace::Type, DefKind::Union, &item_union.name);
        let union_id = self.add_type_def(
            item_union.name,
            DefKind::Union,
            item.visibility,
            item.span,
            item.node_key.clone(),
            item_union.generics.clone(),
        );
        let mut members = MemberScope::default();
        for field in &item_union.fields {
            let field_id = self.push_member_def(
                identity.child(DefKind::UnionField, &field.name),
                Some(union_id),
                field.name,
                DefKind::UnionField,
                Visibility::Private,
                field.span,
            );
            self.def_nodes.insert(field.node_key.clone(), field_id);
            self.insert_member(
                &mut members.fields,
                field.name,
                field_id,
                field.span,
                "duplicate union field",
            );
        }
        self.union_members.insert(union_id, members);
    }

    fn collect_extend(&mut self, _item: &ItemTreeNode, extend: &ExtendItem) {
        self.check_duplicate_generics(&extend.generics, extend.target.span);
        let identity = DefIdentity::extension(extend);
        let mut members = MemberScope::default();
        for associated_type in &extend.associated_types {
            self.collect_extend_associated_type(&identity, None, &mut members, associated_type);
        }
        for associated_value in &extend.associated_values {
            self.collect_extend_associated_value(&identity, None, &mut members, associated_value);
        }
        for method in &extend.methods {
            self.collect_method(&identity, None, &mut members, &method.function, method.vis);
        }
    }

    fn collect_trait(&mut self, item: &ItemTreeNode, item_trait: &nia_ast::TraitItem) {
        self.check_duplicate_generics(&item_trait.generics, item.span);
        let identity = DefIdentity::top(DefNamespace::Type, DefKind::Trait, &item_trait.name);
        let trait_id = self.add_type_def(
            item_trait.name,
            DefKind::Trait,
            item.visibility,
            item.span,
            item.node_key.clone(),
            item_trait.generics.clone(),
        );
        let mut members = MemberScope::default();
        for associated_type in &item_trait.associated_types {
            self.collect_trait_associated_type(
                &identity,
                Some(trait_id),
                &mut members,
                associated_type,
            );
        }
        for associated_value in &item_trait.associated_values {
            self.collect_trait_associated_value(
                &identity,
                Some(trait_id),
                &mut members,
                associated_value,
            );
        }
        for method in &item_trait.methods {
            self.collect_trait_method(&identity, Some(trait_id), &mut members, &method.function);
        }
        self.struct_members.insert(trait_id, members);
    }

    fn collect_trait_associated_type(
        &mut self,
        owner_identity: &DefIdentity,
        parent: Option<DefId>,
        members: &mut MemberScope,
        associated_type: &TraitAssociatedType,
    ) {
        let associated_type_id = self.push_member_def(
            owner_identity.child(DefKind::TraitAssociatedType, &associated_type.name),
            parent,
            associated_type.name,
            DefKind::TraitAssociatedType,
            Visibility::Public,
            associated_type.span,
        );
        self.def_nodes
            .insert(associated_type.node_key.clone(), associated_type_id);
        self.insert_member(
            &mut members.fields,
            associated_type.name,
            associated_type_id,
            associated_type.span,
            "duplicate trait associated type",
        );
    }

    fn collect_trait_associated_value(
        &mut self,
        owner_identity: &DefIdentity,
        parent: Option<DefId>,
        members: &mut MemberScope,
        associated_value: &TraitAssociatedValue,
    ) {
        let value_id = self.push_member_def(
            owner_identity.child(DefKind::Const, &associated_value.name),
            parent,
            associated_value.name,
            DefKind::Const,
            Visibility::Public,
            associated_value.span,
        );
        self.def_nodes
            .insert(associated_value.node_key.clone(), value_id);
        self.insert_member(
            &mut members.values,
            associated_value.name,
            value_id,
            associated_value.span,
            "duplicate trait associated const",
        );
    }

    fn collect_extend_associated_type(
        &mut self,
        owner_identity: &DefIdentity,
        parent: Option<DefId>,
        members: &mut MemberScope,
        associated_type: &ExtendAssociatedType,
    ) {
        let associated_type_id = self.push_member_def(
            owner_identity.child(DefKind::TraitAssociatedType, &associated_type.name),
            parent,
            associated_type.name,
            DefKind::TraitAssociatedType,
            Visibility::Private,
            associated_type.span,
        );
        self.def_nodes
            .insert(associated_type.node_key.clone(), associated_type_id);
        self.insert_member(
            &mut members.fields,
            associated_type.name,
            associated_type_id,
            associated_type.span,
            "duplicate associated type definition",
        );
    }

    fn collect_extend_associated_value(
        &mut self,
        owner_identity: &DefIdentity,
        parent: Option<DefId>,
        members: &mut MemberScope,
        associated_value: &ExtendAssociatedValue,
    ) {
        let binding = &associated_value.binding;
        let value_id = self.push_associated_value_def(
            owner_identity.child(DefKind::Const, &binding.name),
            parent,
            binding,
            associated_value.vis,
            associated_value.span,
        );
        self.def_nodes.insert(binding.node_key.clone(), value_id);
        self.insert_member(
            &mut members.values,
            binding.name,
            value_id,
            associated_value.span,
            "duplicate associated value definition",
        );
    }

    fn collect_trait_method(
        &mut self,
        owner_identity: &DefIdentity,
        parent: Option<DefId>,
        members: &mut MemberScope,
        method: &FunctionItem,
    ) {
        self.check_duplicate_generics(&method.generics, method.span);
        let method_id = self.push_member_def_with_generics(MemberDefInput {
            identity: owner_identity.child(DefKind::TraitMethod, &method.name),
            parent,
            name: method.name,
            kind: DefKind::TraitMethod,
            visibility: Visibility::Public,
            span: method.span,
            generics: method.generics.clone(),
        });
        self.def_nodes.insert(method.node_key.clone(), method_id);
        self.insert_member(
            &mut members.methods,
            method.name,
            method_id,
            method.span,
            "duplicate trait method",
        );
        self.collect_function_local_static_bindings(
            &owner_identity.child(DefKind::TraitMethod, &method.name),
            method_id,
            method,
            Visibility::Private,
        );
    }

    fn collect_method(
        &mut self,
        owner_identity: &DefIdentity,
        parent: Option<DefId>,
        members: &mut MemberScope,
        method: &FunctionItem,
        visibility: Visibility,
    ) {
        self.check_duplicate_generics(&method.generics, method.span);
        let method_id = self.push_member_def_with_generics(MemberDefInput {
            identity: owner_identity.child(DefKind::Method, &method.name),
            parent,
            name: method.name,
            kind: DefKind::Method,
            visibility,
            span: method.span,
            generics: method.generics.clone(),
        });
        self.def_nodes.insert(method.node_key.clone(), method_id);
        self.insert_member(
            &mut members.methods,
            method.name,
            method_id,
            method.span,
            "duplicate struct method",
        );
        self.collect_function_local_static_bindings(
            &owner_identity.child(DefKind::Method, &method.name),
            method_id,
            method,
            Visibility::Private,
        );
    }

    fn collect_function_local_static_bindings(
        &mut self,
        owner_identity: &DefIdentity,
        parent: DefId,
        function: &FunctionItem,
        visibility: Visibility,
    ) {
        let Some(body) = &function.body else {
            return;
        };
        self.collect_block_static_bindings(owner_identity, parent, body, visibility);
    }

    fn collect_block_static_bindings(
        &mut self,
        owner_identity: &DefIdentity,
        parent: DefId,
        block: &Block,
        visibility: Visibility,
    ) {
        for stmt in &block.stmts {
            match &stmt.kind {
                StmtKind::Static(binding) => {
                    let def_id = self.push_member_def(
                        owner_identity.child(DefKind::Global, &binding.name),
                        Some(parent),
                        binding.name,
                        DefKind::Global,
                        visibility,
                        stmt.span,
                    );
                    self.def_nodes.insert(binding.node_key.clone(), def_id);
                }
                StmtKind::ForIn(for_stmt) => {
                    self.collect_block_static_bindings(
                        owner_identity,
                        parent,
                        &for_stmt.body,
                        visibility,
                    );
                }
                StmtKind::While(while_stmt) => {
                    self.collect_block_static_bindings(
                        owner_identity,
                        parent,
                        &while_stmt.body,
                        visibility,
                    );
                }
                StmtKind::Loop(loop_stmt) => {
                    self.collect_block_static_bindings(
                        owner_identity,
                        parent,
                        &loop_stmt.body,
                        visibility,
                    );
                }
                StmtKind::Binding(_)
                | StmtKind::Using(_)
                | StmtKind::Expr(_)
                | StmtKind::Return(_)
                | StmtKind::Break
                | StmtKind::Continue
                | StmtKind::Defer(_) => {}
            }
        }
    }

    fn push_associated_value_def(
        &mut self,
        identity: DefIdentity,
        parent: Option<DefId>,
        binding: &BindingItem,
        visibility: Visibility,
        span: Span,
    ) -> DefId {
        self.push_member_def(
            identity,
            parent,
            binding.name,
            DefKind::Const,
            visibility,
            span,
        )
    }

    fn collect_enum(&mut self, item: &ItemTreeNode, item_enum: &EnumItem) {
        let identity = DefIdentity::top(DefNamespace::Type, DefKind::Enum, &item_enum.name);
        let enum_id = self.add_type_def(
            item_enum.name,
            DefKind::Enum,
            item.visibility,
            item.span,
            item.node_key.clone(),
            Vec::new(),
        );
        let mut members = EnumScope::default();
        for variant in &item_enum.variants {
            let variant_identity = identity.child(DefKind::EnumVariant, &variant.name);
            let variant_id = self.push_member_def(
                variant_identity.clone(),
                Some(enum_id),
                variant.name,
                DefKind::EnumVariant,
                Visibility::Public,
                variant.span,
            );
            self.def_nodes.insert(variant.node_key.clone(), variant_id);
            self.insert_member(
                &mut members.variants,
                variant.name,
                variant_id,
                variant.span,
                "duplicate enum variant",
            );
            if let nia_ast::EnumVariantPayload::Named(fields) = &variant.payload {
                let mut field_names = NameTable::default();
                for field in fields {
                    let field_id = self.push_member_def(
                        variant_identity.child(DefKind::EnumVariantField, &field.name),
                        Some(variant_id),
                        field.name,
                        DefKind::EnumVariantField,
                        Visibility::Private,
                        field.span,
                    );
                    self.def_nodes.insert(field.node_key.clone(), field_id);
                    self.insert_member(
                        &mut field_names,
                        field.name,
                        field_id,
                        field.span,
                        "duplicate enum variant field",
                    );
                }
            }
        }
        self.enum_members.insert(enum_id, members);
    }

    fn collect_type_alias(&mut self, item: &ItemTreeNode, alias: &TypeAliasItem) {
        self.check_duplicate_generics(&alias.generics, item.span);
        self.add_type_def(
            alias.name,
            DefKind::TypeAlias,
            item.visibility,
            item.span,
            item.node_key.clone(),
            alias.generics.clone(),
        );
    }

    fn add_module_def(
        &mut self,
        name: SymbolId,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        node_key: VersionedNodeKey,
    ) -> DefId {
        let def_id = self.push_top_def(
            DefIdentity::top(DefNamespace::Module, kind, &name),
            name,
            kind,
            visibility,
            span,
            Vec::new(),
        );
        self.def_nodes.insert(node_key, def_id);
        self.insert_top_module(name, def_id, span, "duplicate module name");
        def_id
    }

    fn add_type_def(
        &mut self,
        name: SymbolId,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        node_key: VersionedNodeKey,
        generics: Vec<GenericParam>,
    ) -> DefId {
        let identity = DefIdentity::top(DefNamespace::Type, kind, &name);
        let def_id = self.push_top_def(identity, name, kind, visibility, span, generics);
        self.def_nodes.insert(node_key, def_id);
        self.insert_top_type(name, def_id, span, "duplicate type definition");
        def_id
    }

    fn add_value_def(
        &mut self,
        name: SymbolId,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        node_key: VersionedNodeKey,
        generics: Vec<GenericParam>,
    ) -> DefId {
        let def_id = self.push_top_def(
            DefIdentity::top(DefNamespace::Value, kind, &name),
            name,
            kind,
            visibility,
            span,
            generics,
        );
        self.def_nodes.insert(node_key, def_id);
        self.insert_top_value(name, def_id, span, "duplicate value definition");
        def_id
    }

    fn push_top_def(
        &mut self,
        identity: DefIdentity,
        name: SymbolId,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        generics: Vec<GenericParam>,
    ) -> DefId {
        let generic_names = nia_ast::generic_param_names(&generics);
        self.push_def(
            identity,
            Def {
                name,
                kind,
                module_id: self.module_id,
                parent: None,
                generics: generic_names,
                generic_params: generics,
                visibility,
                span,
            },
        )
    }

    fn push_member_def(
        &mut self,
        identity: DefIdentity,
        parent: Option<DefId>,
        name: SymbolId,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
    ) -> DefId {
        self.push_member_def_with_generics(MemberDefInput {
            identity,
            parent,
            name,
            kind,
            visibility,
            span,
            generics: Vec::new(),
        })
    }

    fn push_member_def_with_generics(&mut self, input: MemberDefInput) -> DefId {
        let MemberDefInput {
            identity,
            parent,
            name,
            kind,
            visibility,
            span,
            generics,
        } = input;
        let generic_names = nia_ast::generic_param_names(&generics);
        self.push_def(
            identity,
            Def {
                name,
                kind,
                module_id: self.module_id,
                parent,
                generics: generic_names,
                generic_params: generics,
                visibility,
                span,
            },
        )
    }

    fn push_def(&mut self, identity: DefIdentity, def: Def) -> DefId {
        let identity = self.disambiguate_identity(identity);
        self.defs.push(identity, def)
    }

    fn disambiguate_identity(&mut self, identity: DefIdentity) -> DefIdentity {
        let ordinal = self
            .duplicate_identities
            .entry(identity.clone())
            .or_default();
        let resolved = if *ordinal == 0 {
            identity
        } else {
            identity.duplicate(*ordinal)
        };
        *ordinal += 1;
        resolved
    }

    fn insert_top_type(
        &mut self,
        name: SymbolId,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = self.module_scope.types.insert(name, def_id, span) {
            let name = self.symbol_name(duplicate.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::PARSE,
                duplicate.second_span,
                format!("{message}: `{name}`"),
            ));
        }
    }

    fn insert_top_module(
        &mut self,
        name: SymbolId,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = self.module_scope.modules.insert(name, def_id, span) {
            let name = self.symbol_name(duplicate.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::PARSE,
                duplicate.second_span,
                format!("{message}: `{name}`"),
            ));
        }
    }

    fn insert_top_value(
        &mut self,
        name: SymbolId,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = self.module_scope.values.insert(name, def_id, span) {
            let name = self.symbol_name(duplicate.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::PARSE,
                duplicate.second_span,
                format!("{message}: `{name}`"),
            ));
        }
    }

    fn insert_member(
        &mut self,
        table: &mut NameTable,
        name: SymbolId,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = table.insert(name, def_id, span) {
            let name = self.symbol_name(duplicate.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::PARSE,
                duplicate.second_span,
                format!("{message}: `{name}`"),
            ));
        }
    }

    fn check_duplicate_generics(&mut self, generics: &[GenericParam], span: Span) {
        let mut seen = HashSet::new();
        for generic in generics {
            if !seen.insert(&generic.name) {
                let name = self.symbol_name(generic.name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::PARSE,
                    span,
                    format!("duplicate generic parameter `{name}`"),
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
