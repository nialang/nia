// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{ClosureId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_symbol::{SymbolId, stable_hash};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, ConstGenericValue, PrimitiveTy, RangeTyKind, TraitId, TyKind,
    TypeStore,
};

pub struct MangleResolvers<F, G, H> {
    module_id: F,
    nominal_name: G,
    array_len: H,
}

impl<F, G, H> MangleResolvers<F, G, H> {
    pub fn new(module_id: F, nominal_name: G, array_len: H) -> Self {
        Self {
            module_id,
            nominal_name,
            array_len,
        }
    }
}

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

pub fn mangle_symbol_id(symbol: SymbolId) -> String {
    format!("sym_{:016x}", symbol.raw())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MangleModuleId(u64);

impl MangleModuleId {
    pub const fn from_normalized_source_path(path: &str) -> Self {
        Self(stable_hash(path))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

pub fn mangle_base_symbol(def_id: GlobalDefId, module: MangleModuleId, name: &str) -> String {
    format!(
        "nia__s{:016x}__d{}__{}",
        module.0,
        def_id.def_id.0,
        sanitize_symbol_part(name)
    )
}

pub fn mangle_base_symbol_id(
    def_id: GlobalDefId,
    module: MangleModuleId,
    name: SymbolId,
) -> String {
    mangle_base_symbol(def_id, module, &mangle_symbol_id(name))
}

/// Derives the generated entry symbol for a closure from its concrete owner
/// symbol. Passing the already-instantiated owner symbol keeps entries from
/// distinct generic function instances disjoint without inventing synthetic
/// source definition ids.
pub fn mangle_closure_entry_symbol(owner_symbol: &str, closure_id: ClosureId) -> String {
    format!(
        "{}__closure_entry__ord__{}",
        sanitize_symbol_part(owner_symbol),
        closure_id.ordinal
    )
}

pub fn mangle_instance_symbol_id<F, G, H>(
    def_id: GlobalDefId,
    name: SymbolId,
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
    type_store: &TypeStore,
    resolvers: MangleResolvers<F, G, H>,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    mangle_instance_symbol(
        def_id,
        &mangle_symbol_id(name),
        args,
        const_args,
        type_store,
        resolvers,
    )
}

pub fn mangle_instance_symbol<F, G, H>(
    def_id: GlobalDefId,
    name: &str,
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
    type_store: &TypeStore,
    resolvers: MangleResolvers<F, G, H>,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let MangleResolvers {
        mut module_id,
        mut nominal_name,
        mut array_len,
    } = resolvers;
    let mut parts = args
        .iter()
        .map(|arg| {
            format!(
                "t_{}",
                mangle_type_inner(
                    type_store,
                    *arg,
                    &mut module_id,
                    &mut nominal_name,
                    &mut array_len,
                )
            )
        })
        .collect::<Vec<_>>();
    parts.extend(const_args.iter().map(|arg| {
        format!(
            "c_{}",
            mangle_const_generic_arg(
                type_store,
                arg,
                &mut module_id,
                &mut nominal_name,
                &mut array_len,
            )
        )
    }));
    if parts.is_empty() {
        mangle_base_symbol(def_id, module_id(def_id.module_id), name)
    } else {
        format!(
            "{}__inst__{}",
            mangle_base_symbol(def_id, module_id(def_id.module_id), name),
            parts.join("__")
        )
    }
}

pub fn mangle_type_with<F, G, H>(
    type_store: &TypeStore,
    ty: InternedTyId,
    resolvers: MangleResolvers<F, G, H>,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let MangleResolvers {
        mut module_id,
        mut nominal_name,
        mut array_len,
    } = resolvers;
    mangle_type_inner(
        type_store,
        ty,
        &mut module_id,
        &mut nominal_name,
        &mut array_len,
    )
}

fn mangle_type_inner<F, G, H>(
    type_store: &TypeStore,
    ty: InternedTyId,
    module_id: &mut F,
    nominal_name: &mut G,
    array_len: &mut H,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    match type_store.get(ty) {
        Some(TyKind::Opaque) => "opaque".to_string(),
        Some(TyKind::Primitive(primitive)) => mangle_primitive(*primitive),
        Some(TyKind::Tuple(elems)) => {
            let arity = elems.len();
            let encoded_elems = elems
                .iter()
                .map(|elem| {
                    mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            format!("tuple__len__{arity}__{encoded_elems}")
        }
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let prefix = if *is_readonly { "ptr_read" } else { "ptr" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let prefix = if *is_readonly { "vptr_read" } else { "vptr" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let prefix = if *is_readonly { "slice_read" } else { "slice" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::SlicePointee { elem }) => {
            format!(
                "slice_pointee__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::Array { len, elem }) => format!(
            "arr__{}__{}",
            mangle_array_len(len, type_store, module_id, nominal_name, array_len),
            mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
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
                    mangle_type_inner(type_store, *bound, module_id, nominal_name, array_len)
                ),
                None => kind.to_string(),
            }
        }
        Some(TyKind::Optional { elem }) => {
            format!(
                "opt__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            format!(
                "erru__{}__{}",
                mangle_type_inner(type_store, *error, module_id, nominal_name, array_len),
                mangle_type_inner(type_store, *value, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let param_count = params.len();
            let params = params
                .iter()
                .map(|param| {
                    mangle_type_inner(type_store, *param, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            let mut result = format!(
                "fnptr__pc{}__{}__ret__{}",
                param_count,
                params,
                mangle_type_inner(type_store, *return_type, module_id, nominal_name, array_len)
            );
            if *is_variadic {
                result.push_str("__variadic");
            }
            result
        }
        Some(TyKind::Callable {
            is_readonly,
            params,
            return_type,
        }) => {
            let param_count = params.len();
            let prefix = if *is_readonly {
                "callable_read"
            } else {
                "callable"
            };
            let params = params
                .iter()
                .map(|param| {
                    mangle_type_inner(type_store, *param, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "{prefix}__pc{}__{}__ret__{}",
                param_count,
                params,
                mangle_type_inner(type_store, *return_type, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::CallablePointee {
            params,
            return_type,
        }) => {
            let param_count = params.len();
            let params = params
                .iter()
                .map(|param| {
                    mangle_type_inner(type_store, *param, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "callable_pointee__pc{}__{}__ret__{}",
                param_count,
                params,
                mangle_type_inner(type_store, *return_type, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::ClosureState { closure_id, .. }) => {
            let owner = mangle_source_def(closure_id.owner, module_id, nominal_name);
            format!("closure__{owner}__ord__{}", closure_id.ordinal)
        }
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => {
            let base = mangle_source_def(*def_id, module_id, nominal_name);
            if args.is_empty() && const_args.is_empty() {
                format!("nom__{base}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| {
                        mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                    })
                    .collect::<Vec<_>>()
                    .join("__");
                let const_arg_parts = const_args
                    .iter()
                    .map(|arg| {
                        mangle_const_generic_arg(
                            type_store,
                            arg,
                            module_id,
                            nominal_name,
                            array_len,
                        )
                    })
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
        Some(TyKind::BuiltinType(builtin)) => {
            format!("builtin_type__{}", sanitize_symbol_part(builtin.name()))
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let base = sanitize_symbol_part(trait_id.name());
            if args.is_empty() {
                format!("builtin_trait__{base}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| {
                        mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                    })
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
                TraitId::Source(def_id) => mangle_source_def(*def_id, module_id, nominal_name),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| {
                    mangle_const_generic_arg(type_store, arg, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            let assoc_bindings = associated_type_bindings
                .iter()
                .map(|binding| {
                    let trait_part = binding
                        .trait_id
                        .map(|trait_id| match trait_id {
                            TraitId::Source(def_id) => {
                                mangle_source_def(def_id, module_id, nominal_name)
                            }
                            TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
                        })
                        .unwrap_or_else(|| "self".to_string());
                    let trait_args = binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                        })
                        .collect::<Vec<_>>()
                        .join("__");
                    let trait_const_arg_parts = binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            mangle_const_generic_arg(
                                type_store,
                                arg,
                                module_id,
                                nominal_name,
                                array_len,
                            )
                        })
                        .collect::<Vec<_>>();
                    let trait_const_args = trait_const_arg_parts.join("__");
                    format!(
                        "{}__argc{}__{}__cargc{}__{}__{}__{}",
                        trait_part,
                        binding.trait_args.len(),
                        trait_args,
                        trait_const_arg_parts.len(),
                        trait_const_args,
                        mangle_symbol_id(binding.name),
                        mangle_type_inner(
                            type_store,
                            binding.ty,
                            module_id,
                            nominal_name,
                            array_len,
                        )
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
                TraitId::Source(def_id) => mangle_source_def(*def_id, module_id, nominal_name),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| {
                    mangle_const_generic_arg(type_store, arg, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            let assoc_bindings = associated_type_bindings
                .iter()
                .map(|binding| {
                    let trait_part = binding
                        .trait_id
                        .map(|trait_id| match trait_id {
                            TraitId::Source(def_id) => {
                                mangle_source_def(def_id, module_id, nominal_name)
                            }
                            TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
                        })
                        .unwrap_or_else(|| "self".to_string());
                    let trait_args = binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                        })
                        .collect::<Vec<_>>()
                        .join("__");
                    let trait_const_arg_parts = binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            mangle_const_generic_arg(
                                type_store,
                                arg,
                                module_id,
                                nominal_name,
                                array_len,
                            )
                        })
                        .collect::<Vec<_>>();
                    let trait_const_args = trait_const_arg_parts.join("__");
                    format!(
                        "{}__argc{}__{}__cargc{}__{}__{}__{}",
                        trait_part,
                        binding.trait_args.len(),
                        trait_args,
                        trait_const_arg_parts.len(),
                        trait_const_args,
                        mangle_symbol_id(binding.name),
                        mangle_type_inner(
                            type_store,
                            binding.ty,
                            module_id,
                            nominal_name,
                            array_len,
                        )
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
            let self_ty =
                mangle_type_inner(type_store, *self_ty, module_id, nominal_name, array_len);
            let trait_name = match trait_id {
                TraitId::Source(def_id) => mangle_source_def(*def_id, module_id, nominal_name),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| {
                    mangle_const_generic_arg(type_store, arg, module_id, nominal_name, array_len)
                })
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
                mangle_symbol_id(*name)
            )
        }
        Some(TyKind::GenericParam(name)) => format!("gen__{}", mangle_symbol_id(*name)),
        Some(TyKind::SelfParam) => "self_param".to_string(),
        Some(TyKind::ConstOnly) => "const_only".to_string(),
        Some(TyKind::Error) => "ty_error".to_string(),
        None => panic!(
            "Nia ICE: cannot mangle type {:?} with type_store {:?}",
            ty,
            type_store.id()
        ),
    }
}

fn mangle_source_def<F, G>(def_id: GlobalDefId, module_id: &mut F, nominal_name: &mut G) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
{
    format!(
        "s{:016x}__d{}__{}",
        module_id(def_id.module_id).0,
        def_id.def_id.0,
        nominal_name(def_id)
    )
}

fn mangle_array_len<F, G, H>(
    len: &ArrayLenTy,
    type_store: &TypeStore,
    module_id: &mut F,
    nominal_name: &mut G,
    array_len: &mut H,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    match len {
        ArrayLenTy::Infer => "infer".to_string(),
        ArrayLenTy::GenericParam(name) => format!("gen_len__{}", mangle_symbol_id(*name)),
        ArrayLenTy::ConstValue(value) => format!("len__{value}"),
        ArrayLenTy::ConstExpr(id) => array_len(*id)
            .map(|value| format!("len__{value}"))
            // Keep mangling total so later phases can keep reporting errors.
            // The unresolved marker is stable and cannot collide with a valid
            // evaluated length.
            .unwrap_or_else(|| {
                format!(
                    "len_unresolved__s{:016x}__c{}",
                    module_id(id.module_id).0,
                    id.const_expr_id.0
                )
            }),
        ArrayLenTy::Builtin { builtin, ty } => format!(
            "builtin__{}__{}",
            sanitize_symbol_part(builtin.name()),
            mangle_type_inner(type_store, *ty, module_id, nominal_name, array_len)
        ),
    }
}

fn mangle_const_generic_arg<F, G, H>(
    type_store: &TypeStore,
    arg: &ConstGenericArg,
    module_id: &mut F,
    nominal_name: &mut G,
    array_len: &mut H,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let ty = mangle_type_inner(type_store, arg.ty, module_id, nominal_name, array_len);
    let value = match &arg.value {
        ConstGenericValue::GenericParam(name) => format!("g{}", mangle_symbol_id(*name)),
        ConstGenericValue::ConstExpr(id) => array_len(*id)
            .map(|value| format!("expr_len__{value}"))
            .unwrap_or_else(|| {
                format!(
                    "expr_unresolved__s{:016x}__c{}",
                    module_id(id.module_id).0,
                    id.const_expr_id.0
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
    use nia_ids::{DefId, ModuleIdAllocator, TypeStoreIndex};

    #[test]
    fn base_mangling_is_stable_across_module_allocator_universes() {
        let mut first_ids = ModuleIdAllocator::new();
        let first_module = first_ids.allocate();
        let mut second_ids = ModuleIdAllocator::new();
        let _unrelated_module = second_ids.allocate();
        let second_module = second_ids.allocate();
        let stable_module = MangleModuleId::from_normalized_source_path("std/error.nia");

        assert_eq!(
            mangle_base_symbol(
                GlobalDefId {
                    module_id: first_module,
                    def_id: DefId(7),
                },
                stable_module,
                "Error",
            ),
            mangle_base_symbol(
                GlobalDefId {
                    module_id: second_module,
                    def_id: DefId(7),
                },
                stable_module,
                "Error",
            )
        );
    }

    #[test]
    fn nominal_mangling_is_stable_across_module_allocator_universes() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let first_module = module_ids.allocate();
        let _unrelated_module = module_ids.allocate();
        let second_module = module_ids.allocate();
        let first = type_store
            .append_for_module(first_module)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id: first_module,
                    def_id: DefId(7),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            });
        let second = type_store
            .append_for_module(second_module)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id: second_module,
                    def_id: DefId(7),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            });

        let first = mangle_type_with(
            &type_store,
            first,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("std/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        let second = mangle_type_with(
            &type_store,
            second,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("std/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        assert_eq!(first, second);
    }

    #[test]
    fn nominal_mangling_distinguishes_source_identities() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let ty = type_store
            .append_for_module(module_id)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(7),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            });

        let first = mangle_type_with(
            &type_store,
            ty,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("first/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        let second = mangle_type_with(
            &type_store,
            ty,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("second/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        assert_ne!(first, second);
    }

    #[test]
    fn closure_entry_mangling_uses_concrete_owner_symbol_and_ordinal() {
        use nia_ids::{DefId, ModuleIdAllocator};

        let module_id = ModuleIdAllocator::new().allocate();
        let closure_id = ClosureId {
            owner: GlobalDefId {
                module_id,
                def_id: DefId(7),
            },
            ordinal: 2,
        };
        let source = mangle_closure_entry_symbol("nia__owner", closure_id);
        let instance = mangle_closure_entry_symbol("nia__owner__inst__t_i32", closure_id);

        assert_eq!(source, "nia__owner__closure_entry__ord__2");
        assert_eq!(instance, "nia__owner__inst__t_i32__closure_entry__ord__2");
        assert_ne!(source, instance);
        assert_ne!(
            source,
            mangle_closure_entry_symbol(
                "nia__owner",
                ClosureId {
                    ordinal: 3,
                    ..closure_id
                }
            )
        );
    }

    #[test]
    fn mangles_real_error_type_for_diagnostic_recovery() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let error = type_store.append_for_module(module_ids.allocate()).error();

        assert_eq!(
            mangle_type_with(
                &type_store,
                error,
                MangleResolvers::new(
                    |_| MangleModuleId::from_normalized_source_path("main.nia"),
                    |_| "item".into(),
                    |_| None,
                ),
            ),
            "ty_error"
        );
    }

    #[test]
    fn tuple_mangling_preserves_unit_arity_and_element_order() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let append = type_store.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let bool_ty = append.primitive(PrimitiveTy::Bool);
        let unit = append.intern(TyKind::Tuple(Vec::new()));
        let pair = append.intern(TyKind::Tuple(vec![i32_ty, bool_ty]));
        let reversed = append.intern(TyKind::Tuple(vec![bool_ty, i32_ty]));
        let resolvers = || {
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            )
        };

        assert_eq!(
            mangle_type_with(&type_store, unit, resolvers()),
            "tuple__len__0__"
        );
        assert_eq!(
            mangle_type_with(&type_store, pair, resolvers()),
            "tuple__len__2__i32__bool"
        );
        assert_eq!(
            mangle_type_with(&type_store, reversed, resolvers()),
            "tuple__len__2__bool__i32"
        );
    }

    #[test]
    fn function_and_callable_mangling_preserve_arity_mutability_and_signature_order() {
        let type_store = TypeStore::new();
        let module_id = ModuleIdAllocator::new().allocate();
        let append = type_store.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let bool_ty = append.primitive(PrimitiveTy::Bool);
        let nullary_function = append.intern(TyKind::FunctionPointer {
            params: Vec::new(),
            return_type: i32_ty,
            is_variadic: false,
        });
        let binary_function = append.intern(TyKind::FunctionPointer {
            params: vec![i32_ty, bool_ty],
            return_type: i32_ty,
            is_variadic: false,
        });
        let readonly = append.intern(TyKind::Callable {
            is_readonly: true,
            params: vec![i32_ty, bool_ty],
            return_type: i32_ty,
        });
        let mutable = append.intern(TyKind::Callable {
            is_readonly: false,
            params: vec![i32_ty, bool_ty],
            return_type: i32_ty,
        });
        let pointee = append.intern(TyKind::CallablePointee {
            params: vec![bool_ty, i32_ty],
            return_type: i32_ty,
        });
        let resolvers = || {
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            )
        };

        assert_eq!(
            mangle_type_with(&type_store, nullary_function, resolvers()),
            "fnptr__pc0____ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, binary_function, resolvers()),
            "fnptr__pc2__i32__bool__ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, readonly, resolvers()),
            "callable_read__pc2__i32__bool__ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, mutable, resolvers()),
            "callable__pc2__i32__bool__ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, pointee, resolvers()),
            "callable_pointee__pc2__bool__i32__ret__i32"
        );
    }

    #[test]
    #[should_panic(expected = "Nia ICE: cannot mangle type")]
    fn rejects_missing_type_id_instead_of_mangling_fallback_symbol() {
        let type_store = TypeStore::new();
        let missing = InternedTyId::new(type_store.id(), TypeStoreIndex::from_store_index(999));

        let _ = mangle_type_with(
            &type_store,
            missing,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            ),
        );
    }
}
