// SPDX-License-Identifier: GPL-3.0-or-later
//! Module-local semantic signatures derived from active items and lowered types.
//!
//! Signatures contain declaration facts needed by later semantic and backend
//! phases, but not function-body analysis. Program wrappers qualify these facts
//! with their owning module outside this crate.
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

/// Complete signature product for one module.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemSignatures {
    /// Function signatures keyed by module-local definition id.
    pub functions: HashMap<DefId, FunctionSignature>,
    /// Struct signatures keyed by definition id.
    pub structs: HashMap<DefId, StructSignature>,
    /// Union signatures keyed by definition id.
    pub unions: HashMap<DefId, UnionSignature>,
    /// Trait signatures keyed by definition id.
    pub traits: HashMap<DefId, TraitSignature>,
    /// Trait and inherent extensions in declaration order.
    pub trait_impls: Vec<TraitImplSignature>,
    /// Enum signatures keyed by definition id.
    pub enums: HashMap<DefId, EnumSignature>,
    /// Type-alias signatures keyed by definition id.
    pub type_aliases: HashMap<DefId, TypeAliasSignature>,
    /// Static/global signatures keyed by definition id.
    pub globals: HashMap<DefId, GlobalSignature>,
    /// Const signatures keyed by definition id.
    pub consts: HashMap<DefId, ConstSignature>,
    /// Diagnostics produced while collecting signatures.
    pub diagnostics: Vec<Diagnostic>,
}

impl ItemSignatures {
    /// Returns deduplicated type-store roots referenced by this product.
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
            for supertrait in &signature.supertraits {
                roots.push(supertrait.ty);
                roots.extend(
                    supertrait
                        .associated_type_bindings
                        .iter()
                        .map(|binding| binding.ty),
                );
            }
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

/// Program-qualified function signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramFunctionSignature {
    /// Function name.
    pub name: SymbolId,
    /// Module-local signature payload.
    pub signature: FunctionSignature,
}

/// Program-qualified global signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramGlobalSignature {
    /// Module-local signature payload.
    pub signature: GlobalSignature,
}

/// Program-qualified const signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramConstSignature {
    /// Module-local signature payload.
    pub signature: ConstSignature,
}

/// Program-qualified struct signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramStructSignature {
    /// Module-local signature payload.
    pub signature: StructSignature,
}

/// Program-qualified union signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramUnionSignature {
    /// Module-local signature payload.
    pub signature: UnionSignature,
}

/// Program-qualified enum signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramEnumSignature {
    /// Module-local signature payload.
    pub signature: EnumSignature,
}

/// Program-qualified trait signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTraitSignature {
    /// Module-local signature payload.
    pub signature: TraitSignature,
}

/// Program-qualified type-alias signature payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTypeAliasSignature {
    /// Module-local signature payload.
    pub signature: TypeAliasSignature,
}

/// Program-wide trait implementation signature.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTraitImplSignature {
    /// Module containing the implementation.
    pub module_id: nia_ids::ModuleId,
    /// Stable implementation identity.
    pub impl_id: TraitImplId,
    /// Optional builtin implementation tag.
    pub builtin: Option<String>,
    /// Compact generic parameter name list.
    pub generics: Vec<SymbolId>,
    /// Declaration-order kind-aware generic parameters.
    pub generic_params: Vec<GenericParamSignature>,
    /// Implemented self/target type.
    pub target_ty: InternedTyId,
    /// Implemented trait identity.
    pub trait_id: nia_ty::TraitId,
    /// Trait type arguments.
    pub trait_args: Vec<InternedTyId>,
    /// Trait const arguments.
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
    /// Implementation where predicates.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Associated type definitions.
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    /// Associated value definitions.
    pub associated_values: Vec<TraitImplAssociatedValueSignature>,
}

