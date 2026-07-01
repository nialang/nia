// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, ConstGenericValue, PrimitiveTy, RangeTyKind, TraitId, TyInterner,
    TyKind,
};

pub fn sanitize_symbol_part(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

pub fn mangle_base_symbol(def_id: GlobalDefId, name: &str) -> String {
    format!(
        "nia__m{}__d{}__{}",
        def_id.module_id.0,
        def_id.def_id.0,
        sanitize_symbol_part(name)
    )
}

pub fn mangle_instance_symbol<F, G>(
    def_id: GlobalDefId,
    name: &str,
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
    interner: &TyInterner,
    nominal_name: F,
    array_len: G,
) -> String
where
    F: FnMut(GlobalDefId) -> String,
    G: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let mut nominal_name = nominal_name;
    let mut array_len = array_len;
    let mut parts = args
        .iter()
        .map(|arg| {
            format!(
                "t_{}",
                mangle_type_inner(interner, *arg, &mut nominal_name, &mut array_len)
            )
        })
        .collect::<Vec<_>>();
    parts.extend(const_args.iter().map(|arg| {
        format!(
            "c_{}",
            mangle_const_generic_arg(interner, arg, &mut nominal_name, &mut array_len)
        )
    }));
    if parts.is_empty() {
        mangle_base_symbol(def_id, name)
    } else {
        format!(
            "{}__inst__{}",
            mangle_base_symbol(def_id, name),
            parts.join("__")
        )
    }
}

pub fn mangle_type_with<F, G>(
    interner: &TyInterner,
    ty: InternedTyId,
    nominal_name: F,
    array_len: G,
) -> String
where
    F: FnMut(GlobalDefId) -> String,
    G: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let mut nominal_name = nominal_name;
    let mut array_len = array_len;
    mangle_type_inner(interner, ty, &mut nominal_name, &mut array_len)
}

