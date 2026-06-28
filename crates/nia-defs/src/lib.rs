// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

mod extensions;
mod public_surface;

use nia_ast::{
    BindingItem, EnumItem, ExtendAssociatedType, ExtendAssociatedValue, ExtendItem, FunctionItem,
    Module, StructItem, TraitAssociatedType, TypeAliasItem, UnionItem, UsingItem,
    type_ref_identity, where_clause_identity,
};
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::{DefId, ModuleId, Visibility};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::VersionedNodeKey;
use nia_span::Span;

pub use extensions::{
    AssociatedTypeBindingSignature, ExtensionAssociatedValue, ExtensionAssociatedValues,
    ExtensionMethod, ExtensionMethods, VisibleExtensionAssociatedValue, VisibleExtensionMethod,
    VisibleExtensionMethods, VisibleExtensionTarget, WhereBoundSignature, WherePredicateSignature,
};
pub use public_surface::{
    ModulePublicSurface, ModuleUsingScope, PublicItem, PublicNamespace, PublicSource,
    PublicSurfaces, UsingEntry,
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

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleUsing {
    pub visibility: Visibility,
    pub span: Span,
    pub host: Vec<UsingPathSegment>,
    pub selector: UsingSelector,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingPathSegment {
    pub name: String,
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
    pub name: String,
    pub name_span: Span,
    pub alias: Option<String>,
    pub alias_span: Option<Span>,
}

impl UsingPathSegment {
    fn from_ast(segment: &nia_ast::UsingHostSegment) -> Self {
        Self {
            name: segment.name.clone(),
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
            name: name.name.clone(),
            name_span: name.name_span,
            alias: name.alias.clone(),
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

pub fn collect_module_defs_from_active_item_tree(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
) -> DefCollection {
    Collector::new(module_id).collect(&item_tree.items)
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
                self.string(name);
            }
            DefIdentitySegment::Member { kind, name } => {
                self.bytes(b"member");
                self.kind(*kind);
                self.string(name);
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
            DefKind::Comptime => b"comptime",
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
        name: String,
    },
    Member {
        kind: DefKind,
        name: String,
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
    fn top(namespace: DefNamespace, kind: DefKind, name: &str) -> Self {
        Self {
            segments: vec![DefIdentitySegment::Top {
                namespace,
                kind,
                name: name.to_string(),
            }],
        }
    }

    fn child(&self, kind: DefKind, name: &str) -> Self {
        let mut segments = self.segments.clone();
        segments.push(DefIdentitySegment::Member {
            kind,
            name: name.to_string(),
        });
        Self { segments }
    }

    fn extension(extend: &ExtendItem) -> Self {
        Self {
            segments: vec![DefIdentitySegment::Extension {
                target: type_ref_identity(&extend.target),
                trait_ref: extend.trait_ref.as_ref().map(type_ref_identity),
                generics: extend.generics.clone(),
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
                    format!("{namespace:?}:{kind:?}:{name}")
                }
                DefIdentitySegment::Member { kind, name } => {
                    format!("{kind:?}:{name}")
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
    pub name: String,
    pub kind: DefKind,
    pub module_id: ModuleId,
    pub parent: Option<DefId>,
    pub generics: Vec<String>,
    pub visibility: Visibility,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    Module,
    Function,
    Global,
    Comptime,
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
    entries: HashMap<String, NameEntry>,
}

impl NameTable {
    pub fn get(&self, name: &str) -> Option<DefId> {
        self.entries.get(name).map(|entry| entry.def_id)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, DefId)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name.as_str(), entry.def_id))
    }

    fn insert(&mut self, name: String, def_id: DefId, span: Span) -> Result<(), DuplicateName> {
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
    pub name: String,
    pub first_span: Span,
    pub second_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefScopes {
    pub struct_members: HashMap<DefId, MemberScope>,
    pub union_members: HashMap<DefId, MemberScope>,
    pub enum_members: HashMap<DefId, EnumScope>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefNodeMap {
    nodes: HashMap<VersionedNodeKey, DefId>,
}

impl DefNodeMap {
    pub fn get(&self, node_key: &VersionedNodeKey) -> Option<DefId> {
        self.nodes.get(node_key).copied()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&VersionedNodeKey, DefId)> + '_ {
        self.nodes
            .iter()
            .map(|(node_key, def_id)| (node_key, *def_id))
    }

    fn insert(&mut self, node_key: VersionedNodeKey, def_id: DefId) {
        self.nodes.insert(node_key, def_id);
    }
}

#[derive(Debug, Clone, PartialEq)]
struct NameEntry {
    def_id: DefId,
    span: Span,
}

struct Collector {
    module_id: ModuleId,
    defs: DefMap,
    module_scope: ModuleScope,
    struct_members: HashMap<DefId, MemberScope>,
    union_members: HashMap<DefId, MemberScope>,
    enum_members: HashMap<DefId, EnumScope>,
    def_nodes: DefNodeMap,
    module_usings: Vec<ModuleUsing>,
    diagnostics: Vec<Diagnostic>,
    duplicate_identities: HashMap<DefIdentity, u32>,
}

impl Collector {
    fn new(module_id: ModuleId) -> Self {
        Self {
            module_id,
            defs: DefMap::default(),
            module_scope: ModuleScope::default(),
            struct_members: HashMap::new(),
            union_members: HashMap::new(),
            enum_members: HashMap::new(),
            def_nodes: DefNodeMap::default(),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
            duplicate_identities: HashMap::new(),
        }
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
            def_nodes: self.def_nodes,
            module_usings: self.module_usings,
            diagnostics: self.diagnostics,
        }
    }

    fn collect_item(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Module(module) => {
                self.add_module_def(
                    module.name.clone(),
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
                self.add_value_def(
                    function.name.clone(),
                    DefKind::Function,
                    item.visibility,
                    item.span,
                    function.node_key.clone(),
                    function.generics.clone(),
                );
            }
            ItemTreeNodeKind::Binding(binding) => {
                self.add_value_def(
                    binding.name.clone(),
                    if binding.is_comptime {
                        DefKind::Comptime
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
            identity.clone(),
            item_struct.name.clone(),
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
                field.name.clone(),
                DefKind::StructField,
                Visibility::Private,
                field.span,
            );
            self.def_nodes.insert(field.node_key.clone(), field_id);
            self.insert_member(
                &mut members.fields,
                field.name.clone(),
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
            identity.clone(),
            item_union.name.clone(),
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
                field.name.clone(),
                DefKind::UnionField,
                Visibility::Private,
                field.span,
            );
            self.def_nodes.insert(field.node_key.clone(), field_id);
            self.insert_member(
                &mut members.fields,
                field.name.clone(),
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
            identity.clone(),
            item_trait.name.clone(),
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
            associated_type.name.clone(),
            DefKind::TraitAssociatedType,
            Visibility::Public,
            associated_type.span,
        );
        self.def_nodes
            .insert(associated_type.node_key.clone(), associated_type_id);
        self.insert_member(
            &mut members.fields,
            associated_type.name.clone(),
            associated_type_id,
            associated_type.span,
            "duplicate trait associated type",
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
            associated_type.name.clone(),
            DefKind::TraitAssociatedType,
            Visibility::Private,
            associated_type.span,
        );
        self.def_nodes
            .insert(associated_type.node_key.clone(), associated_type_id);
        self.insert_member(
            &mut members.fields,
            associated_type.name.clone(),
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
            owner_identity.child(DefKind::Comptime, &binding.name),
            parent,
            binding,
            associated_value.vis,
            associated_value.span,
        );
        self.def_nodes.insert(binding.node_key.clone(), value_id);
        self.insert_member(
            &mut members.values,
            binding.name.clone(),
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
        let method_id = self.push_member_def_with_generics(
            owner_identity.child(DefKind::TraitMethod, &method.name),
            parent,
            method.name.clone(),
            DefKind::TraitMethod,
            Visibility::Public,
            method.span,
            method.generics.clone(),
        );
        self.def_nodes.insert(method.node_key.clone(), method_id);
        self.insert_member(
            &mut members.methods,
            method.name.clone(),
            method_id,
            method.span,
            "duplicate trait method",
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
        let method_id = self.push_member_def_with_generics(
            owner_identity.child(DefKind::Method, &method.name),
            parent,
            method.name.clone(),
            DefKind::Method,
            visibility,
            method.span,
            method.generics.clone(),
        );
        self.def_nodes.insert(method.node_key.clone(), method_id);
        self.insert_member(
            &mut members.methods,
            method.name.clone(),
            method_id,
            method.span,
            "duplicate struct method",
        );
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
            binding.name.clone(),
            DefKind::Comptime,
            visibility,
            span,
        )
    }

    fn collect_enum(&mut self, item: &ItemTreeNode, item_enum: &EnumItem) {
        let identity = DefIdentity::top(DefNamespace::Type, DefKind::Enum, &item_enum.name);
        let enum_id = self.add_type_def(
            identity.clone(),
            item_enum.name.clone(),
            DefKind::Enum,
            item.visibility,
            item.span,
            item.node_key.clone(),
            Vec::new(),
        );
        let mut members = EnumScope::default();
        for variant in &item_enum.variants {
            let variant_id = self.push_member_def(
                identity.child(DefKind::EnumVariant, &variant.name),
                Some(enum_id),
                variant.name.clone(),
                DefKind::EnumVariant,
                Visibility::Public,
                variant.span,
            );
            self.def_nodes.insert(variant.node_key.clone(), variant_id);
            self.insert_member(
                &mut members.variants,
                variant.name.clone(),
                variant_id,
                variant.span,
                "duplicate enum variant",
            );
        }
        self.enum_members.insert(enum_id, members);
    }

    fn collect_type_alias(&mut self, item: &ItemTreeNode, alias: &TypeAliasItem) {
        self.check_duplicate_generics(&alias.generics, item.span);
        self.add_type_def(
            DefIdentity::top(DefNamespace::Type, DefKind::TypeAlias, &alias.name),
            alias.name.clone(),
            DefKind::TypeAlias,
            item.visibility,
            item.span,
            item.node_key.clone(),
            alias.generics.clone(),
        );
    }

    fn add_module_def(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        node_key: VersionedNodeKey,
    ) -> DefId {
        let def_id = self.push_top_def(
            DefIdentity::top(DefNamespace::Module, kind, &name),
            name.clone(),
            kind,
            visibility,
            span,
            Vec::new(),
        );
        self.def_nodes.insert(node_key, def_id);
        Self::insert_top(
            &mut self.module_scope.modules,
            &mut self.diagnostics,
            name,
            def_id,
            span,
            "duplicate module name",
        );
        def_id
    }

    fn add_type_def(
        &mut self,
        identity: DefIdentity,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        node_key: VersionedNodeKey,
        generics: Vec<String>,
    ) -> DefId {
        let def_id = self.push_top_def(identity, name.clone(), kind, visibility, span, generics);
        self.def_nodes.insert(node_key, def_id);
        Self::insert_top(
            &mut self.module_scope.types,
            &mut self.diagnostics,
            name,
            def_id,
            span,
            "duplicate type definition",
        );
        def_id
    }

    fn add_value_def(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        node_key: VersionedNodeKey,
        generics: Vec<String>,
    ) -> DefId {
        let def_id = self.push_top_def(
            DefIdentity::top(DefNamespace::Value, kind, &name),
            name.clone(),
            kind,
            visibility,
            span,
            generics,
        );
        self.def_nodes.insert(node_key, def_id);
        Self::insert_top(
            &mut self.module_scope.values,
            &mut self.diagnostics,
            name,
            def_id,
            span,
            "duplicate value definition",
        );
        def_id
    }

    fn push_top_def(
        &mut self,
        identity: DefIdentity,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        generics: Vec<String>,
    ) -> DefId {
        self.push_def(
            identity,
            Def {
                name,
                kind,
                module_id: self.module_id,
                parent: None,
                generics,
                visibility,
                span,
            },
        )
    }

    fn push_member_def(
        &mut self,
        identity: DefIdentity,
        parent: Option<DefId>,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
    ) -> DefId {
        self.push_member_def_with_generics(
            identity,
            parent,
            name,
            kind,
            visibility,
            span,
            Vec::new(),
        )
    }

    fn push_member_def_with_generics(
        &mut self,
        identity: DefIdentity,
        parent: Option<DefId>,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        generics: Vec<String>,
    ) -> DefId {
        self.push_def(
            identity,
            Def {
                name,
                kind,
                module_id: self.module_id,
                parent,
                generics,
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

    fn insert_top(
        table: &mut NameTable,
        diagnostics: &mut Vec<Diagnostic>,
        name: String,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = table.insert(name, def_id, span) {
            diagnostics.push(Diagnostic::user_error_at(
                codes::PARSE,
                duplicate.second_span,
                format!("{message}: `{}`", duplicate.name),
            ));
        }
    }

    fn insert_member(
        &mut self,
        table: &mut NameTable,
        name: String,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = table.insert(name, def_id, span) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::PARSE,
                duplicate.second_span,
                format!("{message}: `{}`", duplicate.name),
            ));
        }
    }

    fn check_duplicate_generics(&mut self, generics: &[String], span: Span) {
        let mut seen = HashSet::new();
        for generic in generics {
            if !seen.insert(generic) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::PARSE,
                    span,
                    format!("duplicate generic parameter `{generic}`"),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_diagnostic::DiagnosticCategory;
    use nia_parser::parse_module;

    #[test]
    fn collects_top_level_defs_into_separate_namespaces() {
        let (module, errors) = parse_module(
            r#"
module math;
using entry::math;
struct Point { x: i32, y: i32 }
enum Color { Red, Green }
type Byte = u8;
fn Point() i32 { 0 }
let mut counter = 0;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        assert!(
            collection.diagnostics.is_empty(),
            "{:?}",
            collection.diagnostics
        );
        assert!(collection.module_scope.modules.get("math").is_some());
        assert!(collection.module_scope.types.get("Point").is_some());
        assert!(collection.module_scope.types.get("Color").is_some());
        assert!(collection.module_scope.types.get("Byte").is_some());
        assert!(collection.module_scope.values.get("Point").is_some());
        assert!(collection.module_scope.values.get("counter").is_some());
        assert!(collection.def_nodes.entries().count() >= collection.defs.len());
    }

    #[test]
    fn reports_duplicates_per_namespace() {
        let (module, errors) = parse_module(
            r#"
struct Thing { a: i32, a: i32 }
struct Thing {}
fn f() {}
fn f() {}
enum E { A, A }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        assert_eq!(collection.diagnostics.len(), 4);
        assert!(collection.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_str() == "E0101"
                && diagnostic.category == DiagnosticCategory::User
                && diagnostic
                    .primary_message()
                    .is_some_and(|message| message.contains("duplicate type definition"))
        }));
        assert!(collection.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_str() == "E0101"
                && diagnostic.category == DiagnosticCategory::User
                && diagnostic
                    .primary_message()
                    .is_some_and(|message| message.contains("duplicate value definition"))
        }));
        assert!(collection.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_str() == "E0101"
                && diagnostic.category == DiagnosticCategory::User
                && diagnostic
                    .primary_message()
                    .is_some_and(|message| message.contains("duplicate struct field"))
        }));
        assert!(collection.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_str() == "E0101"
                && diagnostic.category == DiagnosticCategory::User
                && diagnostic
                    .primary_message()
                    .is_some_and(|message| message.contains("duplicate enum variant"))
        }));
    }

    #[test]
    fn reports_duplicate_generic_parameters() {
        let (module, errors) = parse_module(
            r#"
struct Box[T, T] { value: T }
type Alias[T, T] = T;
fn id[T, T](x: T) T { x }
struct Methods[T] {
    value: T,
}

extend[T, T] Methods[T] {
    fn get[U, U](self) T { self.value }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        let duplicate_count = collection
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code.as_str() == "E0101"
                    && diagnostic.category == DiagnosticCategory::User
                    && diagnostic
                        .primary_message()
                        .is_some_and(|message| message.contains("duplicate generic parameter"))
            })
            .count();
        assert_eq!(duplicate_count, 5, "{:?}", collection.diagnostics);
    }

    #[test]
    fn maps_top_level_bindings_by_binding_node_key() {
        let (module, errors) = parse_module(
            r#"
let global: i32 = 1;
comptime answer: i32 = 42;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        assert!(
            collection.diagnostics.is_empty(),
            "{:?}",
            collection.diagnostics
        );

        for item in &module.items {
            let nia_ast::ItemKind::Binding(binding) = &item.kind else {
                continue;
            };
            let expected_kind = if binding.is_comptime {
                DefKind::Comptime
            } else {
                DefKind::Global
            };
            let def_id = collection
                .def_nodes
                .get(&binding.node_key)
                .expect("binding node key should map to its definition");
            let def = collection
                .defs
                .get(def_id)
                .expect("binding definition should exist");
            assert_eq!(def.kind, expected_kind);
            assert_eq!(def.name, binding.name);
        }
    }

    #[test]
    fn definition_ids_are_stable_across_unrelated_insertions() {
        let before = collect_ok(
            r#"
pub struct Point { x: i32, y: i32 }
pub enum Color { Red, Green }
trait Show {
    fn show(self) i32;
}
extend Point {
    fn len(self) i32 { 0 }
}
pub fn main() i32 { 0 }
"#,
        );
        let after = collect_ok(
            r#"
fn helper() i32 { 1 }
pub struct Point { x: i32, y: i32 }
pub enum Color { Red, Green }
trait Show {
    fn show(self) i32;
}
extend Point {
    fn len(self) i32 { 0 }
}
pub fn main() i32 { 0 }
"#,
        );

        assert_eq!(top_type_id(&before, "Point"), top_type_id(&after, "Point"));
        assert_eq!(top_value_id(&before, "main"), top_value_id(&after, "main"));
        assert_eq!(
            member_id(&before, top_type_id(&before, "Point"), "x"),
            member_id(&after, top_type_id(&after, "Point"), "x")
        );
        assert_eq!(
            enum_variant_id(&before, top_type_id(&before, "Color"), "Green"),
            enum_variant_id(&after, top_type_id(&after, "Color"), "Green")
        );
        assert_eq!(
            member_id(&before, top_type_id(&before, "Show"), "show"),
            member_id(&after, top_type_id(&after, "Show"), "show")
        );
        assert_eq!(
            extension_method_id(&before, "len"),
            extension_method_id(&after, "len")
        );
    }

    #[test]
    fn extension_definition_ids_ignore_type_formatting() {
        let before = collect_ok(
            r#"
struct Box[T] { value: T }
extend[T] &Box[T] {
    fn get(self) T { self.value }
}
"#,
        );
        let after = collect_ok(
            r#"
struct Box[T] { value: T }
extend[T] & Box[ T ] {
    fn get(self) T { self.value }
}
"#,
        );

        assert_eq!(
            extension_method_id(&before, "get"),
            extension_method_id(&after, "get")
        );
    }

    fn collect_ok(source: &str) -> DefCollection {
        let (module, errors) = parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        assert!(
            collection.diagnostics.is_empty(),
            "{:?}",
            collection.diagnostics
        );
        collection
    }

    fn top_type_id(defs: &DefCollection, name: &str) -> DefId {
        defs.module_scope
            .types
            .get(name)
            .unwrap_or_else(|| panic!("missing top-level type `{name}`"))
    }

    fn top_value_id(defs: &DefCollection, name: &str) -> DefId {
        defs.module_scope
            .values
            .get(name)
            .unwrap_or_else(|| panic!("missing top-level value `{name}`"))
    }

    fn member_id(defs: &DefCollection, owner: DefId, name: &str) -> DefId {
        defs.scopes
            .struct_members
            .get(&owner)
            .and_then(|members| {
                members
                    .fields
                    .get(name)
                    .or_else(|| members.methods.get(name))
                    .or_else(|| members.values.get(name))
            })
            .unwrap_or_else(|| panic!("missing member `{name}`"))
    }

    fn enum_variant_id(defs: &DefCollection, owner: DefId, name: &str) -> DefId {
        defs.scopes
            .enum_members
            .get(&owner)
            .and_then(|members| members.variants.get(name))
            .unwrap_or_else(|| panic!("missing enum variant `{name}`"))
    }

    fn extension_method_id(defs: &DefCollection, name: &str) -> DefId {
        defs.defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == DefKind::Method && def.parent.is_none() && def.name == name)
                    .then_some(def_id)
            })
            .unwrap_or_else(|| panic!("missing extension method `{name}`"))
    }
}
