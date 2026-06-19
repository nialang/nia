// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use nia_ast::{BindingItem, FunctionItem};
use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendField, BackendFunction, BackendFunctionAttribute,
    BackendGlobal, BackendParam, BackendStruct, BackendUnion,
};
use nia_comptime_check::ComptimeValue;
use nia_defs::DefKind;
use nia_ids::ReceiverKind;
use nia_item_signatures::FunctionAttribute;
use nia_node_id::NodeKey;
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_ty::TyKind;

pub(crate) const SIMPLIFY_STATIC_INIT_PASS: &str = "simplify-static-init";

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_struct(
        &mut self,
        node_key: &NodeKey,
        span: Span,
        item: &nia_ast::StructItem,
    ) -> Option<BackendStruct> {
        let def_id = self.def_id_for_node(node_key, DefKind::Struct)?;
        let signature = self.input.signatures.structs.get(&def_id)?;
        Some(BackendStruct {
            def_id: self.global_def_id(def_id),
            name: item.name.clone(),
            generics: signature.generics.clone(),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name.clone(),
                    ty: field.ty,
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span,
        })
    }

    pub(crate) fn lower_union(
        &mut self,
        node_key: &NodeKey,
        span: Span,
        item: &nia_ast::UnionItem,
    ) -> Option<BackendUnion> {
        let def_id = self.def_id_for_node(node_key, DefKind::Union)?;
        let signature = self.input.signatures.unions.get(&def_id)?;
        Some(BackendUnion {
            def_id: self.global_def_id(def_id),
            name: item.name.clone(),
            generics: signature.generics.clone(),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name.clone(),
                    ty: field.ty,
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span,
        })
    }

    pub(crate) fn lower_enum(
        &mut self,
        node_key: &NodeKey,
        span: Span,
        item: &nia_ast::EnumItem,
    ) -> Option<BackendEnum> {
        let def_id = self.def_id_for_node(node_key, DefKind::Enum)?;
        let signature = self.input.signatures.enums.get(&def_id)?;
        Some(BackendEnum {
            def_id: self.global_def_id(def_id),
            name: item.name.clone(),
            backing_type: signature.backing_type,
            variants: signature
                .variants
                .iter()
                .map(|variant| BackendEnumVariant {
                    def_id: self.global_def_id(variant.def_id),
                    name: variant.name.clone(),
                    value: self
                        .input
                        .comptime
                        .enum_values
                        .get(&variant.def_id)
                        .and_then(|value| match value {
                            ComptimeValue::Int(value) => value.as_i128(),
                            _ => None,
                        }),
                    span: variant.span,
                })
                .collect(),
            span,
        })
    }

    pub(crate) fn lower_global(
        &mut self,
        node_key: &NodeKey,
        span: Span,
        binding: &BindingItem,
    ) -> Option<BackendGlobal> {
        let def_id = self.def_id_for_node(node_key, DefKind::Global)?;
        let global_def_id = self.global_def_id(def_id);
        let signature = self.input.signatures.globals.get(&def_id)?;
        let ty = self
            .input
            .semantic_facts
            .global_types
            .get(&global_def_id)
            .copied()
            .or(signature
                .explicit_type
                .or_else(|| binding.value.as_ref().and_then(|value| self.expr_ty(value))))
            .unwrap_or_else(|| self.error_ty());
        let init = self
            .input
            .body_ir
            .global_inits
            .get(&global_def_id)
            .cloned()
            .map(|init| self.optimize_static_init(global_def_id, init));
        Some(BackendGlobal {
            def_id: global_def_id,
            name: binding.name.clone(),
            ty,
            is_let: signature.is_let,
            is_extern: signature.is_extern,
            init,
            span,
        })
    }

    fn optimize_static_init(
        &mut self,
        global: nia_ids::GlobalDefId,
        init: StaticInit,
    ) -> StaticInit {
        if !self.static_init_simplification_enabled() {
            return init;
        }
        let simplified = simplify_static_init(init.clone());
        if simplified != init {
            self.optimization_report.changed_passes.push(
                crate::BackendOptimizationChange::Global {
                    module_id: self.input.module_id,
                    global,
                    pass: SIMPLIFY_STATIC_INIT_PASS,
                },
            );
        }
        simplified
    }

    fn static_init_simplification_enabled(&self) -> bool {
        crate::static_init_simplification_enabled(&self.optimization)
    }

    pub(crate) fn lower_function(
        &mut self,
        span: Span,
        function: &FunctionItem,
    ) -> Option<BackendFunction> {
        let def_id = self.def_id_for_node_any_function(&function.node_key)?;
        let signature = self.input.signatures.functions.get(&def_id)?;
        if signature.is_comptime {
            return None;
        }
        let global_def_id = self.global_def_id(def_id);
        let instantiation_snapshot = self.instantiation.take_snapshot();
        self.instantiation.set_function_scope(global_def_id, None);
        let effective_generics = self
            .effective_generics(global_def_id, &signature.generics)
            .to_vec();
        let identity_substitutions = effective_generics
            .iter()
            .map(|generic| {
                (
                    generic.clone(),
                    self.type_context
                        .interner
                        .intern(TyKind::GenericParam(generic.clone())),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        let function_body = self
            .input
            .function_bodies
            .get(&global_def_id)
            .cloned()
            .map(|body| {
                self.instantiate_function_body(
                    global_def_id,
                    self.input.module_id,
                    false,
                    0,
                    body,
                    &identity_substitutions,
                )
            });
        let backend_function = Some(BackendFunction {
            def_id: global_def_id,
            name: function.name.clone(),
            generics: effective_generics,
            params: function
                .params
                .iter()
                .zip(signature.params.iter())
                .map(|(param, signature)| {
                    let local_id = self
                        .input
                        .locals
                        .node_local_defs
                        .get(&param.node_key)
                        .copied();
                    let local_ty = if signature.receiver.is_some() {
                        local_id
                            .and_then(|local_id| {
                                self.input
                                    .semantic_facts
                                    .local_types
                                    .get(&local_id)
                                    .copied()
                            })
                            .unwrap_or(signature.ty)
                    } else {
                        signature.ty
                    };
                    let passing_ty = signature
                        .receiver
                        .map(|receiver| self.receiver_passing_ty(receiver, local_ty))
                        .unwrap_or(signature.ty);
                    let substitutions = std::collections::HashMap::new();
                    BackendParam {
                        local_id,
                        name: param.name.clone(),
                        receiver: signature.receiver,
                        passing_ty: self.instantiate_ty(passing_ty, &substitutions),
                        local_ty: self.instantiate_ty(local_ty, &substitutions),
                        span: param.span,
                    }
                })
                .collect(),
            return_type: self
                .instantiate_ty(signature.return_type, &std::collections::HashMap::new()),
            is_extern: signature.is_extern,
            is_variadic: signature.is_variadic,
            attributes: signature
                .attributes
                .iter()
                .map(|attribute| match attribute {
                    FunctionAttribute::Naked => BackendFunctionAttribute::Naked,
                })
                .collect(),
            function_body,
            span,
        });
        self.instantiation.restore(instantiation_snapshot);
        backend_function
    }

    pub(crate) fn receiver_passing_ty(
        &mut self,
        receiver: ReceiverKind,
        local_ty: nia_ids::InternedTyId,
    ) -> nia_ids::InternedTyId {
        match receiver {
            ReceiverKind::Value => local_ty,
            ReceiverKind::RefReadOnly | ReceiverKind::Ref
                if self.is_fat_receiver_local_ty(local_ty) =>
            {
                local_ty
            }
            ReceiverKind::RefReadOnly => self.type_context.interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem: self.receiver_base_ty(local_ty).unwrap_or(local_ty),
            }),
            ReceiverKind::Ref => self.type_context.interner.intern(TyKind::Pointer {
                is_readonly: false,
                elem: self.receiver_base_ty(local_ty).unwrap_or(local_ty),
            }),
        }
    }

    fn receiver_base_ty(&self, ty: nia_ids::InternedTyId) -> Option<nia_ids::InternedTyId> {
        match self.type_context.interner.get(ty) {
            Some(TyKind::Pointer { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    fn is_fat_receiver_local_ty(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.type_context.interner.get(ty),
            Some(TyKind::Slice { .. } | TyKind::TraitObject { .. })
        )
    }
}

fn simplify_static_init(init: StaticInit) -> StaticInit {
    match init {
        StaticInit::Array(elems) => {
            let elems = elems
                .into_iter()
                .map(simplify_static_init)
                .collect::<Vec<_>>();
            if elems.iter().all(is_zero_static_init) {
                StaticInit::Zero
            } else if let Some(first) = elems.first()
                && elems.iter().all(|elem| elem == first)
            {
                StaticInit::Repeat {
                    value: Box::new(first.clone()),
                    count: elems.len() as u64,
                }
            } else {
                StaticInit::Array(elems)
            }
        }
        StaticInit::Repeat { value, count } => {
            let value = simplify_static_init(*value);
            if count == 0 || is_zero_static_init(&value) {
                StaticInit::Zero
            } else {
                StaticInit::Repeat {
                    value: Box::new(value),
                    count,
                }
            }
        }
        StaticInit::Struct(fields) => {
            let fields = fields
                .into_iter()
                .map(|mut field| {
                    field.value = simplify_static_init(field.value);
                    field
                })
                .collect::<Vec<_>>();
            if fields.iter().all(|field| is_zero_static_init(&field.value)) {
                StaticInit::Zero
            } else {
                StaticInit::Struct(fields)
            }
        }
        StaticInit::StaticArrayPointer {
            array_ty,
            array_init,
        } => StaticInit::StaticArrayPointer {
            array_ty,
            array_init: Box::new(simplify_static_init(*array_init)),
        },
        StaticInit::Chars(scalars) if scalars.iter().all(|scalar| *scalar == 0) => StaticInit::Zero,
        StaticInit::Chars(scalars) => {
            repeated_static_init(scalars, StaticInit::Char).unwrap_or_else(StaticInit::Chars)
        }
        StaticInit::Bytes(bytes) if bytes.iter().all(|byte| *byte == 0) => StaticInit::Zero,
        StaticInit::Bytes(bytes) => {
            repeated_static_init(bytes, StaticInit::Byte).unwrap_or_else(StaticInit::Bytes)
        }
        StaticInit::Float(text) if is_zero_float_static_init(&text) => StaticInit::Zero,
        StaticInit::Int(value) if value.bits() == 0 => StaticInit::Zero,
        StaticInit::Bool(false)
        | StaticInit::Char(0)
        | StaticInit::Byte(0)
        | StaticInit::NullPtr
        | StaticInit::Zero => StaticInit::Zero,
        other => other,
    }
}

fn is_zero_static_init(init: &StaticInit) -> bool {
    match init {
        StaticInit::Int(value) if value.bits() == 0 => true,
        StaticInit::Zero
        | StaticInit::Bool(false)
        | StaticInit::Char(0)
        | StaticInit::Byte(0)
        | StaticInit::NullPtr => true,
        StaticInit::Float(text) => is_zero_float_static_init(text),
        StaticInit::Chars(scalars) => scalars.iter().all(|scalar| *scalar == 0),
        StaticInit::Bytes(bytes) => bytes.iter().all(|byte| *byte == 0),
        StaticInit::Array(elems) => elems.iter().all(is_zero_static_init),
        StaticInit::Repeat { value, count } => *count == 0 || is_zero_static_init(value),
        StaticInit::Struct(fields) => fields.iter().all(|field| is_zero_static_init(&field.value)),
        StaticInit::Int(_)
        | StaticInit::Bool(_)
        | StaticInit::Char(_)
        | StaticInit::Byte(_)
        | StaticInit::AddrOfGlobal { .. }
        | StaticInit::AddrOfFunction { .. }
        | StaticInit::StaticArrayPointer { .. } => false,
    }
}

fn repeated_static_init<T>(
    elems: Vec<T>,
    init: impl FnOnce(T) -> StaticInit + Copy,
) -> Result<StaticInit, Vec<T>>
where
    T: Copy + PartialEq,
{
    let Some(first) = elems.first().copied() else {
        return Err(elems);
    };
    if elems.iter().all(|elem| *elem == first) {
        Ok(StaticInit::Repeat {
            value: Box::new(init(first)),
            count: elems.len() as u64,
        })
    } else {
        Err(elems)
    }
}

fn is_zero_float_static_init(text: &str) -> bool {
    nia_literals::eval_float_literal(text) == Ok(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplifies_empty_repeat_static_init_to_zero() {
        let init = StaticInit::Repeat {
            value: Box::new(StaticInit::Int(nia_ty::IntConst::signed(1))),
            count: 0,
        };

        assert_eq!(simplify_static_init(init), StaticInit::Zero);
    }

    #[test]
    fn treats_empty_repeat_static_init_as_zero() {
        let init = StaticInit::Repeat {
            value: Box::new(StaticInit::AddrOfFunction {
                function: nia_ids::GlobalDefId {
                    module_id: nia_ids::ModuleId(0),
                    def_id: nia_ids::DefId(0),
                },
                args: Vec::new(),
            }),
            count: 0,
        };

        assert!(is_zero_static_init(&init));
    }
}