/// Function declaration signature without body facts.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    /// Function name.
    pub name: SymbolId,
    /// Compact generic parameter name list.
    pub generics: Vec<SymbolId>,
    /// Declaration-order kind-aware generic parameters.
    pub generic_params: Vec<GenericParamSignature>,
    /// Where-clause predicates.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Parameter signatures.
    pub params: Vec<ParamSignature>,
    /// Normalized return type.
    pub return_type: InternedTyId,
    /// Whether external linkage is requested.
    pub is_extern: bool,
    /// Whether const evaluation is permitted.
    pub is_const: bool,
    /// Whether variadic arguments are accepted.
    pub is_variadic: bool,
    /// Validated function attributes.
    pub attributes: Vec<FunctionAttribute>,
    /// Whether the declaration contains a body.
    pub has_body: bool,
    /// Declaration source span.
    pub span: Span,
}

/// Declaration-order generic parameter metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericParamSignature {
    /// Parameter name.
    pub name: SymbolId,
    /// Type or const parameter kind.
    pub kind: GenericParamSignatureKind,
}

/// Kind-specific generic parameter metadata.
#[derive(Debug, Clone, PartialEq)]
pub enum GenericParamSignatureKind {
    /// Type parameter.
    Type,
    /// Const parameter with its declared type.
    Const {
        /// Declared const parameter type.
        ty: InternedTyId,
    },
}

/// Rebuilds type and const substitutions from declaration-order parameter metadata.
///
/// Nominal type identities store type arguments and const arguments in separate
/// vectors even when their source parameters are interleaved. Independent cursors
/// preserve that declaration mapping and reject missing or surplus arguments.
pub fn generic_argument_substitutions(
    params: &[GenericParamSignature],
    args: &[InternedTyId],
    const_args: &[nia_ty::ConstGenericArg],
) -> Option<(
    nia_symbol::SymbolMap<InternedTyId>,
    nia_symbol::SymbolMap<nia_ty::ConstGenericArg>,
)> {
    let mut type_index = 0;
    let mut const_index = 0;
    let mut substitutions = nia_symbol::SymbolMap::default();
    let mut const_substitutions = nia_symbol::SymbolMap::default();
    for param in params {
        match param.kind {
            GenericParamSignatureKind::Type => {
                substitutions.insert(param.name, *args.get(type_index)?);
                type_index += 1;
            }
            GenericParamSignatureKind::Const { .. } => {
                const_substitutions.insert(param.name, const_args.get(const_index)?.clone());
                const_index += 1;
            }
        }
    }
    (type_index == args.len() && const_index == const_args.len())
        .then_some((substitutions, const_substitutions))
}

/// Function attributes retained after declaration validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAttribute {
    /// Request naked calling convention.
    Naked,
    /// Mark a compiler-provided builtin.
    Builtin(BuiltinFunction),
    /// Forward the outer tracked call site's source location.
    TrackCaller,
}

pub use nia_ids::{BuiltinFunction, BuiltinTrait};

/// One lowered function parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSignature {
    /// Optional source name.
    pub name: Option<SymbolId>,
    /// Optional receiver mode.
    pub receiver: Option<ReceiverKind>,
    /// Lowered parameter type.
    pub ty: InternedTyId,
    /// Source span.
    pub span: Span,
}

/// Lowered struct declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct StructSignature {
    /// Compact generic names.
    pub generics: Vec<SymbolId>,
    /// Kind-aware generic metadata.
    pub generic_params: Vec<GenericParamSignature>,
    /// Where predicates.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Field signatures.
    pub fields: Vec<FieldSignature>,
    /// Tuple construction marker.
    pub is_tuple: bool,
    /// External ABI marker.
    pub is_extern: bool,
    /// Source span.
    pub span: Span,
}

/// Lowered union declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct UnionSignature {
    /// Compact generic names.
    pub generics: Vec<SymbolId>,
    /// Kind-aware generic metadata.
    pub generic_params: Vec<GenericParamSignature>,
    /// Where predicates.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Field signatures.
    pub fields: Vec<FieldSignature>,
    /// External ABI marker.
    pub is_extern: bool,
    /// Source span.
    pub span: Span,
}

