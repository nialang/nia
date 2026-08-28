// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use nia_ast::{BindingItem, FunctionItem};
use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendField, BackendFunction, BackendFunctionAttribute,
    BackendGlobal, BackendParam, BackendStruct, BackendUnion,
};
use nia_const_check::ConstValue;
use nia_defs::DefKind;
use nia_ids::ReceiverKind;
use nia_item_signatures::FunctionAttribute;
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_symbol::SymbolMap;
use nia_ty::TyKind;

pub(crate) const SIMPLIFY_STATIC_INIT_PASS: &str = "simplify-static-init";

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_struct(
        &mut self,
        node_key: &VersionedNodeKey,
        span: Span,
        item: &nia_ast::StructItem,
    ) -> Option<BackendStruct> {
        let def_id = self.def_id_for_node(node_key, DefKind::Struct)?;
        let signature = self.input.signatures.structs.get(&def_id)?;
        let substitutions = SymbolMap::default();
        Some(BackendStruct {
            def_id: self.global_def_id(def_id),
            name: item.name,
            generics: signature.generics.clone(),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name,
                    ty: self.instantiate_ty(field.ty, &substitutions),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span,
        })
    }

    pub(crate) fn lower_union(
        &mut self,
        node_key: &VersionedNodeKey,
        span: Span,
        item: &nia_ast::UnionItem,
    ) -> Option<BackendUnion> {
        let def_id = self.def_id_for_node(node_key, DefKind::Union)?;
        let signature = self.input.signatures.unions.get(&def_id)?;
        let substitutions = SymbolMap::default();
        Some(BackendUnion {
            def_id: self.global_def_id(def_id),
            name: item.name,
            generics: signature.generics.clone(),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name,
                    ty: self.instantiate_ty(field.ty, &substitutions),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span,
        })
    }

    pub(crate) fn lower_enum(
        &mut self,
        node_key: &VersionedNodeKey,
        span: Span,
        item: &nia_ast::EnumItem,
    ) -> Option<BackendEnum> {
        let def_id = self.def_id_for_node(node_key, DefKind::Enum)?;
        let signature = self.input.signatures.enums.get(&def_id)?;
        let substitutions = SymbolMap::default();
        Some(BackendEnum {
            def_id: self.global_def_id(def_id),
            name: item.name,
            backing_type: self.instantiate_ty(signature.backing_type, &substitutions),
            variants: signature
                .variants
                .iter()
                .map(|variant| BackendEnumVariant {
                    def_id: self.global_def_id(variant.def_id),
                    name: variant.name,
                    value: self
                        .input
                        .const_enum_values
                        .get(&variant.def_id)
                        .and_then(|value| match value {
                            ConstValue::Int(value) => value.as_i128(),
                            _ => None,
                        }),
                    payload: match &variant.payload {
                        nia_item_signatures::EnumVariantPayloadSignature::Unit => {
                            nia_backend_ir::BackendEnumVariantPayload::Unit
                        }
                        nia_item_signatures::EnumVariantPayloadSignature::Tuple(fields) => {
                            nia_backend_ir::BackendEnumVariantPayload::Tuple(
                                fields
                                    .iter()
                                    .map(|ty| self.instantiate_ty(*ty, &substitutions))
                                    .collect(),
                            )
                        }
                        nia_item_signatures::EnumVariantPayloadSignature::Named(fields) => {
                            nia_backend_ir::BackendEnumVariantPayload::Named(
                                fields
                                    .iter()
                                    .map(|field| BackendField {
                                        def_id: self.global_def_id(field.def_id),
                                        name: field.name,
                                        ty: self.instantiate_ty(field.ty, &substitutions),
                                        span: field.span,
                                    })
                                    .collect(),
                            )
                        }
                    },
                    span: variant.span,
                })
                .collect(),
            span,
        })
    }

    pub(crate) fn lower_global(
        &mut self,
        node_key: &VersionedNodeKey,
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
        let ty = self.instantiate_ty(ty, &SymbolMap::default());
        let init = self
            .input
            .program
            .static_init(global_def_id)
            .cloned()
            .map(|init| self.optimize_static_init(global_def_id, init));
        Some(BackendGlobal {
            def_id: global_def_id,
            name: binding.name,
            link_name: signature.is_extern.then(|| self.symbol_name(binding.name)),
            ty,
            is_let: !signature.is_mutable,
            is_extern: signature.is_extern,
            init,
            span,
        })
    }

    pub(crate) fn lower_global_from_static_init(
        &mut self,
        global_def_id: nia_ids::GlobalDefId,
    ) -> Option<BackendGlobal> {
        let def = self.input.defs.defs.get(global_def_id.def_id)?;
        let signature = self.input.signatures.globals.get(&global_def_id.def_id);
        let ty = self
            .input
            .semantic_facts
            .global_types
            .get(&global_def_id)
            .copied()
            .or_else(|| signature.and_then(|signature| signature.explicit_type))
            .unwrap_or_else(|| self.error_ty());
        let ty = self.instantiate_ty(ty, &SymbolMap::default());
        let init = self
            .input
            .program
            .static_init(global_def_id)
            .cloned()
            .map(|init| self.optimize_static_init(global_def_id, init));
        Some(BackendGlobal {
            def_id: global_def_id,
            name: def.name,
            link_name: signature
                .is_some_and(|signature| signature.is_extern)
                .then(|| self.symbol_name(def.name)),
            ty,
            is_let: signature.is_none_or(|signature| !signature.is_mutable),
            is_extern: signature.is_some_and(|signature| signature.is_extern),
            init,
            span: def.span,
        })
    }

    pub(crate) fn optimize_static_init(
        &mut self,
        global: nia_ids::GlobalDefId,
        init: StaticInit,
    ) -> StaticInit {
        if !self.static_init_simplification_enabled() {
            return init;
        }
        let (simplified, changed) = simplify_static_init(init);
        if changed {
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
        if !signature.is_extern && !signature.has_body {
            return None;
        }
        let global_def_id = self.global_def_id(def_id);
        // Syntax parameters own source names/spans while signature parameters own ABI types.
        // Lowering may zip them only after proving they still describe the same sequence;
        // otherwise a stale phase product would silently create a truncated backend ABI.
        if function.params.len() != signature.params.len() {
            self.report_backend_function_param_count_mismatch(
                global_def_id,
                span,
                function.params.len(),
                signature.params.len(),
            );
            return None;
        }
        if !signature.is_extern && self.input.program.function_body(global_def_id).is_none() {
            return None;
        }
        let instantiation_snapshot = self.instantiation.take_snapshot();
        self.instantiation.set_function_scope(global_def_id, None);
        let effective_generics = self
            .effective_generics(global_def_id, &signature.generics)
            .to_vec();
        let source_function_body = self.input.program.function_body(global_def_id);
        let effective_params = self
            .effective_generic_params_for_def(global_def_id)
            .unwrap_or_else(|| {
                effective_generics
                    .iter()
                    .map(|generic| (*generic, false))
                    .collect()
            });
        let identity_substitutions = effective_params
            .iter()
            .filter(|(_, is_const)| !is_const)
            .map(|(generic, _)| {
                (
                    *generic,
                    self.type_context
                        .append
                        .intern(TyKind::GenericParam(*generic)),
                )
            })
            .collect::<SymbolMap<_>>();
        let function_body = source_function_body.map(|body| {
            self.instantiate_function_body(crate::instantiate::FunctionBodyInstantiation {
                function: global_def_id,
                module_id: self.input.module_id,
                is_instance: false,
                type_arg_count: 0,
                body: body.clone(),
                self_arg: None,
                substitutions: &identity_substitutions,
                const_substitutions: &SymbolMap::default(),
            })
        });
        let typed_param_tys = source_function_body
            .map(|body| {
                body.locals
                    .iter()
                    .filter(|local| local.kind == nia_function_ir::FunctionLocalKind::Param)
                    .map(|local| (local.id, local.ty))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let backend_function = Some(BackendFunction {
            def_id: global_def_id,
            name: function.name,
            link_name: signature.is_extern.then(|| self.symbol_name(function.name)),
            generics: effective_generics,
            params: function
                .params
                .iter()
                .zip(signature.params.iter())
                .enumerate()
                .map(|(index, (param, signature))| {
                    let local_id = self
                        .input
                        .locals
                        .node_local_defs
                        .get(&param.node_key)
                        .copied();
                    let local_ty = if signature.receiver.is_some() {
                        typed_param_tys
                            .get(index)
                            .map(|(_, ty)| *ty)
                            .or_else(|| {
                                local_id.and_then(|local_id| {
                                    typed_param_tys
                                        .iter()
                                        .find_map(|(id, ty)| (*id == local_id).then_some(*ty))
                                })
                            })
                            .or_else(|| {
                                local_id.and_then(|local_id| {
                                    self.input
                                        .semantic_facts
                                        .function_facts
                                        .get(&global_def_id)
                                        .and_then(|facts| facts.local_types.get(&local_id))
                                        .copied()
                                })
                            })
                            .unwrap_or(signature.ty)
                    } else {
                        signature.ty
                    };
                    let passing_ty = signature
                        .receiver
                        .map(|receiver| self.receiver_passing_ty(receiver, local_ty))
                        .unwrap_or(signature.ty);
                    let substitutions = SymbolMap::default();
                    BackendParam {
                        local_id,
                        name: param.name,
                        receiver: signature.receiver,
                        passing_ty: self.instantiate_ty(passing_ty, &substitutions),
                        local_ty: self.instantiate_ty(local_ty, &substitutions),
                        span: param.span,
                    }
                })
                .collect(),
            return_type: self.instantiate_ty(signature.return_type, &SymbolMap::default()),
            is_extern: signature.is_extern,
            is_variadic: signature.is_variadic,
            attributes: signature
                .attributes
                .iter()
                .filter_map(|attribute| match attribute {
                    FunctionAttribute::Naked => Some(BackendFunctionAttribute::Naked),
                    FunctionAttribute::Builtin(_) => None,
                })
                .collect(),
            local_names: function_body
                .as_ref()
                .map(|body| self.function_local_names(body))
                .unwrap_or_default(),
            function_body,
            span,
        });
        self.instantiation.restore(instantiation_snapshot);
        backend_function
    }

    fn report_backend_function_param_count_mismatch(
        &mut self,
        def_id: nia_ids::GlobalDefId,
        span: Span,
        syntax_params: usize,
        signature_params: usize,
    ) {
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::internal_error(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                "backend function syntax parameters do not match its signature",
            )
            .primary(
                span,
                "backend function syntax parameters do not match its signature",
            )
            .debug("def_id", def_id)
            .debug("syntax_params", syntax_params)
            .debug("signature_params", signature_params)
            .finish(),
        );
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
            ReceiverKind::RefReadOnly => {
                let elem = self.receiver_base_ty(local_ty).unwrap_or(local_ty);
                self.type_context.append.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem,
                })
            }
            ReceiverKind::Ref => {
                let elem = self.receiver_base_ty(local_ty).unwrap_or(local_ty);
                self.type_context.append.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem,
                })
            }
        }
    }

    fn receiver_base_ty(&self, ty: nia_ids::InternedTyId) -> Option<nia_ids::InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Pointer { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    fn is_fat_receiver_local_ty(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Slice { .. } | TyKind::TraitObject { .. })
        )
    }
}