fn mangle_type_inner<F, G>(
    interner: &TyInterner,
    ty: InternedTyId,
    nominal_name: &mut F,
    array_len: &mut G,
) -> String
where
    F: FnMut(GlobalDefId) -> String,
    G: FnMut(GlobalConstExprId) -> Option<u64>,
{
    match interner.get(ty) {
        Some(TyKind::Primitive(primitive)) => mangle_primitive(*primitive),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let prefix = if *is_readonly { "ptr_read" } else { "ptr" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(interner, *elem, nominal_name, array_len)
            )
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let prefix = if *is_readonly { "vptr_read" } else { "vptr" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(interner, *elem, nominal_name, array_len)
            )
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let prefix = if *is_readonly { "slice" } else { "slice" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(interner, *elem, nominal_name, array_len)
            )
        }
        Some(TyKind::SlicePointee { elem }) => {
            format!(
                "slice_pointee__{}",
                mangle_type_inner(interner, *elem, nominal_name, array_len)
            )
        }
        Some(TyKind::Array { len, elem }) => format!(
            "arr__{}__{}",
            mangle_array_len(len, interner, nominal_name, array_len),
            mangle_type_inner(interner, *elem, nominal_name, array_len)
        ),
        Some(TyKind::Vector { elem, lanes }) => {
            format!("vec__len__{lanes}__{}", mangle_primitive(*elem))
        }
        Some(TyKind::Range { kind, bound }) => {
            let kind = match kind {
                RangeTyKind::Exclusive => "range",
                RangeTyKind::Inclusive => "range_incl",
                RangeTyKind::From => "range_from",
                RangeTyKind::To => "range_to",
                RangeTyKind::ToInclusive => "range_to_incl",
                RangeTyKind::Full => "range_full",
            };
            match bound {
                Some(bound) => format!(
                    "{kind}__{}",
                    mangle_type_inner(interner, *bound, nominal_name, array_len)
                ),
                None => kind.to_string(),
            }
        }
        Some(TyKind::Optional { elem }) => {
            format!(
                "opt__{}",
                mangle_type_inner(interner, *elem, nominal_name, array_len)
            )
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            format!(
                "erru__{}__{}",
                mangle_type_inner(interner, *error, nominal_name, array_len),
                mangle_type_inner(interner, *value, nominal_name, array_len)
            )
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .iter()
                .map(|param| mangle_type_inner(interner, *param, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let mut result = format!(
                "fnptr__pc{}__{}__ret__{}",
                params.len(),
                params,
                mangle_type_inner(interner, *return_type, nominal_name, array_len)
            );
            if *is_variadic {
                result.push_str("__variadic");
            }
            result
        }
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => {
            let base = nominal_name(*def_id);
            if args.is_empty() && const_args.is_empty() {
                format!("nom__{base}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                    .collect::<Vec<_>>()
                    .join("__");
                let const_arg_parts = const_args
                    .iter()
                    .map(|arg| mangle_const_generic_arg(interner, arg, nominal_name, array_len))
                    .collect::<Vec<_>>();
                let const_args = const_arg_parts.join("__");
                format!(
                    "nom__{base}__argc{}__{}__constargc{}__{}",
                    args.len(),
                    args,
                    const_arg_parts.len(),
                    const_args
                )
            }
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let base = sanitize_symbol_part(trait_id.name());
            if args.is_empty() {
                format!("builtin_trait__{base}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                    .collect::<Vec<_>>()
                    .join("__");
                format!("builtin_trait__{base}__argc{}__{}", args.len(), args)
            }
        }
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let prefix = if *is_readonly {
                "trait_obj_read"
            } else {
                "trait_obj"
            };
            let trait_name = match trait_id {
                TraitId::Source(def_id) => nominal_name(*def_id),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| mangle_const_generic_arg(interner, arg, nominal_name, array_len))
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            let assoc_bindings = associated_type_bindings
                .iter()
                .map(|binding| {
                    let trait_part = binding
                        .trait_id
                        .map(|trait_id| match trait_id {
                            TraitId::Source(def_id) => nominal_name(def_id),
                            TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
                        })
                        .unwrap_or_else(|| "self".to_string());
                    let trait_args = binding
                        .trait_args
                        .iter()
                        .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                        .collect::<Vec<_>>()
                        .join("__");
                    let trait_const_arg_parts = binding
                        .trait_const_args
                        .iter()
                        .map(|arg| mangle_const_generic_arg(interner, arg, nominal_name, array_len))
                        .collect::<Vec<_>>();
                    let trait_const_args = trait_const_arg_parts.join("__");
                    format!(
                        "{}__argc{}__{}__cargc{}__{}__{}__{}",
                        trait_part,
                        binding.trait_args.len(),
                        trait_args,
                        trait_const_arg_parts.len(),
                        trait_const_args,
                        sanitize_symbol_part(&binding.name),
                        mangle_type_inner(interner, binding.ty, nominal_name, array_len)
                    )
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "{prefix}__{}__argc{}__{}__cargc{}__{}__assoc{}__{}",
                trait_name,
                trait_args.len(),
                trait_args,
                trait_const_arg_parts.len(),
                trait_const_args,
                associated_type_bindings.len(),
                assoc_bindings
            )
        }
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_name = match trait_id {
                TraitId::Source(def_id) => nominal_name(*def_id),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| mangle_const_generic_arg(interner, arg, nominal_name, array_len))
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            let assoc_bindings = associated_type_bindings
                .iter()
                .map(|binding| {
                    let trait_part = binding
                        .trait_id
                        .map(|trait_id| match trait_id {
                            TraitId::Source(def_id) => nominal_name(def_id),
                            TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
                        })
                        .unwrap_or_else(|| "self".to_string());
                    let trait_args = binding
                        .trait_args
                        .iter()
                        .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                        .collect::<Vec<_>>()
                        .join("__");
                    let trait_const_arg_parts = binding
                        .trait_const_args
                        .iter()
                        .map(|arg| mangle_const_generic_arg(interner, arg, nominal_name, array_len))
                        .collect::<Vec<_>>();
                    let trait_const_args = trait_const_arg_parts.join("__");
                    format!(
                        "{}__argc{}__{}__cargc{}__{}__{}__{}",
                        trait_part,
                        binding.trait_args.len(),
                        trait_args,
                        trait_const_arg_parts.len(),
                        trait_const_args,
                        sanitize_symbol_part(&binding.name),
                        mangle_type_inner(interner, binding.ty, nominal_name, array_len)
                    )
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "trait_obj_pointee__{}__argc{}__{}__cargc{}__{}__assoc{}__{}",
                trait_name,
                trait_args.len(),
                trait_args,
                trait_const_arg_parts.len(),
                trait_const_args,
                associated_type_bindings.len(),
                assoc_bindings
            )
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        }) => {
            let self_ty = mangle_type_inner(interner, *self_ty, nominal_name, array_len);
            let trait_name = match trait_id {
                TraitId::Source(def_id) => nominal_name(*def_id),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| mangle_const_generic_arg(interner, arg, nominal_name, array_len))
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            format!(
                "proj__{}__as__{}__argc{}__{}__cargc{}__{}__{}",
                self_ty,
                trait_name,
                trait_args.len(),
                trait_args,
                trait_const_arg_parts.len(),
                trait_const_args,
                sanitize_symbol_part(name)
            )
        }
        Some(TyKind::GenericParam(name)) => format!("gen__{}", sanitize_symbol_part(name)),
        Some(TyKind::ComptimeOnly) => "comptime_only".to_string(),
        Some(TyKind::Error) => "ty_error".to_string(),
        None => panic!(
            "Nia ICE: cannot mangle type {:?} with interner {:?}",
            ty,
            interner.interner_id()
        ),
    }
}

fn mangle_array_len<F, G>(
    len: &ArrayLenTy,
    interner: &TyInterner,
    nominal_name: &mut F,
    array_len: &mut G,
) -> String
where
    F: FnMut(GlobalDefId) -> String,
    G: FnMut(GlobalConstExprId) -> Option<u64>,
{
    match len {
        ArrayLenTy::Infer => "infer".to_string(),
        ArrayLenTy::GenericParam(name) => format!("gen_len__{}", sanitize_symbol_part(name)),
        ArrayLenTy::ConstValue(value) => format!("len__{value}"),
        ArrayLenTy::ConstExpr(id) => array_len(*id)
            .map(|value| format!("len__{value}"))
            // Keep mangling total so later phases can keep reporting errors.
            // The unresolved marker is stable and cannot collide with a valid
            // evaluated length.
            .unwrap_or_else(|| {
                format!(
                    "len_unresolved__m{}__c{}",
                    id.module_id.0, id.const_expr_id.0
                )
            }),
        ArrayLenTy::Builtin { builtin, ty } => format!(
            "builtin__{}__{}",
            sanitize_symbol_part(builtin.name()),
            mangle_type_inner(interner, *ty, nominal_name, array_len)
        ),
    }
}

fn mangle_const_generic_arg<F, G>(
    interner: &TyInterner,
    arg: &ConstGenericArg,
    nominal_name: &mut F,
    array_len: &mut G,
) -> String
where
    F: FnMut(GlobalDefId) -> String,
    G: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let ty = mangle_type_inner(interner, arg.ty, nominal_name, array_len);
    let value = match &arg.value {
        ConstGenericValue::GenericParam(name) => format!("g{}", sanitize_symbol_part(name)),
        ConstGenericValue::ConstExpr(id) => array_len(*id)
            .map(|value| format!("expr_len__{value}"))
            .unwrap_or_else(|| {
                format!(
                    "expr_unresolved__m{}__c{}",
                    id.module_id.0, id.const_expr_id.0
                )
            }),
        ConstGenericValue::Int(value) => {
            let sign = if value.is_signed() { "i" } else { "u" };
            format!("{sign}{}", value.bits())
        }
        ConstGenericValue::Bool(value) => format!("b{}", u8::from(*value)),
        ConstGenericValue::Char(value) => format!("c{}", *value as u32),
    };
    format!("const__{ty}__{value}")
}

fn mangle_primitive(primitive: PrimitiveTy) -> String {
    match primitive {
        PrimitiveTy::Never => "never".to_string(),
        _ => primitive.name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{ModuleId, TyInternerIndex};

    #[test]
    fn mangles_real_error_type_for_diagnostic_recovery() {
        let interner = TyInterner::new(ModuleId(0));

        assert_eq!(
            mangle_type_with(&interner, interner.error(), |_| "item".into(), |_| None),
            "ty_error"
        );
    }

    #[test]
    #[should_panic(expected = "Nia ICE: cannot mangle type")]
    fn rejects_missing_type_id_instead_of_mangling_fallback_symbol() {
        let interner = TyInterner::new(ModuleId(0));
        let missing = InternedTyId::new(
            interner.interner_id(),
            TyInternerIndex::from_interner_index(999),
        );

        let _ = mangle_type_with(&interner, missing, |_| "item".into(), |_| None);
    }
}
