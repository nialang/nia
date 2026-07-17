use super::*;

pub(super) fn substitute_ty_generics(
    interner: &ConstTypeCx<'_>,
    ty: InternedTyId,
    lookup: &impl Fn(&SymbolId) -> Option<InternedTyId>,
) -> InternedTyId {
    match interner.get(ty).cloned() {
        Some(TyKind::GenericParam(name)) => lookup(&name).unwrap_or(ty),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let elem = substitute_ty_generics(interner, elem, lookup);
            interner.intern(TyKind::Pointer { is_readonly, elem })
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let elem = substitute_ty_generics(interner, elem, lookup);
            interner.intern(TyKind::VolatilePointer { is_readonly, elem })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let elem = substitute_ty_generics(interner, elem, lookup);
            interner.intern(TyKind::Slice { is_readonly, elem })
        }
        Some(TyKind::SlicePointee { elem }) => {
            let elem = substitute_ty_generics(interner, elem, lookup);
            interner.intern(TyKind::SlicePointee { elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let elem = substitute_ty_generics(interner, elem, lookup);
            interner.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::Range { kind, bound }) => {
            let bound = bound.map(|bound| substitute_ty_generics(interner, bound, lookup));
            interner.intern(TyKind::Range { kind, bound })
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .into_iter()
                .map(|param| substitute_ty_generics(interner, param, lookup))
                .collect();
            let return_type = substitute_ty_generics(interner, return_type, lookup);
            interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            })
        }
        Some(TyKind::Optional { elem }) => {
            let elem = substitute_ty_generics(interner, elem, lookup);
            interner.intern(TyKind::Optional { elem })
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = substitute_ty_generics(interner, error, lookup);
            let value = substitute_ty_generics(interner, value, lookup);
            interner.intern(TyKind::ErrorUnion { error, value })
        }
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty_generics(interner, arg, lookup))
                .collect();
            let const_args = const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty_generics(interner, arg.ty, lookup);
                    arg
                })
                .collect();
            interner.intern(TyKind::Nominal {
                def_id,
                args,
                const_args,
            })
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty_generics(interner, arg, lookup))
                .collect();
            interner.intern(TyKind::BuiltinTrait { trait_id, args })
        }
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty_generics(interner, arg, lookup))
                .collect();
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty_generics(interner, arg.ty, lookup);
                    arg
                })
                .collect();
            let associated_type_bindings = associated_type_bindings
                .into_iter()
                .map(|binding| nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .into_iter()
                        .map(|arg| substitute_ty_generics(interner, arg, lookup))
                        .collect(),
                    trait_const_args: binding
                        .trait_const_args
                        .into_iter()
                        .map(|mut arg| {
                            arg.ty = substitute_ty_generics(interner, arg.ty, lookup);
                            arg
                        })
                        .collect(),
                    name: binding.name,
                    ty: substitute_ty_generics(interner, binding.ty, lookup),
                })
                .collect();
            interner.intern(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty_generics(interner, arg, lookup))
                .collect();
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty_generics(interner, arg.ty, lookup);
                    arg
                })
                .collect();
            let associated_type_bindings = associated_type_bindings
                .into_iter()
                .map(|binding| nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .into_iter()
                        .map(|arg| substitute_ty_generics(interner, arg, lookup))
                        .collect(),
                    trait_const_args: binding
                        .trait_const_args
                        .into_iter()
                        .map(|mut arg| {
                            arg.ty = substitute_ty_generics(interner, arg.ty, lookup);
                            arg
                        })
                        .collect(),
                    name: binding.name,
                    ty: substitute_ty_generics(interner, binding.ty, lookup),
                })
                .collect();
            interner.intern(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        }) => {
            let self_ty = substitute_ty_generics(interner, self_ty, lookup);
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty_generics(interner, arg, lookup))
                .collect();
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty_generics(interner, arg.ty, lookup);
                    arg
                })
                .collect();
            interner.intern(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            })
        }
        Some(
            TyKind::Error
            | TyKind::ConstOnly
            | TyKind::SelfParam
            | TyKind::Primitive(_)
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. },
        )
        | None => ty,
    }
}
