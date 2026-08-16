use nia_ids::InternedTyId;
use nia_symbol::SymbolId;

use crate::{
    ArrayLenTy, AssociatedTypeBindingTy, ConstGenericArg, ConstGenericValue, TyKind, TypeStore,
    TypeStoreAppend,
};

pub fn substitute_ty(
    store: &TypeStore,
    append: &TypeStoreAppend,
    ty: InternedTyId,
    type_arg: &impl Fn(&SymbolId) -> Option<InternedTyId>,
    const_arg: &impl Fn(&SymbolId) -> Option<ConstGenericArg>,
    self_ty: Option<InternedTyId>,
) -> InternedTyId {
    let substitute = |ty| substitute_ty(store, append, ty, type_arg, const_arg, self_ty);
    let substitute_const_arg = |arg: &ConstGenericArg| {
        let mut substituted = match &arg.value {
            ConstGenericValue::GenericParam(name) => const_arg(name).unwrap_or_else(|| arg.clone()),
            ConstGenericValue::ConstExpr(_)
            | ConstGenericValue::Int(_)
            | ConstGenericValue::Bool(_)
            | ConstGenericValue::Char(_) => arg.clone(),
        };
        substituted.ty = substitute(substituted.ty);
        substituted
    };
    let substitute_binding = |binding: &AssociatedTypeBindingTy| AssociatedTypeBindingTy {
        trait_id: binding.trait_id,
        trait_args: binding.trait_args.iter().copied().map(substitute).collect(),
        trait_const_args: binding
            .trait_const_args
            .iter()
            .map(substitute_const_arg)
            .collect(),
        name: binding.name,
        ty: substitute(binding.ty),
    };

    match store.get(ty) {
        Some(TyKind::GenericParam(name)) => type_arg(name).unwrap_or(ty),
        Some(TyKind::SelfParam) => self_ty.unwrap_or(ty),
        Some(TyKind::Tuple(elems)) => append.intern(TyKind::Tuple(
            elems.iter().copied().map(substitute).collect(),
        )),
        Some(TyKind::Pointer { is_readonly, elem }) => append.intern(TyKind::Pointer {
            is_readonly: *is_readonly,
            elem: substitute(*elem),
        }),
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            append.intern(TyKind::VolatilePointer {
                is_readonly: *is_readonly,
                elem: substitute(*elem),
            })
        }
        Some(TyKind::Slice { is_readonly, elem }) => append.intern(TyKind::Slice {
            is_readonly: *is_readonly,
            elem: substitute(*elem),
        }),
        Some(TyKind::SlicePointee { elem }) => append.intern(TyKind::SlicePointee {
            elem: substitute(*elem),
        }),
        Some(TyKind::Array { len, elem }) => {
            let len = match len {
                ArrayLenTy::GenericParam(name) => const_arg(name)
                    .as_ref()
                    .and_then(array_len_from_const_arg)
                    .unwrap_or_else(|| len.clone()),
                ArrayLenTy::Builtin { builtin, ty } => ArrayLenTy::Builtin {
                    builtin: *builtin,
                    ty: substitute(*ty),
                },
                _ => len.clone(),
            };
            append.intern(TyKind::Array {
                len,
                elem: substitute(*elem),
            })
        }
        Some(TyKind::Range { kind, bound }) => append.intern(TyKind::Range {
            kind: *kind,
            bound: bound.map(substitute),
        }),
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => append.intern(TyKind::FunctionPointer {
            params: params.iter().copied().map(substitute).collect(),
            return_type: substitute(*return_type),
            is_variadic: *is_variadic,
        }),
        Some(TyKind::Callable {
            is_readonly,
            params,
            return_type,
        }) => append.intern(TyKind::Callable {
            is_readonly: *is_readonly,
            params: params.iter().copied().map(substitute).collect(),
            return_type: substitute(*return_type),
        }),
        Some(TyKind::CallablePointee {
            params,
            return_type,
        }) => append.intern(TyKind::CallablePointee {
            params: params.iter().copied().map(substitute).collect(),
            return_type: substitute(*return_type),
        }),
        Some(TyKind::ClosureState {
            closure_id,
            captures,
            params,
            return_type,
        }) => append.intern(TyKind::ClosureState {
            closure_id: *closure_id,
            captures: captures.iter().copied().map(substitute).collect(),
            params: params.iter().copied().map(substitute).collect(),
            return_type: substitute(*return_type),
        }),
        Some(TyKind::Optional { elem }) => append.intern(TyKind::Optional {
            elem: substitute(*elem),
        }),
        Some(TyKind::ErrorUnion { error, value }) => append.intern(TyKind::ErrorUnion {
            error: substitute(*error),
            value: substitute(*value),
        }),
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => append.intern(TyKind::Nominal {
            def_id: *def_id,
            args: args.iter().copied().map(substitute).collect(),
            const_args: const_args.iter().map(substitute_const_arg).collect(),
        }),
        Some(TyKind::BuiltinTrait { trait_id, args }) => append.intern(TyKind::BuiltinTrait {
            trait_id: *trait_id,
            args: args.iter().copied().map(substitute).collect(),
        }),
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => append.intern(TyKind::TraitObject {
            is_readonly: *is_readonly,
            trait_id: *trait_id,
            trait_args: trait_args.iter().copied().map(substitute).collect(),
            trait_const_args: trait_const_args.iter().map(substitute_const_arg).collect(),
            associated_type_bindings: associated_type_bindings
                .iter()
                .map(substitute_binding)
                .collect(),
        }),
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => append.intern(TyKind::TraitObjectPointee {
            trait_id: *trait_id,
            trait_args: trait_args.iter().copied().map(substitute).collect(),
            trait_const_args: trait_const_args.iter().map(substitute_const_arg).collect(),
            associated_type_bindings: associated_type_bindings
                .iter()
                .map(substitute_binding)
                .collect(),
        }),
        Some(TyKind::Projection {
            self_ty: projection_self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        }) => append.intern(TyKind::Projection {
            self_ty: substitute(*projection_self_ty),
            trait_id: *trait_id,
            trait_args: trait_args.iter().copied().map(substitute).collect(),
            trait_const_args: trait_const_args.iter().map(substitute_const_arg).collect(),
            name: *name,
        }),
        Some(
            TyKind::Error
            | TyKind::ConstOnly
            | TyKind::Opaque
            | TyKind::Primitive(_)
            | TyKind::Vector { .. }
            | TyKind::BuiltinType(_),
        )
        | None => ty,
    }
}

/// Convert a checked const argument into the array-length representation.
///
/// Array lengths are non-negative `u64` values. In particular, a signed
/// `IntConst` must be checked as a signed value before reading its raw bits;
/// otherwise `-1` would become a huge positive length through two's-complement
/// reinterpretation. Type checking normally rejects that argument earlier,
/// but substitution is a persistence/codegen boundary and must remain safe on
/// malformed or stale inputs too.
pub fn array_len_from_const_arg(arg: &ConstGenericArg) -> Option<ArrayLenTy> {
    match &arg.value {
        ConstGenericValue::Int(value) => {
            let value = if value.is_signed() {
                u128::try_from(value.as_i128()?).ok()?
            } else {
                value.bits()
            };
            value.try_into().ok().map(ArrayLenTy::ConstValue)
        }
        ConstGenericValue::GenericParam(name) => Some(ArrayLenTy::GenericParam(*name)),
        ConstGenericValue::ConstExpr(id) => Some(ArrayLenTy::ConstExpr(*id)),
        ConstGenericValue::Bool(_) | ConstGenericValue::Char(_) => None,
    }
}
