// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId};
use nia_ty::{ArrayLenTy, PrimitiveTy, TyInterner, TyKind};

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
    G: FnMut(GlobalConstExprId) -> u64,
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
    G: FnMut(GlobalConstExprId) -> u64,
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
    G: FnMut(GlobalConstExprId) -> u64,
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
    G: FnMut(GlobalConstExprId) -> u64,
{
    match len {
        ArrayLenTy::Infer => "infer".to_string(),
        ArrayLenTy::ConstValue(value) => format!("len__{value}"),
        ArrayLenTy::ConstExpr(id) => format!("len__{}", array_len(*id)),
        ArrayLenTy::Builtin { name, ty } => format!(
            "builtin__{}__{}",
            sanitize_symbol_part(name),
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
