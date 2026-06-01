// SPDX-License-Identifier: GPL-3.0-or-later
mod function_instances;
mod instantiate;
mod items;
mod operator_dispatch;
mod opt;
mod struct_instances;
mod trait_object_vtables;

use std::collections::{HashMap, HashSet};

use nia_ast::{Expr, ItemKind, Module};
use nia_backend_ir::{BackendLayouts, BackendModule, BackendProgram, BackendStructInstanceKey};
use nia_body_check::BodyCheck;
use nia_defs::{DefCollection, DefId, DefKind, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_function_ir::FunctionBody;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::{
    ItemSignatures, ProgramEnumSignature, ProgramTraitImplSignature, ProgramTraitSignature,
};
use nia_layout::{Layouts, StructLayoutKey};
use nia_local_resolve::LocalResolution;
use nia_monomorphize::Monomorphization;
use nia_opt::OptimizationPolicy;
use nia_span::Span;
use nia_ty::TyKind;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLowering {
    pub program: BackendProgram,
    pub optimization: OptimizationPolicy,
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
    pub program_enums: &'a std::collections::HashMap<GlobalDefId, ProgramEnumSignature>,
    pub program_traits: &'a std::collections::HashMap<GlobalDefId, ProgramTraitSignature>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

pub fn lower_backend_program(
    modules: &[BackendLowerModuleInput<'_>],
    monomorphization: &Monomorphization,
    optimization: OptimizationPolicy,
) -> BackendLowering {
    let mut diagnostics = Vec::new();
    let lowered_modules = modules
        .iter()
        .map(|input| {
            let mut lowerer = ModuleLowerer::new(input, monomorphization, optimization);
            let module = lowerer.lower_module();
            diagnostics.extend(lowerer.diagnostics);
            module
        })
        .collect();
    BackendLowering {
        program: BackendProgram {
            modules: lowered_modules,
        },
        optimization,
        diagnostics,
    }
}

pub(crate) struct ModuleLowerer<'a> {
    pub(crate) input: &'a BackendLowerModuleInput<'a>,
    pub(crate) monomorphization: &'a Monomorphization,
    pub(crate) optimization: OptimizationPolicy,
    pub(crate) interner: nia_ty::TyInterner,
    pub(crate) diagnostics: Vec<Diagnostic>,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    extension_targets_by_method: HashMap<GlobalDefId, InternedTyId>,
    struct_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
    union_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
}

impl<'a> ModuleLowerer<'a> {
    fn new(
        input: &'a BackendLowerModuleInput<'a>,
        monomorphization: &'a Monomorphization,
        optimization: OptimizationPolicy,
    ) -> Self {
        Self {
            input,
            monomorphization,
            optimization,
            interner: input.body_check.ir.interner.clone(),
            diagnostics: Vec::new(),
            missing_array_len_diagnostics: HashSet::new(),
            extension_targets_by_method: index_extension_targets_by_method(input.extensions),
            struct_layout_instances_by_def: index_layout_instances_by_def(
                input.layouts.struct_instances.keys(),
            ),
            union_layout_instances_by_def: index_layout_instances_by_def(
                input.layouts.union_instances.keys(),
            ),
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
        let mut trait_object_vtables = Vec::new();

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
        self.collect_trait_object_vtables(
            &mut trait_object_vtables,
            &functions,
            &function_instances,
        );
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
            trait_object_vtables,
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

    fn resolved_array_len(&mut self, id: GlobalConstExprId) -> Option<u64> {
        let value = self.input.comptime.array_lengths.get(&id).copied();
        if value.is_none() && self.missing_array_len_diagnostics.insert(id) {
            let span = self
                .input
                .type_lowering
                .const_exprs
                .get(&id)
                .map(|expr| expr.span)
                .unwrap_or_default();
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("array length {id:?} was not evaluated before backend symbol generation"),
            ));
        }
        value
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.input.type_normalization.normalize(ty);
        if let Some(layout) = self.input.layouts.types.get(&ty).cloned() {
            return Some(layout);
        }
        let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty) else {
            return None;
        };
        if def_id.module_id != self.input.module_id {
            return None;
        }
        self.input.layouts.nominal_type_layout(*def_id, args)
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

fn index_extension_targets_by_method(
    extensions: &VisibleExtensionMethods,
) -> HashMap<GlobalDefId, InternedTyId> {
    let mut targets_by_method = HashMap::new();
    for target in extensions.targets() {
        for method in &target.methods {
            targets_by_method.insert(method.def_id, target.target_ty);
        }
    }
    targets_by_method
}

fn index_layout_instances_by_def<'a>(
    keys: impl IntoIterator<Item = &'a StructLayoutKey>,
) -> HashMap<DefId, Vec<StructLayoutKey>> {
    let mut instances_by_def = HashMap::new();
    for key in keys {
        instances_by_def
            .entry(key.def_id)
            .or_insert_with(Vec::new)
            .push(key.clone());
    }
    instances_by_def
}
#[cfg(test)]
mod tests;
