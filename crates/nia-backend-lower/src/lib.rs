// SPDX-License-Identifier: GPL-3.0-or-later
mod body;
mod instantiate;
mod items;
mod literals;
mod static_init;
mod struct_instances;

use nia_ast::{BindingItem, BracketArg, Expr, ExprKind, ItemKind, Module};
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendLayouts, BackendModule, BackendProgram,
    BackendStructInstanceKey,
};
use nia_body_check::BodyCheck;
use nia_defs::{DefCollection, DefId, DefKind, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_item_signatures::ItemSignatures;
use nia_layout::Layouts;
use nia_local_resolve::{LocalResolution, LocalUse};
use nia_monomorphize::Monomorphization;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLowering {
    pub program: BackendProgram,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLowerModuleInput<'a> {
    pub module_id: ModuleId,
    pub module_name: String,
    pub module: &'a Module,
    pub all_modules: &'a [Module],
    pub defs: &'a DefCollection,
    pub all_defs: &'a [DefCollection],
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub type_lowering: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub type_normalization: &'a TypeNormalization,
    pub body_check: &'a BodyCheck,
    pub extensions: &'a VisibleExtensionMethods,
    pub comptime: &'a nia_comptime_check::ComptimeCheck,
    pub layouts: &'a Layouts,
    pub extension_interner: Option<&'a nia_ty::TyInterner>,
}

pub fn lower_backend_program(
    modules: &[BackendLowerModuleInput<'_>],
    monomorphization: &Monomorphization,
) -> BackendLowering {
    let mut diagnostics = Vec::new();
    let lowered_modules = modules
        .iter()
        .map(|input| {
            let mut lowerer = ModuleLowerer::new(input, monomorphization);
            let module = lowerer.lower_module();
            diagnostics.extend(lowerer.diagnostics);
            module
        })
        .collect();
    BackendLowering {
        program: BackendProgram {
            modules: lowered_modules,
        },
        diagnostics,
    }
}

pub(crate) struct ModuleLowerer<'a> {
    pub(crate) input: &'a BackendLowerModuleInput<'a>,
    pub(crate) monomorphization: &'a Monomorphization,
    pub(crate) interner: nia_ty::TyInterner,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) current_param_locals: Vec<LocalId>,
    pub(crate) comptime_global_stack: Vec<GlobalDefId>,
    pub(crate) comptime_local_stack: Vec<LocalId>,
}

impl<'a> ModuleLowerer<'a> {
    fn new(input: &'a BackendLowerModuleInput<'a>, monomorphization: &'a Monomorphization) -> Self {
        Self {
            input,
            monomorphization,
            interner: input.body_check.interner.clone(),
            diagnostics: Vec::new(),
            current_param_locals: Vec::new(),
            comptime_global_stack: Vec::new(),
            comptime_local_stack: Vec::new(),
        }
    }

