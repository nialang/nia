// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

mod aggregates;
mod calls;
mod expr;
mod helpers;
mod literals;
mod places;
mod type_support;

use nia_ast::{
    BindingStmt, Block, Expr, ExprKind, ForInit, FunctionItem, ItemKind, Module, Stmt, StmtKind,
};
use nia_defs::{DefCollection, DefId, DefKind, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, LocalId, TyId};
use nia_item_signatures::{
    EnumSignature, FunctionSignature, GlobalSignature, ItemSignatures, StructSignature,
    UnionSignature,
};
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

#[derive(Debug, Clone, PartialEq)]
pub struct BodyCheck {
    pub interner: TyInterner,
    pub expr_types: HashMap<Span, TyId>,
    pub array_to_slice_coercions: HashMap<Span, ArrayToSliceCoercion>,
    pub local_types: HashMap<LocalId, TyId>,
    pub builtin_values: HashMap<Span, BuiltinValue>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinValue {
    Usize(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayToSliceCoercion {
    pub array_ty: TyId,
    pub slice_ty: TyId,
    pub is_const: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericInstantiation {
    pub def_id: GlobalDefId,
    pub args: Vec<TyId>,
    pub generics: Vec<String>,
    pub span: Span,
    pub source_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramFunctionSignature {
    pub signature: FunctionSignature,
    pub interner: TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramGlobalSignature {
    pub signature: GlobalSignature,
    pub interner: TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramStructSignature {
    pub signature: StructSignature,
    pub interner: TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramUnionSignature {
    pub signature: UnionSignature,
    pub interner: TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramEnumSignature {
    pub signature: nia_item_signatures::EnumSignature,
    pub interner: TyInterner,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramSignatureMaps<'a> {
    pub functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    pub unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    pub enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyCheckInput<'a> {
    pub module: &'a Module,
    pub defs: &'a DefCollection,
    pub all_defs: &'a [DefCollection],
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub layouts: &'a Layouts,
    pub extensions: &'a VisibleExtensionMethods,
    pub program_signatures: ProgramSignatureMaps<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyCheckWithProgramSignaturesInput<'a> {
    pub module: &'a Module,
    pub defs: &'a DefCollection,
    pub all_defs: &'a [DefCollection],
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub extensions: &'a VisibleExtensionMethods,
    pub program_signatures: ProgramSignatureMaps<'a>,
}

#[derive(Debug, Clone)]
struct ResolvedFunctionSignature {
    def_id: GlobalDefId,
    signature: FunctionSignature,
}

pub fn check_module_bodies(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
    signatures: &ItemSignatures,
) -> BodyCheck {
    let empty_normalization = TypeNormalization {
        interner: lowered.interner.clone(),
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let layouts = nia_layout::compute_layouts(
        defs,
        &lowered.interner,
        signatures,
        nia_layout::TargetDataLayout::LP64,
    );
    let empty_functions = HashMap::new();
    let empty_globals = HashMap::new();
    let empty_structs = HashMap::new();
    let empty_unions = HashMap::new();
    let empty_enums = HashMap::new();
    let empty_extensions = VisibleExtensionMethods::default();
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        module,
        defs,
        all_defs: std::slice::from_ref(defs),
        values,
        locals,
        lowered,
        signatures,
        normalization: &empty_normalization,
        layouts: &layouts,
        extensions: &empty_extensions,
        program_signatures: ProgramSignatureMaps {
            functions: &empty_functions,
            globals: &empty_globals,
            structs: &empty_structs,
            unions: &empty_unions,
            enums: &empty_enums,
        },
    });
    checked.diagnostics.extend(layouts.diagnostics);
    checked
}

pub fn check_module_bodies_with_layouts(input: BodyCheckInput<'_>) -> BodyCheck {
    check_module_bodies_with_program_signatures_and_layouts(input)
}

pub fn check_module_bodies_with_program_signatures(
    input: BodyCheckWithProgramSignaturesInput<'_>,
) -> BodyCheck {
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        input.defs,
        &input.normalization.interner,
        input.signatures,
        &input.normalization.normalized,
        nia_layout::TargetDataLayout::LP64,
    );
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        module: input.module,
        defs: input.defs,
        all_defs: input.all_defs,
        values: input.values,
        locals: input.locals,
        lowered: input.lowered,
        signatures: input.signatures,
        normalization: input.normalization,
        layouts: &layouts,
        extensions: input.extensions,
        program_signatures: input.program_signatures,
    });
    checked.diagnostics.extend(layouts.diagnostics);
    checked
}

pub fn check_module_bodies_with_program_signatures_and_layouts(
    input: BodyCheckInput<'_>,
) -> BodyCheck {
    let mut checker = BodyChecker {
        defs: input.defs,
        all_defs: input.all_defs,
        values: input.values,
        locals: input.locals,
        interner: input.lowered.interner.clone(),
        type_uses: &input.lowered.type_uses,
        signatures: input.signatures,
        normalization: input.normalization,
        layouts: input.layouts,
        extensions: input.extensions,
        program_functions: input.program_signatures.functions,
        program_globals: input.program_signatures.globals,
        program_structs: input.program_signatures.structs,
        program_unions: input.program_signatures.unions,
        program_enums: input.program_signatures.enums,
        expr_types: HashMap::new(),
        array_to_slice_coercions: HashMap::new(),
        builtin_values: HashMap::new(),
        generic_instantiations: Vec::new(),
        local_types: HashMap::new(),
        global_types: HashMap::new(),
        diagnostics: Vec::new(),
        current_return: input.lowered.interner.primitive(PrimitiveTy::Void),
        current_def_id: None,
    };
    checker.seed_global_types();
    checker.check_module(input.module);
    BodyCheck {
        interner: checker.interner,
        expr_types: checker.expr_types,
        array_to_slice_coercions: checker.array_to_slice_coercions,
        local_types: checker.local_types,
        builtin_values: checker.builtin_values,
        generic_instantiations: checker.generic_instantiations,
        diagnostics: checker.diagnostics,
    }
}

struct BodyChecker<'a> {
    defs: &'a DefCollection,
    all_defs: &'a [DefCollection],
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    interner: TyInterner,
    type_uses: &'a HashMap<Span, TyId>,
    signatures: &'a ItemSignatures,
    normalization: &'a TypeNormalization,
    layouts: &'a Layouts,
    extensions: &'a VisibleExtensionMethods,
    program_functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    program_globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    program_structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    program_unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    program_enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    expr_types: HashMap<Span, TyId>,
    array_to_slice_coercions: HashMap<Span, ArrayToSliceCoercion>,
    builtin_values: HashMap<Span, BuiltinValue>,
    generic_instantiations: Vec<GenericInstantiation>,
    local_types: HashMap<LocalId, TyId>,
    global_types: HashMap<DefId, TyId>,
    diagnostics: Vec<Diagnostic>,
    current_return: TyId,
    current_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
struct ReceiverBase {
    def_id: GlobalDefId,
    args: Vec<TyId>,
    from_pointer: bool,
    has_readonly_pointer: bool,
}

#[derive(Debug, Clone)]
struct ResolvedStructSignature {
    signature: StructSignature,
}

#[derive(Debug, Clone)]
struct ResolvedUnionSignature {
    signature: UnionSignature,
}

#[derive(Debug, Clone)]
struct ResolvedEnumSignature {
    signature: EnumSignature,
}

impl<'a> BodyChecker<'a> {
    fn check_module(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::Binding(binding) = &item.kind {
                self.check_global_binding(item.span, binding);
            }
        }
        for item in &module.items {
            if let ItemKind::Function(function) = &item.kind {
                self.check_function_item(item.span, function);
            }
        }
        for item in &module.items {
            if let ItemKind::Extend(extend) = &item.kind {
                for method in &extend.methods {
                    self.check_function_def(method.function.span, &method.function);
                }
            }
        }
    }

    fn seed_global_types(&mut self) {
        for (def_id, signature) in &self.signatures.globals {
            if let Some(ty) = signature.explicit_type {
                self.global_types.insert(*def_id, ty);
            }
        }
    }

    fn check_global_binding(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Global) else {
            return;
        };
        let Some(value) = &binding.value else {
            let Some(signature) = self.signatures.globals.get(&def_id) else {
                return;
            };
            if let Some(ty) = signature.explicit_type {
                self.global_types.insert(def_id, ty);
            } else {
                self.diagnostics.push(Diagnostic::error(
                    item_span,
                    "global declaration requires an explicit type",
                ));
            }
            return;
        };
        let global_ty = match binding.ty.as_ref() {
            Some(ty) => {
                let explicit = self.ty_for_span(ty.span);
                let value_ty = self.check_expr_with_expected(value, Some(explicit));
                self.expect_expr_type(value, explicit, value_ty, "global initializer");
                self.materialize_inferred_array_type(explicit, value_ty)
                    .unwrap_or(explicit)
            }
            None => self.check_expr(value),
        };
        self.global_types.insert(def_id, global_ty);
    }

    fn check_function_item(&mut self, item_span: Span, function: &FunctionItem) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Function) else {
            return;
        };
        self.check_function(def_id, function);
    }

    fn check_function_def(&mut self, span: Span, function: &FunctionItem) {
        let Some(def_id) = self.def_id_for_span(span, DefKind::Method) else {
            return;
        };
        self.check_function(def_id, function);
    }

    fn check_function(&mut self, def_id: DefId, function: &FunctionItem) {
        let Some(signature) = self.signatures.functions.get(&def_id) else {
            return;
        };
        let previous_return = self.current_return;
        let previous_def_id = self.current_def_id;
        self.current_return = signature.return_type;
        self.current_def_id = Some(self.global_def_id(def_id));
        let self_ty = self.method_self_type(def_id, signature);
        self.seed_param_types(signature, function, self_ty);
        if let Some(body) = &function.body {
            let expected_tail =
                (!self.is_void(signature.return_type)).then_some(signature.return_type);
            let body_ty = self.check_block_with_expected(body, expected_tail);
            if let Some(tail) = body.tail.as_deref() {
                if !self.is_void(signature.return_type) {
                    self.expect_expr_type(tail, signature.return_type, body_ty, "function body");
                }
            } else if self.is_void(signature.return_type) {
                self.expect_type(body.span, signature.return_type, body_ty, "function body");
            }
        }
        self.current_return = previous_return;
        self.current_def_id = previous_def_id;
    }

    fn seed_param_types(
        &mut self,
        signature: &FunctionSignature,
        function: &FunctionItem,
        self_ty: Option<TyId>,
    ) {
        for (param, param_sig) in function.params.iter().zip(&signature.params) {
            if let Some(local_id) = self.locals.local_defs.get(&param.span).copied() {
                let ty = if param_sig.receiver.is_some() {
                    self_ty.unwrap_or(param_sig.ty)
                } else {
                    param_sig.ty
                };
                self.local_types.insert(local_id, ty);
            }
        }
    }

    fn check_block(&mut self, block: &Block) -> TyId {
        self.check_block_with_expected(block, None)
    }

    fn check_block_with_expected(&mut self, block: &Block, expected_tail: Option<TyId>) -> TyId {
        if block.stmts.is_empty()
            && block.tail.is_none()
            && let Some(expected) = expected_tail
            && let Some(TyKind::Nominal { def_id, args }) = self.interner.get(expected)
        {
            let def_id = *def_id;
            let args = args.clone();
            if self.is_union_def(def_id) {
                self.diagnostics.push(Diagnostic::error(
                    block.span,
                    "union literal requires exactly one field, got 0",
                ));
                return expected;
            }
            if self.is_empty_struct_type(def_id, &args) {
                return expected;
            }
        }
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.check_expr_with_expected(tail, expected_tail)
        } else {
            self.void()
        }
    }