/// Lowered trait declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitSignature {
    /// Compact generic names.
    pub generics: Vec<SymbolId>,
    /// Declaration-order generic parameters, including whether each parameter
    /// is a type or const. `generics` remains the compact name list used by
    /// older consumers; instantiation must use this kind-aware representation.
    pub generic_params: Vec<GenericParamSignature>,
    /// Where predicates.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Supertrait signatures.
    pub supertraits: Vec<TraitSupertraitSignature>,
    /// Associated type declarations.
    pub associated_types: Vec<TraitAssociatedTypeSignature>,
    /// Associated value declarations.
    pub associated_values: Vec<TraitAssociatedValueSignature>,
    /// Method declarations.
    pub methods: Vec<TraitMethodSignature>,
    /// Optional builtin marker.
    pub builtin: Option<BuiltinTrait>,
    /// Source span.
    pub span: Span,
}

/// One lowered supertrait bound.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitSupertraitSignature {
    /// Lowered trait type.
    pub ty: InternedTyId,
    /// Associated type bindings.
    pub associated_type_bindings: Vec<AssociatedTypeBindingSignature>,
    /// Source span.
    pub span: Span,
}

/// Trait associated type declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedTypeSignature {
    /// Definition identity.
    pub def_id: DefId,
    /// Associated type name.
    pub name: SymbolId,
    /// Source span.
    pub span: Span,
}

/// Trait associated value declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedValueSignature {
    /// Definition identity.
    pub def_id: DefId,
    /// Associated value name.
    pub name: SymbolId,
    /// Lowered value type.
    pub ty: InternedTyId,
    /// Source span.
    pub span: Span,
}

/// Trait method declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethodSignature {
    /// Definition identity.
    pub def_id: DefId,
    /// Method name.
    pub name: SymbolId,
    /// Function signature.
    pub signature: FunctionSignature,
    /// Whether a default body is present.
    pub has_default: bool,
    /// Source span.
    pub span: Span,
}

/// Lowered trait or inherent implementation signature.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplSignature {
    /// Stable implementation identity.
    pub impl_id: TraitImplId,
    /// Optional builtin implementation marker.
    pub builtin: Option<String>,
    /// Compact generic names.
    pub generics: Vec<SymbolId>,
    /// Kind-aware generic metadata.
    pub generic_params: Vec<GenericParamSignature>,
    /// Target type.
    pub target_ty: InternedTyId,
    /// Optional implemented trait type.
    pub trait_ty: Option<InternedTyId>,
    /// Optional trait source span.
    pub trait_span: Option<Span>,
    /// Where predicates.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Associated type definitions.
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    /// Associated value definitions.
    pub associated_values: Vec<TraitImplAssociatedValueSignature>,
    /// Method definitions.
    pub methods: Vec<TraitImplMethodSignature>,
    /// Source span.
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

/// Associated type definition in an implementation.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplAssociatedTypeSignature {
    /// Associated type name.
    pub name: SymbolId,
    /// Defined lowered type.
    pub ty: InternedTyId,
    /// Source span.
    pub span: Span,
}

/// Associated value definition in an implementation.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplAssociatedValueSignature {
    /// Definition identity.
    pub def_id: DefId,
    /// Associated value name.
    pub name: SymbolId,
    /// Visibility of the definition.
    pub visibility: Visibility,
    /// Source span.
    pub span: Span,
}

/// Method definition in an implementation.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplMethodSignature {
    /// Definition identity.
    pub def_id: DefId,
    /// Method name.
    pub name: SymbolId,
    /// Visibility of the method.
    pub visibility: Visibility,
    /// Source span.
    pub span: Span,
}

impl UnionSignature {
    /// Views this union's fields as a struct-like signature for shared layout checks.
    pub fn as_struct_like(&self) -> StructSignature {
        StructSignature {
            generics: self.generics.clone(),
            generic_params: self.generic_params.clone(),
            where_predicates: self.where_predicates.clone(),
            fields: self.fields.clone(),
            is_tuple: false,
            is_extern: self.is_extern,
            span: self.span,
        }
    }
}

