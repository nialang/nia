// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    Attribute, AttributeKind, BindingItem, Block, EnumItem, ExtendItem, FunctionItem, GenericParam,
    GenericParamKind, Module, Param, StmtKind, StructItem, TraitItem, TypeAliasItem, TypeRef,
    UnionItem, WhereClause, generic_param_identities, type_ref_identity, where_clause_identity,
};
pub use nia_defs::{AssociatedTypeBindingSignature, WhereBoundSignature, WherePredicateSignature};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{
    BuiltinConstValue, BuiltinType, BuiltinTypeAnchor, InternedTyId, ReceiverKind, TraitImplId,
    Visibility,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_symbol::{
    SymbolId, SymbolText, known, symbol_identity_key, symbol_text_from_optional_resolver,
};
use nia_ty::{PrimitiveTy, TraitId, TyKind, TypeStore, TypeStoreAppend};
use nia_type_lower::TypeLowering;

#[derive(Debug, Clone, PartialEq)]
pub struct ItemSignatures {
    pub functions: HashMap<DefId, FunctionSignature>,
    pub structs: HashMap<DefId, StructSignature>,
    pub unions: HashMap<DefId, UnionSignature>,
    pub traits: HashMap<DefId, TraitSignature>,
    pub trait_impls: Vec<TraitImplSignature>,
    pub enums: HashMap<DefId, EnumSignature>,
    pub type_aliases: HashMap<DefId, TypeAliasSignature>,
    pub globals: HashMap<DefId, GlobalSignature>,
    pub consts: HashMap<DefId, ConstSignature>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ItemSignatures {
    pub fn type_roots(&self) -> Vec<InternedTyId> {
        let mut roots = Vec::new();
        for signature in self.functions.values() {
            collect_function_type_roots(signature, &mut roots);
        }
        for signature in self.structs.values() {
            collect_where_type_roots(&signature.where_predicates, &mut roots);
            roots.extend(signature.fields.iter().map(|field| field.ty));
        }
        for signature in self.unions.values() {
            collect_where_type_roots(&signature.where_predicates, &mut roots);
            roots.extend(signature.fields.iter().map(|field| field.ty));
        }
        for signature in self.traits.values() {
            collect_where_type_roots(&signature.where_predicates, &mut roots);
            roots.extend(signature.supertraits.iter().map(|supertrait| supertrait.ty));
            roots.extend(signature.associated_values.iter().map(|value| value.ty));
            for method in &signature.methods {
                collect_function_type_roots(&method.signature, &mut roots);
            }
        }
        for signature in &self.trait_impls {
            roots.push(signature.target_ty);
            roots.extend(signature.trait_ty);
            collect_where_type_roots(&signature.where_predicates, &mut roots);
            roots.extend(signature.associated_types.iter().map(|binding| binding.ty));
        }
        roots.extend(self.enums.values().map(|signature| signature.backing_type));
        roots.extend(self.type_aliases.values().map(|signature| signature.target));
        roots.extend(
            self.globals
                .values()
                .filter_map(|signature| signature.explicit_type),
        );
        roots.extend(
            self.consts
                .values()
                .filter_map(|signature| signature.explicit_type),
        );
        roots.sort_unstable();
        roots.dedup();
        roots
    }
}

fn collect_function_type_roots(signature: &FunctionSignature, roots: &mut Vec<InternedTyId>) {
    roots.extend(signature.generic_params.iter().filter_map(|param| {
        let GenericParamSignatureKind::Const { ty } = param.kind else {
            return None;
        };
        Some(ty)
    }));
    collect_where_type_roots(&signature.where_predicates, roots);
    roots.extend(signature.params.iter().map(|param| param.ty));
    roots.push(signature.return_type);
}

fn collect_where_type_roots(predicates: &[WherePredicateSignature], roots: &mut Vec<InternedTyId>) {
    for predicate in predicates {
        roots.push(predicate.ty);
        for bound in &predicate.bounds {
            roots.push(bound.trait_ty);
            roots.extend(
                bound
                    .associated_type_bindings
                    .iter()
                    .map(|binding| binding.ty),
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramFunctionSignature {
    pub name: SymbolId,
    pub signature: FunctionSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramGlobalSignature {
    pub signature: GlobalSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramConstSignature {
    pub signature: ConstSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramStructSignature {
    pub signature: StructSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramUnionSignature {
    pub signature: UnionSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramEnumSignature {
    pub signature: EnumSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTraitSignature {
    pub signature: TraitSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTypeAliasSignature {
    pub signature: TypeAliasSignature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTraitImplSignature {
    pub module_id: nia_ids::ModuleId,
    pub impl_id: TraitImplId,
    pub builtin: Option<String>,
    pub generics: Vec<SymbolId>,
    pub target_ty: InternedTyId,
    pub trait_id: nia_ty::TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    pub associated_values: Vec<TraitImplAssociatedValueSignature>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramTraitImplIndex {
    by_trait: HashMap<TraitId, Vec<usize>>,
}

impl ProgramTraitImplIndex {
    pub fn new(trait_impls: &[ProgramTraitImplSignature]) -> Self {
        let mut by_trait = HashMap::<TraitId, Vec<usize>>::new();
        for (index, impl_signature) in trait_impls.iter().enumerate() {
            by_trait
                .entry(impl_signature.trait_id)
                .or_default()
                .push(index);
        }
        Self { by_trait }
    }

    pub fn indexes_for_trait(&self, trait_id: TraitId) -> &[usize] {
        self.by_trait
            .get(&trait_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.by_trait.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    pub name: SymbolId,
    pub generics: Vec<SymbolId>,
    pub generic_params: Vec<GenericParamSignature>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub params: Vec<ParamSignature>,
    pub return_type: InternedTyId,
    pub is_extern: bool,
    pub is_const: bool,
    pub is_variadic: bool,
    pub attributes: Vec<FunctionAttribute>,
    pub has_body: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParamSignature {
    pub name: SymbolId,
    pub kind: GenericParamSignatureKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericParamSignatureKind {
    Type,
    Const { ty: InternedTyId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAttribute {
    Naked,
    Builtin(BuiltinFunction),
}

pub use nia_ids::{BuiltinFunction, BuiltinTrait};

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSignature {
    pub name: Option<SymbolId>,
    pub receiver: Option<ReceiverKind>,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructSignature {
    pub generics: Vec<SymbolId>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub fields: Vec<FieldSignature>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionSignature {
    pub generics: Vec<SymbolId>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub fields: Vec<FieldSignature>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitSignature {
    pub generics: Vec<SymbolId>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub supertraits: Vec<TraitSupertraitSignature>,
    pub associated_types: Vec<TraitAssociatedTypeSignature>,
    pub associated_values: Vec<TraitAssociatedValueSignature>,
    pub methods: Vec<TraitMethodSignature>,
    pub builtin: Option<BuiltinTrait>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitSupertraitSignature {
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedTypeSignature {
    pub def_id: DefId,
    pub name: SymbolId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedValueSignature {
    pub def_id: DefId,
    pub name: SymbolId,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethodSignature {
    pub def_id: DefId,
    pub name: SymbolId,
    pub signature: FunctionSignature,
    pub has_default: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplSignature {
    pub impl_id: TraitImplId,
    pub builtin: Option<String>,
    pub generics: Vec<SymbolId>,
    pub target_ty: InternedTyId,
    pub trait_ty: Option<InternedTyId>,
    pub trait_span: Option<Span>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    pub associated_values: Vec<TraitImplAssociatedValueSignature>,
    pub methods: Vec<TraitImplMethodSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitImplIdentity {
    target: String,
    trait_ref: Option<String>,
    generics: Vec<String>,
    where_clause: Vec<(String, Vec<String>)>,
    duplicate_ordinal: Option<u32>,
}

impl TraitImplIdentity {
    fn from_extend(extend: &ExtendItem) -> Self {
        Self {
            target: type_ref_identity(&extend.target),
            trait_ref: extend.trait_ref.as_ref().map(type_ref_identity),
            generics: generic_param_identities(&extend.generics),
            where_clause: where_clause_identity(&extend.where_clause),
            duplicate_ordinal: None,
        }
    }

    fn duplicate(mut self, ordinal: u32) -> Self {
        self.duplicate_ordinal = Some(ordinal);
        self
    }
}

fn stable_trait_impl_id(identity: &TraitImplIdentity) -> u64 {
    let mut hash = StableTraitImplHasher::new();
    hash.bytes(b"trait_impl");
    hash.string(&identity.target);
    hash.optional_string(identity.trait_ref.as_deref());
    hash.string_slice(&identity.generics);
    hash.u64(identity.where_clause.len() as u64);
    for (ty, bounds) in &identity.where_clause {
        hash.string(ty);
        hash.string_slice(bounds);
    }
    match identity.duplicate_ordinal {
        Some(ordinal) => {
            hash.bytes(b"duplicate");
            hash.u64(u64::from(ordinal));
        }
        None => hash.bytes(b"primary"),
    }
    hash.finish()
}

fn generic_signature_names(generics: &[GenericParam]) -> Vec<SymbolId> {
    generics.iter().map(|generic| generic.name).collect()
}

fn symbol_debug_text(symbol: SymbolId) -> String {
    if let Some((_, text)) = known::WELL_KNOWN
        .iter()
        .find(|(known_symbol, _)| *known_symbol == symbol)
    {
        return (*text).to_string();
    }
    symbol_identity_key(symbol)
}

fn symbol_debug_text_with_symbols(symbols: Option<&dyn SymbolText>, symbol: SymbolId) -> String {
    match symbols {
        Some(symbols) => symbol_text_from_optional_resolver(Some(symbols), symbol),
        None => symbol_debug_text(symbol),
    }
}

fn attribute_path_text_with_symbols(path: &[SymbolId], symbols: Option<&dyn SymbolText>) -> String {
    path.iter()
        .map(|name| symbol_debug_text_with_symbols(symbols, *name))
        .collect::<Vec<_>>()
        .join(".")
}

fn builtin_const_item_symbol(builtin: BuiltinConstValue) -> SymbolId {
    match builtin {
        BuiltinConstValue::TargetArch => known::ARCH,
        BuiltinConstValue::TargetVendor => known::VENDOR,
        BuiltinConstValue::TargetOs => known::OS,
        BuiltinConstValue::TargetEnv => known::ENV,
        BuiltinConstValue::TargetAbi => known::ABI,
        BuiltinConstValue::TargetEndian => known::ENDIAN,
        BuiltinConstValue::TargetPointerWidth => known::POINTER_WIDTH,
    }
}

struct StableTraitImplHasher {
    value: u64,
}

impl StableTraitImplHasher {
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
pub struct TraitImplAssociatedTypeSignature {
    pub name: SymbolId,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplAssociatedValueSignature {
    pub def_id: DefId,
    pub name: SymbolId,
    pub visibility: Visibility,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplMethodSignature {
    pub def_id: DefId,
    pub name: SymbolId,
    pub visibility: Visibility,
    pub span: Span,
}

impl UnionSignature {
    pub fn as_struct_like(&self) -> StructSignature {
        StructSignature {
            generics: self.generics.clone(),
            where_predicates: self.where_predicates.clone(),
            fields: self.fields.clone(),
            is_extern: self.is_extern,
            span: self.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldSignature {
    pub def_id: DefId,
    pub name: SymbolId,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumSignature {
    pub backing_type: InternedTyId,
    pub is_open: bool,
    pub variants: Vec<EnumVariantSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantSignature {
    pub def_id: DefId,
    pub name: SymbolId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasSignature {
    pub generics: Vec<SymbolId>,
    pub target: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalSignature {
    pub explicit_type: Option<InternedTyId>,
    pub is_mutable: bool,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstSignature {
    pub explicit_type: Option<InternedTyId>,
    pub builtin: Option<BuiltinConstValue>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltinTypeDeclaration {
    Opaque(BuiltinType),
    Primitive(BuiltinTypeAnchor),
}

impl BuiltinTypeDeclaration {
    fn from_name(name: &str) -> Option<Self> {
        BuiltinType::from_name(name)
            .map(Self::Opaque)
            .or_else(|| BuiltinTypeAnchor::from_name(name).map(Self::Primitive))
    }
}

fn builtin_type_anchor_primitive(anchor: BuiltinTypeAnchor) -> PrimitiveTy {
    match anchor {
        BuiltinTypeAnchor::I8 => PrimitiveTy::I8,
        BuiltinTypeAnchor::I16 => PrimitiveTy::I16,
        BuiltinTypeAnchor::I32 => PrimitiveTy::I32,
        BuiltinTypeAnchor::I64 => PrimitiveTy::I64,
        BuiltinTypeAnchor::I128 => PrimitiveTy::I128,
        BuiltinTypeAnchor::Isize => PrimitiveTy::Isize,
        BuiltinTypeAnchor::U8 => PrimitiveTy::U8,
        BuiltinTypeAnchor::U16 => PrimitiveTy::U16,
        BuiltinTypeAnchor::U32 => PrimitiveTy::U32,
        BuiltinTypeAnchor::U64 => PrimitiveTy::U64,
        BuiltinTypeAnchor::U128 => PrimitiveTy::U128,
        BuiltinTypeAnchor::Usize => PrimitiveTy::Usize,
        BuiltinTypeAnchor::F32 => PrimitiveTy::F32,
        BuiltinTypeAnchor::F64 => PrimitiveTy::F64,
        BuiltinTypeAnchor::Bool => PrimitiveTy::Bool,
        BuiltinTypeAnchor::Char => PrimitiveTy::Char,
        BuiltinTypeAnchor::Void => PrimitiveTy::Void,
        BuiltinTypeAnchor::Never => PrimitiveTy::Never,
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ItemSignatureSource<'a> {
    Module(&'a Module),
    ActiveItemTree(&'a ActiveModuleItemTree),
}

#[derive(Clone, Copy)]
pub struct ItemSignatureInput<'a> {
    pub source: ItemSignatureSource<'a>,
    pub defs: &'a DefCollection,
    pub lowered: &'a TypeLowering,
    pub type_store: &'a TypeStore,
    pub symbols: Option<&'a dyn SymbolText>,
}

impl std::fmt::Debug for ItemSignatureInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemSignatureInput")
            .field("source", &self.source)
            .field("module_id", &self.defs.module_id)
            .field("type_store", &self.type_store.id())
            .field("symbols", &self.symbols.is_some())
            .finish_non_exhaustive()
    }
}

pub fn collect_item_signatures(input: ItemSignatureInput<'_>) -> ItemSignatures {
    let append = input.type_store.append_for_module(input.defs.module_id);
    let collect = |items| {
        collect_item_signatures_from_items(
            items,
            input.defs,
            input.lowered,
            input.type_store,
            &append,
            input.symbols,
        )
    };
    match input.source {
        ItemSignatureSource::Module(module) => {
            let item_tree = ModuleItemTree::from_module(module);
            collect(&item_tree.items)
        }
        ItemSignatureSource::ActiveItemTree(item_tree) => collect(&item_tree.items),
    }
}

fn collect_item_signatures_from_items(
    items: &[ItemTreeNode],
    defs: &DefCollection,
    lowered: &TypeLowering,
    type_store: &TypeStore,
    append: &TypeStoreAppend,
    symbols: Option<&dyn SymbolText>,
) -> ItemSignatures {
    let mut collector = SignatureCollector {
        defs,
        lowered,
        type_store,
        append,
        symbols,
        diagnostics: Vec::new(),
        duplicate_impl_identities: HashMap::new(),
    };
    let mut signatures = ItemSignatures {
        functions: HashMap::new(),
        structs: HashMap::new(),
        unions: HashMap::new(),
        traits: HashMap::new(),
        trait_impls: Vec::new(),
        enums: HashMap::new(),
        type_aliases: HashMap::new(),
        globals: HashMap::new(),
        consts: HashMap::new(),
        diagnostics: Vec::new(),
    };
    for item in items {
        collector.collect_item_into(&mut signatures, item);
    }
    signatures.diagnostics = collector.diagnostics;
    signatures
}

struct SignatureCollector<'a> {
    defs: &'a DefCollection,
    lowered: &'a TypeLowering,
    type_store: &'a TypeStore,
    append: &'a TypeStoreAppend,
    symbols: Option<&'a dyn SymbolText>,
    diagnostics: Vec<Diagnostic>,
    duplicate_impl_identities: HashMap<TraitImplIdentity, u32>,
}

impl<'a> SignatureCollector<'a> {
    fn symbol_debug_text(&self, symbol: SymbolId) -> String {
        symbol_debug_text_with_symbols(self.symbols, symbol)
    }

    fn attribute_path_text(&self, path: &[SymbolId]) -> String {
        attribute_path_text_with_symbols(path, self.symbols)
    }

    fn collect_item_into(&mut self, signatures: &mut ItemSignatures, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Module(_) | ItemTreeNodeKind::Using(_) => {}
            ItemTreeNodeKind::Struct(item_struct) => {
                self.collect_struct(signatures, item, item_struct);
            }
            ItemTreeNodeKind::Union(item_union) => {
                self.collect_union(signatures, item, item_union);
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                self.collect_trait(signatures, item, item_trait);
            }
            ItemTreeNodeKind::Extend(extend) => {
                self.collect_extend(signatures, item, extend);
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                self.collect_enum(signatures, item, item_enum);
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.collect_type_alias(signatures, item, alias);
            }
            ItemTreeNodeKind::Function(function) => {
                self.collect_function(signatures, item);
                self.collect_function_local_static_signatures(signatures, function);
            }
            ItemTreeNodeKind::Binding(binding) => {
                if binding.is_const() {
                    self.collect_const(signatures, item, binding);
                } else {
                    self.collect_global(signatures, item, binding);
                }
            }
        }
    }

    fn collect_struct(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_struct: &StructItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Struct) else {
            return;
        };
        let mut fields = Vec::new();
        for field in &item_struct.fields {
            let Some(field_id) =
                self.def_id_for_node(&field.node_key, field.span, DefKind::StructField)
            else {
                continue;
            };
            fields.push(FieldSignature {
                def_id: field_id,
                name: field.name,
                ty: self.ty_for_type(&field.ty),
                span: field.span,
            });
        }
        signatures.structs.insert(
            def_id,
            StructSignature {
                generics: generic_signature_names(&item_struct.generics),
                where_predicates: self.where_predicate_signatures(&item_struct.where_clause),
                fields,
                is_extern: item_struct.is_extern,
                span: item.span,
            },
        );
    }

    fn collect_union(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_union: &UnionItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Union) else {
            return;
        };
        let mut fields = Vec::new();
        for field in &item_union.fields {
            let Some(field_id) =
                self.def_id_for_node(&field.node_key, field.span, DefKind::UnionField)
            else {
                continue;
            };
            fields.push(FieldSignature {
                def_id: field_id,
                name: field.name,
                ty: self.ty_for_type(&field.ty),
                span: field.span,
            });
        }
        signatures.unions.insert(
            def_id,
            UnionSignature {
                generics: generic_signature_names(&item_union.generics),
                where_predicates: self.where_predicate_signatures(&item_union.where_clause),
                fields,
                is_extern: item_union.is_extern,
                span: item.span,
            },
        );
    }

    fn collect_extend(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        extend: &ExtendItem,
    ) {
        let methods = extend
            .methods
            .iter()
            .filter_map(|method| {
                self.collect_method(signatures, &method.function)
                    .map(|def_id| TraitImplMethodSignature {
                        def_id,
                        name: method.function.name,
                        visibility: method.vis,
                        span: method.function.span,
                    })
            })
            .collect();
        let associated_values = extend
            .associated_values
            .iter()
            .filter_map(|associated_value| {
                self.collect_associated_const(signatures, associated_value)
                    .map(|def_id| TraitImplAssociatedValueSignature {
                        def_id,
                        name: associated_value.binding.name,
                        visibility: associated_value.vis,
                        span: associated_value.span,
                    })
            })
            .collect();
        let impl_id = self.trait_impl_id(extend);
        signatures.trait_impls.push(TraitImplSignature {
            impl_id,
            builtin: self.builtin_extend_attribute(&item.attributes),
            generics: generic_signature_names(&extend.generics),
            target_ty: self.ty_for_type(&extend.target),
            trait_ty: extend
                .trait_ref
                .as_ref()
                .map(|trait_ref| self.ty_for_type(trait_ref)),
            trait_span: extend.trait_ref.as_ref().map(|trait_ref| trait_ref.span),
            where_predicates: self.where_predicate_signatures(&extend.where_clause),
            associated_types: extend
                .associated_types
                .iter()
                .map(|associated_type| TraitImplAssociatedTypeSignature {
                    name: associated_type.name,
                    ty: self.ty_for_type(&associated_type.ty),
                    span: associated_type.span,
                })
                .collect(),
            associated_values,
            methods,
            span: extend.target.span,
        });
    }

    fn trait_impl_id(&mut self, extend: &ExtendItem) -> TraitImplId {
        let identity = TraitImplIdentity::from_extend(extend);
        let ordinal = self
            .duplicate_impl_identities
            .entry(identity.clone())
            .or_default();
        let resolved = if *ordinal == 0 {
            identity
        } else {
            identity.duplicate(*ordinal)
        };
        *ordinal += 1;
        TraitImplId(stable_trait_impl_id(&resolved))
    }

    fn collect_trait(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_trait: &TraitItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Trait) else {
            return;
        };
        let mut associated_types = Vec::new();
        for associated_type in &item_trait.associated_types {
            let Some(associated_type_id) = self.def_id_for_node(
                &associated_type.node_key,
                associated_type.span,
                DefKind::TraitAssociatedType,
            ) else {
                continue;
            };
            associated_types.push(TraitAssociatedTypeSignature {
                def_id: associated_type_id,
                name: associated_type.name,
                span: associated_type.span,
            });
        }
        let mut associated_values = Vec::new();
        for associated_value in &item_trait.associated_values {
            let Some(associated_value_id) = self.def_id_for_node(
                &associated_value.node_key,
                associated_value.span,
                DefKind::Const,
            ) else {
                continue;
            };
            associated_values.push(TraitAssociatedValueSignature {
                def_id: associated_value_id,
                name: associated_value.name,
                ty: self.ty_for_type(&associated_value.ty),
                span: associated_value.span,
            });
        }
        let mut methods = Vec::new();
        for method in &item_trait.methods {
            let Some(method_id) = self.def_id_for_node(
                &method.function.node_key,
                method.function.span,
                DefKind::TraitMethod,
            ) else {
                continue;
            };
            let signature = self.function_signature(&method.function);
            methods.push(TraitMethodSignature {
                def_id: method_id,
                name: method.function.name,
                signature: signature.clone(),
                has_default: method.function.body.is_some(),
                span: method.function.span,
            });
            signatures.functions.insert(method_id, signature);
        }
        signatures.traits.insert(
            def_id,
            TraitSignature {
                generics: generic_signature_names(&item_trait.generics),
                where_predicates: self.where_predicate_signatures(&item_trait.where_clause),
                supertraits: item_trait
                    .supertraits
                    .iter()
                    .map(|supertrait| TraitSupertraitSignature {
                        ty: self.ty_for_type(supertrait),
                        span: supertrait.span,
                    })
                    .collect(),
                associated_types,
                associated_values,
                methods,
                builtin: self.builtin_trait_attribute(&item.attributes),
                span: item.span,
            },
        );
    }

    fn collect_method(
        &mut self,
        signatures: &mut ItemSignatures,
        method: &FunctionItem,
    ) -> Option<DefId> {
        let def_id = self.def_id_for_node(&method.node_key, method.span, DefKind::Method)?;
        signatures
            .functions
            .insert(def_id, self.function_signature(method));
        Some(def_id)
    }

    fn collect_enum(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_enum: &EnumItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Enum) else {
            return;
        };
        let backing_type = match &item_enum.backing_type {
            Some(ty) => self.ty_for_type(ty),
            None => self.primitive(PrimitiveTy::I32),
        };
        let mut variants = Vec::new();
        for variant in &item_enum.variants {
            let Some(variant_id) =
                self.def_id_for_node(&variant.node_key, variant.span, DefKind::EnumVariant)
            else {
                continue;
            };
            variants.push(EnumVariantSignature {
                def_id: variant_id,
                name: variant.name,
                span: variant.span,
            });
        }
        signatures.enums.insert(
            def_id,
            EnumSignature {
                backing_type,
                is_open: item_enum.is_open,
                variants,
                span: item.span,
            },
        );
    }

    fn collect_type_alias(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        alias: &TypeAliasItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::TypeAlias)
        else {
            return;
        };
        signatures.type_aliases.insert(
            def_id,
            TypeAliasSignature {
                generics: generic_signature_names(&alias.generics),
                target: self.type_alias_target(item, alias),
                span: item.span,
            },
        );
    }

    fn type_alias_target(&mut self, item: &ItemTreeNode, alias: &TypeAliasItem) -> InternedTyId {
        if let Some(ty) = &alias.ty {
            return self.ty_for_type(ty);
        }
        let Some(builtin) = self.builtin_type_attribute(&item.attributes) else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::ITEM_SIGNATURE_LOWERED_TYPE,
                    "bodyless type alias without valid builtin attribute reached item signatures",
                )
                .primary(item.span, "this type alias has no target type")
                .finish(),
            );
            return self.error();
        };
        match builtin {
            BuiltinTypeDeclaration::Opaque(builtin) => {
                self.append.intern(TyKind::BuiltinType(builtin))
            }
            BuiltinTypeDeclaration::Primitive(anchor) => {
                self.primitive(builtin_type_anchor_primitive(anchor))
            }
        }
    }

    fn collect_function(&mut self, signatures: &mut ItemSignatures, item: &ItemTreeNode) {
        let ItemTreeNodeKind::Function(function) = &item.kind else {
            return;
        };
        let Some(def_id) =
            self.def_id_for_node(&function.node_key, function.span, DefKind::Function)
        else {
            return;
        };
        let attributes = self.function_attributes(&item.attributes, function);
        signatures.functions.insert(
            def_id,
            self.function_signature_with_attributes(function, attributes),
        );
    }

    fn collect_function_local_static_signatures(
        &mut self,
        signatures: &mut ItemSignatures,
        function: &FunctionItem,
    ) {
        let Some(body) = &function.body else {
            return;
        };
        self.collect_block_static_signatures(signatures, body);
    }

    fn collect_block_static_signatures(&mut self, signatures: &mut ItemSignatures, block: &Block) {
        for stmt in &block.stmts {
            match &stmt.kind {
                StmtKind::Static(binding) => {
                    let Some(def_id) =
                        self.def_id_for_node(&binding.node_key, stmt.span, DefKind::Global)
                    else {
                        continue;
                    };
                    signatures.globals.insert(
                        def_id,
                        GlobalSignature {
                            explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                            is_mutable: binding.is_mutable(),
                            is_extern: false,
                            span: stmt.span,
                        },
                    );
                }
                StmtKind::ForIn(for_stmt) => {
                    self.collect_block_static_signatures(signatures, &for_stmt.body);
                }
                StmtKind::While(while_stmt) => {
                    self.collect_block_static_signatures(signatures, &while_stmt.body);
                }
                StmtKind::Loop(loop_stmt) => {
                    self.collect_block_static_signatures(signatures, &loop_stmt.body);
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

    fn collect_global(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        binding: &BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item.span, DefKind::Global)
        else {
            return;
        };
        signatures.globals.insert(
            def_id,
            GlobalSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                is_mutable: binding.is_mutable(),
                is_extern: binding.is_extern(),
                span: item.span,
            },
        );
    }

    fn collect_const(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        binding: &BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item.span, DefKind::Const)
        else {
            return;
        };
        signatures.consts.insert(
            def_id,
            ConstSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                builtin: self.builtin_const_attribute(&item.attributes, binding),
                span: item.span,
            },
        );
    }

    fn collect_associated_const(
        &mut self,
        signatures: &mut ItemSignatures,
        associated_value: &nia_ast::ExtendAssociatedValue,
    ) -> Option<DefId> {
        let binding = &associated_value.binding;
        let def_id =
            self.def_id_for_node(&binding.node_key, associated_value.span, DefKind::Const)?;
        signatures.consts.insert(
            def_id,
            ConstSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                builtin: None,
                span: associated_value.span,
            },
        );
        Some(def_id)
    }

    fn builtin_const_attribute(
        &mut self,
        attributes: &[Attribute],
        binding: &BindingItem,
    ) -> Option<BuiltinConstValue> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) = BuiltinConstValue::from_name(builtin_name.as_str()) {
                            if out.replace(builtin).is_some() {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    "duplicate `@[builtin]` const attribute",
                                ));
                            }
                            if builtin_const_item_symbol(builtin) != binding.name {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    format!(
                                        "builtin const source item `{}` must match descriptor item `{}`",
                                        self.symbol_debug_text(binding.name),
                                        builtin.item_name()
                                    ),
                                ));
                            }
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin const `{builtin_name}`"),
                            ));
                        }
                    }
                    if binding.is_extern() || binding.value.is_some() || binding.ty.is_none() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[builtin]` is only valid on bodyless non-extern const declarations with an explicit type",
                        ));
                    }
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown const attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        out
    }

    fn function_signature(&mut self, function: &FunctionItem) -> FunctionSignature {
        let params = function
            .params
            .iter()
            .map(|param| self.param_signature(param))
            .collect();
        let return_type = match &function.return_type {
            Some(ty) => self.ty_for_type(ty),
            None => self.primitive(PrimitiveTy::Void),
        };
        FunctionSignature {
            name: function.name,
            generics: generic_signature_names(&function.generics),
            generic_params: self.generic_param_signatures(&function.generics),
            where_predicates: self.where_predicate_signatures(&function.where_clause),
            params,
            return_type,
            is_extern: function.is_extern,
            is_const: function.is_const,
            is_variadic: function.is_variadic,
            attributes: Vec::new(),
            has_body: function.body.is_some(),
            span: function.span,
        }
    }

    fn generic_param_signatures(
        &mut self,
        generics: &[GenericParam],
    ) -> Vec<GenericParamSignature> {
        generics
            .iter()
            .map(|generic| GenericParamSignature {
                name: generic.name,
                kind: match &generic.kind {
                    GenericParamKind::Type => GenericParamSignatureKind::Type,
                    GenericParamKind::Const { ty } => GenericParamSignatureKind::Const {
                        ty: self.ty_for_type(ty),
                    },
                },
            })
            .collect()
    }

    fn function_signature_with_attributes(
        &mut self,
        function: &FunctionItem,
        attributes: Vec<FunctionAttribute>,
    ) -> FunctionSignature {
        let mut signature = self.function_signature(function);
        signature.attributes = attributes;
        signature
    }

    fn function_attributes(
        &mut self,
        attributes: &[Attribute],
        function: &FunctionItem,
    ) -> Vec<FunctionAttribute> {
        let mut out = Vec::new();
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::NAKED => {
                    if !meta.args.is_empty() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[naked]` does not take arguments",
                        ));
                    }
                    if !function.is_extern || function.body.is_none() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[naked]` is only valid on `extern fn` definitions",
                        ));
                    }
                    out.push(FunctionAttribute::Naked);
                }
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) = BuiltinFunction::from_name(builtin_name.as_str()) {
                            out.push(FunctionAttribute::Builtin(builtin));
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin function `{builtin_name}`"),
                            ));
                        }
                    }
                    if function.is_extern || function.body.is_some() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[builtin]` is only valid on bodyless non-extern function declarations",
                        ));
                    }
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown function attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        if !function.is_extern
            && function.body.is_none()
            && !out
                .iter()
                .any(|attribute| matches!(attribute, FunctionAttribute::Builtin(_)))
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::ITEM_SIGNATURE,
                function.span,
                "bodyless non-extern functions require `@[builtin]`",
            ));
        }
        out
    }

    fn builtin_trait_attribute(&mut self, attributes: &[Attribute]) -> Option<BuiltinTrait> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) = BuiltinTrait::from_name(builtin_name.as_str()) {
                            if out.replace(builtin).is_some() {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    "duplicate `@[builtin]` trait attribute",
                                ));
                            }
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin trait `{builtin_name}`"),
                            ));
                        }
                    }
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown trait attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        out
    }

    fn builtin_type_attribute(
        &mut self,
        attributes: &[Attribute],
    ) -> Option<BuiltinTypeDeclaration> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) =
                            BuiltinTypeDeclaration::from_name(builtin_name.as_str())
                        {
                            if out.replace(builtin).is_some() {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    "duplicate `@[builtin]` type attribute",
                                ));
                            }
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin type `{builtin_name}`"),
                            ));
                        }
                    }
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown type attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        out
    }

    fn builtin_extend_attribute(&mut self, attributes: &[Attribute]) -> Option<String> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            if meta.path.as_slice() != [known::BUILTIN] {
                continue;
            }
            if let Some(builtin_name) =
                self.parse_builtin_attribute_name(attribute, meta.args.as_slice())
                && out.replace(builtin_name).is_some()
            {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::ITEM_SIGNATURE,
                    attribute.span,
                    "duplicate `@[builtin]` extend attribute",
                ));
            }
        }
        out
    }

    fn parse_builtin_attribute_name(
        &mut self,
        attribute: &Attribute,
        args: &[nia_ast::Expr],
    ) -> Option<String> {
        match args {
            [arg] => match &arg.kind {
                nia_ast::ExprKind::String(text) => {
                    let name = nia_literals::eval_string_literal_parts(
                        text.parts.iter().map(String::as_str),
                    );
                    if name.is_none() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            arg.span,
                            "`@[builtin]` expects a valid string literal name",
                        ));
                    }
                    name
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        arg.span,
                        "`@[builtin]` expects a single string literal name",
                    ));
                    None
                }
            },
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::ITEM_SIGNATURE,
                    attribute.span,
                    "`@[builtin]` expects exactly one string literal name",
                ));
                None
            }
        }
    }

    fn param_signature(&mut self, param: &Param) -> ParamSignature {
        let ty = match &param.ty {
            Some(ty) => self.ty_for_type(ty),
            None if param.receiver.is_some() => self.error(),
            None => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::ITEM_SIGNATURE,
                    param.span,
                    "parameter requires an explicit type",
                ));
                self.error()
            }
        };
        ParamSignature {
            name: param.name,
            receiver: param.receiver,
            ty,
            span: param.span,
        }
    }

    fn where_predicate_signatures(&mut self, clause: &WhereClause) -> Vec<WherePredicateSignature> {
        clause
            .predicates
            .iter()
            .map(|predicate| WherePredicateSignature {
                ty: self.ty_for_type(&predicate.ty),
                bounds: predicate
                    .bounds
                    .iter()
                    .map(|bound| WhereBoundSignature {
                        trait_ty: self.ty_for_type(bound),
                        associated_type_bindings: self.associated_type_binding_signatures(bound),
                        span: bound.span,
                    })
                    .collect(),
                span: predicate.span,
            })
            .collect()
    }

    fn associated_type_binding_signatures(
        &mut self,
        bound: &nia_ast::TypeRef,
    ) -> Vec<AssociatedTypeBindingSignature> {
        let nia_ast::TypeKind::Path { segments } = &bound.kind else {
            return Vec::new();
        };
        let Some(segment) = segments.last() else {
            return Vec::new();
        };
        segment
            .args
            .iter()
            .filter_map(|arg| match arg {
                nia_ast::TypeArg::AssocBinding { key, ty, span } => {
                    let name = match key {
                        nia_ast::AssocBindingKey::Name(name) => *name,
                        nia_ast::AssocBindingKey::Projection(projection) => {
                            let nia_ast::TypeKind::Projection { name, .. } = &projection.kind
                            else {
                                return None;
                            };
                            *name
                        }
                    };
                    Some(AssociatedTypeBindingSignature {
                        name,
                        ty: self.ty_for_type(ty),
                        span: *span,
                    })
                }
                nia_ast::TypeArg::Type(_)
                | nia_ast::TypeArg::Const(_)
                | nia_ast::TypeArg::TypeOrConst { .. } => None,
            })
            .collect()
    }

    fn def_id_for_node(
        &mut self,
        node_key: &VersionedNodeKey,
        diagnostic_span: Span,
        expected: DefKind,
    ) -> Option<DefId> {
        let Some(def_id) = self.defs.def_nodes.get(node_key) else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::ITEM_SIGNATURE_DEF_NODE,
                    "missing definition id while collecting item signature",
                )
                .primary(diagnostic_span, "this syntax node has no definition id")
                .debug("node_key", node_key)
                .debug("expected_def_kind", expected)
                .finish(),
            );
            return None;
        };
        let Some(def) = self.defs.defs.get(def_id) else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::ITEM_SIGNATURE_DEF_MAP,
                    "definition id does not exist in definition map",
                )
                .primary(diagnostic_span, "definition map lookup failed here")
                .debug("node_key", node_key)
                .debug("def_id", def_id)
                .debug("expected_def_kind", expected)
                .finish(),
            );
            return None;
        };
        if def.kind != expected {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::ITEM_SIGNATURE_DEF_KIND,
                    "definition kind mismatch while collecting item signature",
                )
                .primary(def.span, "definition has an unexpected kind")
                .debug("node_key", node_key)
                .debug("def_id", def_id)
                .debug("expected_def_kind", expected)
                .debug("actual_def_kind", def.kind)
                .finish(),
            );
            return None;
        }
        Some(def_id)
    }

    fn ty_for_type(&mut self, ty_ref: &TypeRef) -> InternedTyId {
        if let Some(ty) = self.lowered.ty_for_key(&ty_ref.node_key) {
            if self.type_store.get(ty).is_some() {
                ty
            } else {
                self.diagnostics.push(
                    Diagnostic::internal_error(
                        codes::ITEM_SIGNATURE_LOWERED_TYPE,
                        "lowered type is outside the session type store",
                    )
                    .primary(ty_ref.span, "this type belongs to a different type store")
                    .debug("node_key", &ty_ref.node_key)
                    .debug("ty", ty)
                    .finish(),
                );
                self.error()
            }
        } else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::ITEM_SIGNATURE_LOWERED_TYPE,
                    "missing lowered type while collecting item signature",
                )
                .primary(ty_ref.span, "this type reference was not lowered")
                .debug("node_key", &ty_ref.node_key)
                .finish(),
            );
            self.error()
        }
    }

    fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.append.intern(TyKind::Primitive(primitive))
    }

    fn error(&self) -> InternedTyId {
        self.append.intern(TyKind::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};

    use nia_defs::{collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_ids::ModuleIdAllocator;
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;
    use nia_symbol::{ToSymbolId, stable_hash};
    use nia_type_lower::{TypeLoweringContext, lower_module_types_with_context};
    use nia_type_resolve::resolve_module_types;

    include!("tests/item_signatures/test_support.rs");

    #[path = "item_signatures/collection_contracts.rs"]
    mod collection_contracts;

    #[path = "item_signatures/builtin_contracts.rs"]
    mod builtin_contracts;

    #[path = "item_signatures/std_builtin_contracts.rs"]
    mod std_builtin_contracts;

    #[path = "item_signatures/type_store_contracts.rs"]
    mod type_store_contracts;
}
