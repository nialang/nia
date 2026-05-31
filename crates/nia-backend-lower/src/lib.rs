// SPDX-License-Identifier: GPL-3.0-or-later
mod instantiate;
mod items;
mod struct_instances;

use nia_ast::{Expr, ItemKind, Module};
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendLayouts, BackendModule, BackendProgram,
    BackendStructInstanceKey,
};
use nia_body_check::BodyCheck;
use nia_body_check::ProgramTraitImplSignature;
use nia_defs::{DefCollection, DefId, DefKind, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_function_ir::FunctionBody;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::ItemSignatures;
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_monomorphize::Monomorphization;
use nia_span::Span;
use nia_ty::TyKind;
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
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub type_lowering: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub type_normalization: &'a TypeNormalization,
    pub body_check: &'a BodyCheck,
    pub extensions: &'a VisibleExtensionMethods,
    pub comptime: &'a nia_comptime_check::ComptimeCheck,
    pub layouts: &'a Layouts,
    pub function_bodies: &'a std::collections::HashMap<GlobalDefId, FunctionBody>,
    pub extension_interner: Option<&'a nia_ty::TyInterner>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
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
}

impl<'a> ModuleLowerer<'a> {
    fn new(input: &'a BackendLowerModuleInput<'a>, monomorphization: &'a Monomorphization) -> Self {
        Self {
            input,
            monomorphization,
            interner: input.body_check.ir.interner.clone(),
            diagnostics: Vec::new(),
        }
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
                ItemKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        if method.function.body.is_some()
                            && let Some(function) =
                                self.lower_function(method.function.span, &method.function)
                        {
                            function_templates.push(function);
                        }
                    }
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
                .ir
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
            let function_body = base
                .function_body
                .clone()
                .map(|body| self.instantiate_function_body(body, &substitutions));
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
                function_body,
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

    fn expr_ty(&self, expr: &Expr) -> Option<InternedTyId> {
        self.input.body_check.ir.expr_types.get(&expr.span).copied()
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
        matches!(
            def.kind,
            DefKind::Function | DefKind::Method | DefKind::TraitMethod
        )
        .then_some(def_id)
    }

    fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.module_id,
            def_id,
        }
    }

    fn resolved_array_len(&self, id: GlobalConstExprId) -> u64 {
        self.input
            .comptime
            .array_lengths
            .get(&id)
            .copied()
            .expect("array length used in backend symbol must be evaluated")
    }

    fn error_ty(&self) -> InternedTyId {
        self.input.body_check.ir.interner.error()
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        if ty.interner_id == self.input.body_check.ir.interner.interner_id() {
            return self.input.body_check.ir.interner.get(ty);
        }
        if let Some(extension_interner) = self.input.extension_interner
            && ty.interner_id == extension_interner.interner_id()
        {
            return extension_interner.get(ty);
        }
        None
    }
}
#[cfg(test)]
mod tests;
