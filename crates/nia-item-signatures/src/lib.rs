// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    Attribute, AttributeKind, BindingItem, Block, EnumItem, EnumVariantPayload, ExtendItem,
    FunctionItem, GenericParam, GenericParamKind, Module, Param, StmtKind, StructItem, TraitItem,
    TypeAliasItem, TypeRef, UnionItem, WhereClause, generic_param_identities, type_ref_identity,
    where_clause_identity,
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

mod collector;
mod trait_impls;

pub use collector::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
pub use trait_impls::ProgramTraitImplIndex;
use trait_impls::{TraitImplIdentity, stable_trait_impl_id};

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
        for signature in self.enums.values() {
            roots.push(signature.backing_type);
            for variant in &signature.variants {
                match &variant.payload {
                    EnumVariantPayloadSignature::Unit => {}
                    EnumVariantPayloadSignature::Tuple(fields) => {
                        roots.extend(fields.iter().copied());
                    }
                    EnumVariantPayloadSignature::Named(fields) => {
                        roots.extend(fields.iter().map(|field| field.ty));
                    }
                }
            }
        }
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
    pub generic_params: Vec<GenericParamSignature>,
    pub target_ty: InternedTyId,
    pub trait_id: nia_ty::TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    pub associated_values: Vec<TraitImplAssociatedValueSignature>,
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
    pub generic_params: Vec<GenericParamSignature>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub fields: Vec<FieldSignature>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionSignature {
    pub generics: Vec<SymbolId>,
    pub generic_params: Vec<GenericParamSignature>,
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
    pub generic_params: Vec<GenericParamSignature>,
    pub target_ty: InternedTyId,
    pub trait_ty: Option<InternedTyId>,
    pub trait_span: Option<Span>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    pub associated_values: Vec<TraitImplAssociatedValueSignature>,
    pub methods: Vec<TraitImplMethodSignature>,
    pub span: Span,
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
            generic_params: self.generic_params.clone(),
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
    pub payload: EnumVariantPayloadSignature,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariantPayloadSignature {
    Unit,
    Tuple(Vec<InternedTyId>),
    Named(Vec<FieldSignature>),
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