/// Lowered field signature.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSignature {
    /// Field definition identity.
    pub def_id: DefId,
    /// Field name.
    pub name: SymbolId,
    /// Lowered field type.
    pub ty: InternedTyId,
    /// Source span.
    pub span: Span,
}

/// Lowered enum declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumSignature {
    /// Lowered backing type.
    pub backing_type: InternedTyId,
    /// Whether external variants may exist.
    pub is_open: bool,
    /// Variant signatures in declaration order.
    pub variants: Vec<EnumVariantSignature>,
    /// Source span.
    pub span: Span,
}

/// One lowered enum variant signature.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantSignature {
    /// Variant definition identity.
    pub def_id: DefId,
    /// Variant name.
    pub name: SymbolId,
    /// Variant payload signature.
    pub payload: EnumVariantPayloadSignature,
    /// Source span.
    pub span: Span,
}

/// Lowered enum payload shape.
#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariantPayloadSignature {
    /// No payload.
    Unit,
    /// Positional payload types.
    Tuple(Vec<InternedTyId>),
    /// Named payload fields.
    Named(Vec<FieldSignature>),
}

/// Lowered type-alias signature.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasSignature {
    /// Compact generic names.
    pub generics: Vec<SymbolId>,
    /// Declaration-order parameter kinds used to rebuild the separate type and
    /// const argument vectors stored in nominal type identities.
    pub generic_params: Vec<GenericParamSignature>,
    /// Lowered alias target.
    pub target: InternedTyId,
    /// Source span.
    pub span: Span,
}

/// Global/static declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalSignature {
    /// Optional explicit declared type.
    pub explicit_type: Option<InternedTyId>,
    /// Whether mutation is permitted.
    pub is_mutable: bool,
    /// Whether storage is externally defined.
    pub is_extern: bool,
    /// Source span.
    pub span: Span,
}

/// Const declaration signature.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstSignature {
    /// Optional explicit declared type.
    pub explicit_type: Option<InternedTyId>,
    /// Optional builtin const marker.
    pub builtin: Option<BuiltinConstValue>,
    /// Source span.
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use nia_defs::{collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_ids::ModuleIdAllocator;
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;
    use nia_symbol::{ToSymbolId, stable_hash};
    use nia_type_lower::{
        ProgramDefsContext, TypeLoweringContext, lower_module_types_with_context,
    };
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

    #[test]
    fn generic_argument_substitutions_preserve_interleaved_parameter_order() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = TypeStore::new();
        let append = type_store.append_for_module(module_id);
        let usize_ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::Usize));
        let u8_ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::U8));
        let u16_ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::U16));
        let type_name = sym("T");
        let const_name = sym("N");
        let trailing_type_name = sym("U");
        let params = [
            GenericParamSignature {
                name: type_name,
                kind: GenericParamSignatureKind::Type,
            },
            GenericParamSignature {
                name: const_name,
                kind: GenericParamSignatureKind::Const { ty: usize_ty },
            },
            GenericParamSignature {
                name: trailing_type_name,
                kind: GenericParamSignatureKind::Type,
            },
        ];
        let const_arg = nia_ty::ConstGenericArg {
            ty: usize_ty,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(4)),
        };

        let (types, consts) = generic_argument_substitutions(
            &params,
            &[u8_ty, u16_ty],
            std::slice::from_ref(&const_arg),
        )
        .expect("complete argument vectors");
        assert_eq!(types.get(&type_name), Some(&u8_ty));
        assert_eq!(types.get(&trailing_type_name), Some(&u16_ty));
        assert_eq!(consts.get(&const_name), Some(&const_arg));
        assert!(
            generic_argument_substitutions(&params, &[u8_ty], std::slice::from_ref(&const_arg))
                .is_none()
        );
        assert!(
            generic_argument_substitutions(
                &params,
                &[u8_ty, u16_ty],
                &[const_arg.clone(), const_arg],
            )
            .is_none()
        );
    }
}