fn simplify_static_init(init: StaticInit) -> (StaticInit, bool) {
    match init {
        StaticInit::Array(elems) => {
            let mut changed = false;
            let elems = elems
                .into_iter()
                .map(|elem| {
                    let (elem, elem_changed) = simplify_static_init(elem);
                    changed |= elem_changed;
                    elem
                })
                .collect::<Vec<_>>();
            if elems.iter().all(is_zero_static_init) {
                (StaticInit::Zero, true)
            } else if let Some(first) = elems.first()
                && elems.iter().all(|elem| elem == first)
            {
                let Some(count) = u64::try_from(elems.len()).ok() else {
                    return (StaticInit::Array(elems), changed);
                };
                let first = elems
                    .into_iter()
                    .next()
                    .expect("uniform static initializer array must be non-empty");
                (
                    StaticInit::Repeat {
                        value: Box::new(first),
                        count,
                    },
                    true,
                )
            } else {
                (StaticInit::Array(elems), changed)
            }
        }
        StaticInit::Vector(elems) => {
            let mut changed = false;
            let elems = elems
                .into_iter()
                .map(|elem| {
                    let (elem, elem_changed) = simplify_static_init(elem);
                    changed |= elem_changed;
                    elem
                })
                .collect::<Vec<_>>();
            if elems.iter().all(is_zero_static_init) {
                (StaticInit::Zero, true)
            } else {
                (StaticInit::Vector(elems), changed)
            }
        }
        StaticInit::Repeat { value, count } => {
            let (value, changed) = simplify_static_init(*value);
            if count == 0 || is_zero_static_init(&value) {
                (StaticInit::Zero, true)
            } else {
                (
                    StaticInit::Repeat {
                        value: Box::new(value),
                        count,
                    },
                    changed,
                )
            }
        }
        StaticInit::Struct(fields) => {
            let mut changed = false;
            let fields = fields
                .into_iter()
                .map(|mut field| {
                    let (value, field_changed) = simplify_static_init(field.value);
                    field.value = value;
                    changed |= field_changed;
                    field
                })
                .collect::<Vec<_>>();
            if fields.iter().all(|field| is_zero_static_init(&field.value)) {
                (StaticInit::Zero, true)
            } else {
                (StaticInit::Struct(fields), changed)
            }
        }
        StaticInit::Chars(scalars) if scalars.iter().all(|scalar| *scalar == 0) => {
            (StaticInit::Zero, true)
        }
        StaticInit::Chars(scalars) => match repeated_static_init(scalars, StaticInit::Char) {
            Ok(init) => (init, true),
            Err(scalars) => (StaticInit::Chars(scalars), false),
        },
        StaticInit::Bytes(bytes) if bytes.iter().all(|byte| *byte == 0) => (StaticInit::Zero, true),
        StaticInit::Bytes(bytes) => match repeated_static_init(bytes, StaticInit::Byte) {
            Ok(init) => (init, true),
            Err(bytes) => (StaticInit::Bytes(bytes), false),
        },
        StaticInit::Float(text) if is_zero_float_static_init(&text) => (StaticInit::Zero, true),
        StaticInit::Int(value) if value.bits() == 0 => (StaticInit::Zero, true),
        StaticInit::Bool(false)
        | StaticInit::Char(0)
        | StaticInit::Byte(0)
        | StaticInit::NullPtr => (StaticInit::Zero, true),
        StaticInit::Zero => (StaticInit::Zero, false),
        other => (other, false),
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
        StaticInit::Array(elems) | StaticInit::Vector(elems) => {
            elems.iter().all(is_zero_static_init)
        }
        StaticInit::Repeat { value, count } => *count == 0 || is_zero_static_init(value),
        StaticInit::Struct(fields) => fields.iter().all(|field| is_zero_static_init(&field.value)),
        StaticInit::Int(_)
        | StaticInit::Bool(_)
        | StaticInit::Char(_)
        | StaticInit::Byte(_)
        | StaticInit::AddrOfGlobal { .. }
        | StaticInit::AddrOfFunction { .. } => false,
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
        let Some(count) = u64::try_from(elems.len()).ok() else {
            return Err(elems);
        };
        Ok(StaticInit::Repeat {
            value: Box::new(init(first)),
            count,
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

        assert_eq!(simplify_static_init(init), (StaticInit::Zero, true));
    }

    #[test]
    fn treats_empty_repeat_static_init_as_zero() {
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let init = StaticInit::Repeat {
            value: Box::new(StaticInit::AddrOfFunction {
                function: nia_ids::GlobalDefId {
                    module_id,
                    def_id: nia_ids::DefId(0),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            }),
            count: 0,
        };

        assert!(is_zero_static_init(&init));
    }

    #[test]
    fn vector_simplification_preserves_lane_identity() {
        let init = StaticInit::Vector(vec![
            StaticInit::Int(nia_ty::IntConst::unsigned(3)),
            StaticInit::Int(nia_ty::IntConst::unsigned(3)),
            StaticInit::Int(nia_ty::IntConst::unsigned(9)),
        ]);

        assert_eq!(simplify_static_init(init.clone()), (init, false));
        assert_eq!(
            simplify_static_init(StaticInit::Vector(vec![
                StaticInit::Int(nia_ty::IntConst::unsigned(0)),
                StaticInit::Int(nia_ty::IntConst::unsigned(0)),
            ])),
            (StaticInit::Zero, true)
        );
    }
}