    fn comptime_binding_for(&self, global_id: GlobalDefId) -> Option<&'a BindingItem> {
        let (module_index, defs) = self
            .input
            .all_defs
            .iter()
            .enumerate()
            .find(|(_, defs)| defs.module_id == global_id.module_id)?;
        let module = self.input.all_modules.get(module_index)?;
        module.items.iter().find_map(|item| {
            let ItemKind::Binding(binding) = &item.kind else {
                return None;
            };
            if !binding.is_comptime {
                return None;
            }
            let def_id = defs.def_spans.get(item.span)?;
            (def_id == global_id.def_id).then_some(binding)
        })
    }

    pub(crate) fn comptime_global_id_for_expr(&self, expr: &Expr) -> Option<GlobalDefId> {
        if let Some(global_id) = self.input.values.qualified_values.get(&expr.span).copied()
            && self.def_kind_of(global_id) == Some(DefKind::Comptime)
        {
            return Some(global_id);
        }
        let Some(nia_value_resolve::ValueNameResolution::Def(def_id)) =
            self.input.values.names.get(&expr.span)
        else {
            return None;
        };
        (self.input.defs.defs.get(*def_id)?.kind == DefKind::Comptime)
            .then_some(self.global_def_id(*def_id))
    }

    fn lower_module(&mut self) -> BackendModule {
        let mut structs = Vec::new();
        let mut unions = Vec::new();
        let mut struct_instances = Vec::new();
        let mut union_instances = Vec::new();
        let mut enums = Vec::new();
        let mut globals = Vec::new();
        let mut functions = Vec::new();
        let mut function_templates = Vec::new();

        for item in &self.input.module.items {
            match &item.kind {
                ItemKind::Struct(item_struct) => {
                    if item_struct.generics.is_empty()
                        && let Some(item) = self.lower_struct(item.span, item_struct)
                    {
                        structs.push(item);
                    }
                    struct_instances.extend(self.lower_struct_instances(item.span, item_struct));
                }
                ItemKind::Union(item_union) => {
                    if item_union.generics.is_empty()
                        && let Some(item) = self.lower_union(item.span, item_union)
                    {
                        unions.push(item);
                    }
                    union_instances.extend(self.lower_union_instances(item.span, item_union));
                }
                ItemKind::Extend(extend) => {
                    let extend_target_is_generic = self.extend_target_has_generics(extend);
                    for method in &extend.methods {
                        if let Some(function) =
                            self.lower_function(method.function.span, &method.function)
                        {
                            if !extend_target_is_generic && function.generics.is_empty() {
                                functions.push(function.clone());
                            }
                            function_templates.push(function);
                        }
                    }
                }
                ItemKind::Enum(item_enum) => {
                    if let Some(item) = self.lower_enum(item.span, item_enum) {
                        enums.push(item);
                    }
                }
                ItemKind::Function(function) => {
                    if let Some(function) = self.lower_function(item.span, function) {
                        if function.generics.is_empty() {
                            functions.push(function.clone());
                        }
                        function_templates.push(function);
                    }
                }
                ItemKind::Binding(binding) => {
                    if binding.is_comptime {
                        continue;
                    }
                    if let Some(global) = self.lower_global(item.span, binding) {
                        globals.push(global);
                    }
                }
                ItemKind::Import(_) | ItemKind::Using(_) | ItemKind::TypeAlias(_) => {}
            }
        }

        let function_instances = self.lower_function_instances(&function_templates);
        self.extend_struct_instances_from_functions(
            &mut struct_instances,
            &mut union_instances,
            &functions,
            &function_instances,
        );

        let mut backend_layouts =
            BackendLayouts::from_module_layouts(self.input.module_id, self.input.layouts);
        self.extend_backend_layouts_for_instances(&mut backend_layouts);

        BackendModule {
            id: self.input.module_id,
            name: self.input.module_name.clone(),
            interner: self.interner.clone(),
            comptime: self.input.comptime.clone(),
            layouts: backend_layouts,
            structs,
            unions,
            struct_instances,
            union_instances,
            enums,
            globals,
            functions,
            function_instances,
            generic_instantiations: self
                .input
                .body_check
                .generic_instantiations
                .iter()
                .map(|inst| nia_backend_ir::BackendGenericInstantiation {
                    def_id: inst.def_id,
                    arg_module_id: self.input.module_id,
                    args: inst.args.clone(),
                    span: inst.span,
                    source_def_id: inst.source_def_id,
                })
                .collect(),
        }
    }

    fn extend_backend_layouts_for_instances(&self, layouts: &mut BackendLayouts) {
        let computed = nia_layout::compute_layouts_with_normalized_types(
            self.input.defs,
            &self.interner,
            self.input.signatures,
            &self.input.type_normalization.normalized,
            self.input.comptime,
            self.input.layouts.target,
        );
        for (key, layout) in computed.struct_instances {
            let key = BackendStructInstanceKey::from_module_key(self.input.module_id, &key);
            if !layouts
                .struct_instances
                .iter()
                .any(|(candidate, _)| *candidate == key)
            {
                layouts.struct_instances.push((key, layout));
            }
        }
        for (key, layout) in computed.union_instances {
            let key = BackendStructInstanceKey::from_module_key(self.input.module_id, &key);
            if !layouts
                .union_instances
                .iter()
                .any(|(candidate, _)| *candidate == key)
            {
                layouts.union_instances.push((key, layout));
            }
        }
    }

    fn lower_function_instances(
        &mut self,
        functions: &[BackendFunction],
    ) -> Vec<BackendFunctionInstance> {
        let mut instances = Vec::new();
        for instance in &self.monomorphization.instances {
            if instance.def_id.module_id != self.input.module_id {
                continue;
            }
            let Some(base) = functions
                .iter()
                .find(|function| function.def_id == instance.def_id)
            else {
                continue;
            };
            let substitutions = self.effective_generic_substitutions(base.def_id, &instance.args);
            instances.push(BackendFunctionInstance {
                def_id: instance.def_id,
                name: base.name.clone(),
                arg_module_id: instance.arg_module_id,
                args: instance.args.clone(),
                symbol: instance.symbol.clone(),
                params: self.instantiate_params(base, &substitutions),
                return_type: self.instantiate_ty(base.return_type, &substitutions),
                is_extern: base.is_extern,
                is_variadic: base.is_variadic,
                body: base
                    .body
                    .clone()
                    .map(|body| self.instantiate_body(body, &substitutions)),
                span: base.span,
            });
        }
        instances
    }

    fn extend_target_has_generics(&self, extend: &nia_ast::ExtendItem) -> bool {
        let Some(ty) = self
            .input
            .type_lowering
            .type_uses
            .get(&extend.target.span)
            .copied()
        else {
            return !extend.generics.is_empty();
        };
        !self.generic_params_in_ty(ty).is_empty()
    }

    fn def_name(&self, def_id: DefId) -> String {
        self.input
            .defs
            .defs
            .get(def_id)
            .map(|def| def.name.clone())
            .unwrap_or_else(|| format!("def{}", def_id.0))
    }

    fn local_ty(&self, local_id: LocalId) -> Option<InternedTyId> {
        self.input.body_check.local_types.get(&local_id).copied()
    }

    fn expr_ty(&self, expr: &Expr) -> Option<InternedTyId> {
        self.input.body_check.expr_types.get(&expr.span).copied()
    }

    fn ty_for_type_span(&self, span: Span) -> InternedTyId {
        self.input
            .type_lowering
            .type_uses
            .get(&span)
            .copied()
            .unwrap_or_else(|| self.error_ty())
    }

    fn def_id_for_span(&mut self, span: Span, expected: DefKind) -> Option<DefId> {
        let def_id = self.input.defs.def_spans.get(span)?;
        let def = self.input.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    fn def_id_for_span_any_function(&mut self, span: Span) -> Option<DefId> {
        let def_id = self.input.defs.def_spans.get(span)?;
        let def = self.input.defs.defs.get(def_id)?;
        matches!(def.kind, DefKind::Function | DefKind::Method).then_some(def_id)
    }

    fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.module_id,
            def_id,
        }
    }

    fn global_error_def(&self) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.module_id,
            def_id: DefId(u32::MAX),
        }
    }

    fn error_ty(&self) -> InternedTyId {
        self.input.body_check.interner.error()
    }

    fn void_ty(&self) -> InternedTyId {
        self.input.body_check.interner.primitive(PrimitiveTy::Void)
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        if ty.interner_id == self.input.body_check.interner.interner_id() {
            return self.input.body_check.interner.get(ty);
        }
        if let Some(extension_interner) = self.input.extension_interner
            && ty.interner_id == extension_interner.interner_id()
        {
            return extension_interner.get(ty);
        }
        None
    }

    fn nominal_global_def(&self, ty: InternedTyId) -> Option<GlobalDefId> {
        match self.input.body_check.interner.get(ty) {
            Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
            _ => None,
        }
    }

    fn field_def_for_struct_ty(&self, ty: InternedTyId, name: &str) -> Option<GlobalDefId> {
        let def_id = self.nominal_global_def(ty)?;
        let defs = self.defs_for_module(def_id.module_id)?;
        defs.scopes
            .struct_members
            .get(&def_id.def_id)
            .and_then(|members| members.fields.get(name))
            .or_else(|| {
                defs.scopes
                    .union_members
                    .get(&def_id.def_id)
                    .and_then(|members| members.fields.get(name))
            })
            .map(|field| GlobalDefId {
                module_id: def_id.module_id,
                def_id: field,
            })
    }

    fn field_def_for_base_ty(&self, ty: InternedTyId, name: &str) -> Option<GlobalDefId> {
        let (def_id, _) = self.receiver_base_type(ty)?;
        let defs = self.defs_for_module(def_id.module_id)?;
        defs.scopes
            .struct_members
            .get(&def_id.def_id)
            .and_then(|members| members.fields.get(name))
            .or_else(|| {
                defs.scopes
                    .union_members
                    .get(&def_id.def_id)
                    .and_then(|members| members.fields.get(name))
            })
            .map(|field| GlobalDefId {
                module_id: def_id.module_id,
                def_id: field,
            })
    }

    fn receiver_base_type(&self, ty: InternedTyId) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        match self.input.body_check.interner.get(ty) {
            Some(TyKind::Nominal { def_id, args }) => Some((*def_id, args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.receiver_base_type(*elem),
            _ => None,
        }
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<&DefCollection> {
        self.input
            .all_defs
            .iter()
            .find(|defs| defs.module_id == module_id)
    }

    fn type_prefix_instance(&self, expr: &Expr) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if let ExprKind::BracketSuffix { callee, args } = &expr.kind {
            let (def_id, _) = self.type_prefix_instance(callee)?;
            let args = lowered_type_args(args, self.input.type_lowering);
            return Some((def_id, args));
        }
        if let ExprKind::Qualified { .. } = &expr.kind {
            if let Some(def_id) = self
                .input
                .values
                .qualified_type_prefixes
                .get(&expr.span)
                .copied()
            {
                return Some((def_id, Vec::new()));
            }
            return None;
        }
        if let Some(ty) = self.expr_ty(expr)
            && let Some(TyKind::Nominal { def_id, args }) = self.input.body_check.interner.get(ty)
        {
            return Some((*def_id, args.clone()));
        }
        let ExprKind::Ident(name) = &expr.kind else {
            return None;
        };
        if !matches!(
            self.input.locals.uses.get(&expr.span),
            Some(LocalUse::TypePrefix)
        ) {
            return None;
        }
        self.input
            .defs
            .module_scope
            .types
            .get(name)
            .map(|def_id| (self.global_def_id(def_id), Vec::new()))
    }

    fn enum_variant_for_qualified(&self, lhs: &Expr, name: &str) -> Option<GlobalDefId> {
        let (enum_id, _) = self.type_prefix_instance(lhs)?;
        let target_defs = self
            .input
            .all_defs
            .iter()
            .find(|defs| defs.module_id == enum_id.module_id)?;
        let variant_id = target_defs
            .scopes
            .enum_members
            .get(&enum_id.def_id)?
            .variants
            .get(name)?;
        Some(GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_id,
        })
    }

    fn qualified_enum_variant(&self, expr: &Expr) -> Option<GlobalDefId> {
        self.input
            .values
            .variant_enums
            .contains_key(&expr.span)
            .then(|| self.input.values.qualified_values.get(&expr.span).copied())
            .flatten()
    }

    pub(crate) fn def_kind_of(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.input
            .all_defs
            .iter()
            .find(|defs| defs.module_id == global_id.module_id)
            .and_then(|defs| defs.defs.get(global_id.def_id))
            .map(|def| def.kind)
    }
}

fn generic_inst_base(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::BracketSuffix { callee, .. } => generic_inst_base(callee),
        _ => expr,
    }
}

pub(crate) fn lowered_type_args(
    args: &[BracketArg],
    type_lowering: &TypeLowering,
) -> Vec<InternedTyId> {
    args.iter()
        .filter_map(|arg| {
            arg.ty
                .as_ref()
                .and_then(|ty| type_lowering.type_uses.get(&ty.span).copied())
        })
        .collect()
}

#[cfg(test)]
mod tests;