    fn is_empty_struct_type(&mut self, def_id: GlobalDefId, args: &[TyId]) -> bool {
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            return false;
        };
        resolved.signature.generics.len() == args.len() && resolved.signature.fields.is_empty()
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(binding) => {
                self.check_local_binding(stmt.span, binding);
            }
            StmtKind::Using(_) => {
                // Block-scope `using` is a no-op for body type-checking.
            }
            StmtKind::Expr(expr) => {
                let expr_ty = self.check_expr(expr);
                if !self.is_void(expr_ty) && !self.is_never(expr_ty) {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "non-void expression result is discarded; assign it to `_` explicitly",
                    ));
                }
            }
            StmtKind::Defer(expr) => {
                let expr_ty = self.check_expr(expr);
                if !self.is_void(expr_ty) {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "`defer` expression must have type `void`",
                    ));
                }
            }
            StmtKind::Return(value) => {
                let value_ty = match value {
                    Some(value) => self.check_expr_with_expected(value, Some(self.current_return)),
                    None => self.void(),
                };
                if let Some(value) = value {
                    self.expect_expr_type(value, self.current_return, value_ty, "return");
                } else {
                    self.expect_type(stmt.span, self.current_return, value_ty, "return");
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::For(for_stmt) => {
                match &for_stmt.header {
                    nia_ast::ForHeader::Infinite => {}
                    nia_ast::ForHeader::Condition(cond) => {
                        let cond_ty = self.check_expr(cond);
                        self.expect_type(cond.span, self.bool(), cond_ty, "for condition");
                    }
                    nia_ast::ForHeader::CStyle { init, cond, step } => {
                        if let Some(init) = init {
                            self.check_for_init(init);
                        }
                        if let Some(cond) = cond {
                            let cond_ty = self.check_expr(cond);
                            self.expect_type(cond.span, self.bool(), cond_ty, "for condition");
                        }
                        if let Some(step) = step {
                            self.check_expr(step);
                        }
                    }
                }
                self.check_block(&for_stmt.body);
            }
            StmtKind::Switch(switch) => {
                self.check_switch_stmt(switch);
            }
        }
    }

    fn check_for_init(&mut self, init: &ForInit) {
        match init {
            ForInit::Binding { span, binding } => self.check_local_binding(*span, binding),
            ForInit::Expr(expr) => {
                self.check_expr(expr);
            }
        }
    }

    fn check_local_binding(&mut self, span: Span, binding: &BindingStmt) {
        let binding_ty = match (&binding.ty, &binding.value) {
            (Some(ty), Some(value)) => {
                let explicit = self.ty_for_span(ty.span);
                let value_ty = self.check_expr_with_expected(value, Some(explicit));
                self.expect_expr_type(value, explicit, value_ty, "binding initializer");
                self.materialize_inferred_array_type(explicit, value_ty)
                    .unwrap_or(explicit)
            }
            (Some(ty), None) => self.ty_for_span(ty.span),
            (None, Some(value)) => self.check_expr(value),
            (None, None) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "binding declaration requires an explicit type",
                ));
                self.error()
            }
        };
        if let Some(local_id) = self.locals.local_defs.get(&span).copied() {
            self.local_types.insert(local_id, binding_ty);
        }
    }

    fn check_switch_stmt(&mut self, switch: &nia_ast::SwitchStmt) {
        let target_ty = self.check_expr(&switch.target);
        let enum_id = self.enum_global_def_id(target_ty);
        let mut has_default = false;
        let mut covered_variants = HashSet::new();

        for arm in &switch.arms {
            match &arm.pattern {
                nia_ast::SwitchPattern::Default => {
                    has_default = true;
                }
                nia_ast::SwitchPattern::Expr(pattern) => {
                    let pattern_ty = self.check_expr_with_expected(pattern, Some(target_ty));
                    if self.is_open_enum(target_ty)
                        && self.check_integer_literal_enum_backing_range(
                            pattern,
                            target_ty,
                            "switch pattern",
                        )
                    {
                        self.expr_types.insert(pattern.span, target_ty);
                    } else {
                        self.expect_expr_type(pattern, target_ty, pattern_ty, "switch pattern");
                    }
                    if let Some(expected_enum) = enum_id
                        && let Some((variant_enum, variant_id)) = self.enum_variant_info(pattern)
                        && variant_enum == expected_enum
                    {
                        covered_variants.insert(variant_id);
                    }
                }
            }
            match &arm.body {
                nia_ast::SwitchArmBody::Expr(expr) => {
                    self.check_expr(expr);
                }
                nia_ast::SwitchArmBody::Stmt(stmt) => self.check_stmt(stmt),
                nia_ast::SwitchArmBody::Block(block) => {
                    self.check_block(block);
                }
            }
        }

        if let Some(enum_id) = enum_id {
            self.check_enum_switch_exhaustive(
                switch.target.span,
                enum_id,
                has_default,
                &covered_variants,
            );
        }
    }
}

pub(crate) fn generic_inst_base(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::BracketSuffix { callee, .. } => callee,
        _ => expr,
    }
}

#[cfg(test)]
mod tests;
