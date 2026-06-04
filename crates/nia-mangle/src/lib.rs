// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId};
use nia_ty::{ArrayLenTy, PrimitiveTy, RangeTyKind, TraitId, TyInterner, TyKind};

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
    let args = args
        .iter()
        .map(|arg| {
            format!(
                "t_{}",
                mangle_type_inner(interner, *arg, &mut nominal_name, &mut array_len)
            )
        })
        .collect::<Vec<_>>()
        .join("__");
    if args.is_empty() {
        mangle_base_symbol(def_id, name)
    } else {
        format!("{}__inst__{}", mangle_base_symbol(def_id, name), args)
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
        Some(TyKind::Pointer { is_const, elem }) => {
            let prefix = if *is_const { "ptr_const" } else { "ptr" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(interner, *elem, nominal_name, array_len)
            )
        }
        Some(TyKind::Slice { is_const, elem }) => {
            let prefix = if *is_const { "slice_const" } else { "slice" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(interner, *elem, nominal_name, array_len)
            )
        }
        Some(TyKind::Array { len, elem }) => format!(
            "arr__{}__{}",
            mangle_array_len(len, interner, nominal_name, array_len),
            mangle_type_inner(interner, *elem, nominal_name, array_len)
        ),
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
        Some(TyKind::Nominal { def_id, args }) => {
            let base = nominal_name(*def_id);
            if args.is_empty() {
                format!("nom__{base}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| mangle_type_inner(interner, *arg, nominal_name, array_len))
                    .collect::<Vec<_>>()
                    .join("__");
                format!("nom__{base}__argc{}__{}", args.len(), args)
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
            is_const,
            trait_id,
            trait_args,
            associated_type_bindings,
        }) => {
            let prefix = if *is_const {
                "trait_obj_const"
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
                    format!(
                        "{}__argc{}__{}__{}__{}",
                        trait_part,
                        binding.trait_args.len(),
                        trait_args,
                        sanitize_symbol_part(&binding.name),
                        mangle_type_inner(interner, binding.ty, nominal_name, array_len)
                    )
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "{prefix}__{}__argc{}__{}__assoc{}__{}",
                trait_name,
                trait_args.len(),
                trait_args,
                associated_type_bindings.len(),
                assoc_bindings
            )
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
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
            format!(
                "proj__{}__as__{}__argc{}__{}__{}",
                self_ty,
                trait_name,
                trait_args.len(),
                trait_args,
                sanitize_symbol_part(name)
            )
        }
        Some(TyKind::GenericParam(name)) => format!("gen__{}", sanitize_symbol_part(name)),
        Some(TyKind::Error) | None => "ty_error".to_string(),
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

fn mangle_primitive(primitive: PrimitiveTy) -> String {
    match primitive {
        PrimitiveTy::I8 => "i8".to_string(),
        PrimitiveTy::I16 => "i16".to_string(),
        PrimitiveTy::I32 => "i32".to_string(),
        PrimitiveTy::I64 => "i64".to_string(),
        PrimitiveTy::I128 => "i128".to_string(),
        PrimitiveTy::Isize => "isize".to_string(),
        PrimitiveTy::U8 => "u8".to_string(),
        PrimitiveTy::U16 => "u16".to_string(),
        PrimitiveTy::U32 => "u32".to_string(),
        PrimitiveTy::U64 => "u64".to_string(),
        PrimitiveTy::U128 => "u128".to_string(),
        PrimitiveTy::Usize => "usize".to_string(),
        PrimitiveTy::F32 => "f32".to_string(),
        PrimitiveTy::F64 => "f64".to_string(),
        PrimitiveTy::Bool => "bool".to_string(),
        PrimitiveTy::Char => "char".to_string(),
        PrimitiveTy::Void => "void".to_string(),
        PrimitiveTy::Never => "never".to_string(),
    }
}
