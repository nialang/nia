// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

mod aggregates;
mod bir;
mod calls;
mod expr;
mod helpers;
mod literals;
mod places;
mod projection_obligations;
mod static_init;
mod trait_objects;
mod type_support;

pub use nia_ty::import_type_into;

use nia_ast::{
    BindingStmt, Block, Expr, ExprKind, FunctionItem, ItemKind, Module, SliceRange, Stmt, StmtKind,
};
use nia_body_ir::{
    ArrayToSliceCoercion, BodyFacts, BodyIr, BracketSuffixResolution, BuiltinValue,
    CStringPointerCoercion, ComptimeIfSelection, FunctionReference, GenericInstantiation,
    ResolvedCall, TraitObjectCoercion, TraitObjectUpcast,
};
use nia_comptime_check::ComptimeCheck;
use nia_defs::{DefCollection, DefId, DefKind, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_item_signatures::{
    EnumSignature, FunctionSignature, ItemSignatures, ProgramComptimeSignature,
    ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature, ProgramSignatureMaps,
    ProgramStructSignature, ProgramTraitImplSignature, ProgramTraitSignature,
    ProgramUnionSignature, StructSignature, UnionSignature,
};
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_node_id::{NodeKey, NodeOriginTable, SyntaxKind};
use nia_source::SourceVersion;
use nia_span::Span;
use nia_ty::{PrimitiveTy, RangeTyKind, TyInterner, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

#[derive(Debug, Clone, PartialEq)]
pub struct BodyCheck {
    pub ir: BodyIr,
    pub facts: BodyFacts,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SwitchInterval {
    start: i128,
    end: i128,
    span: Span,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramComptimeMaps<'a> {
    pub comptimes: &'a HashMap<ModuleId, ComptimeCheck>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyProgramContext<'a> {
    pub modules: Option<&'a HashMap<ModuleId, Module>>,
    pub defs: Option<&'a HashMap<ModuleId, DefCollection>>,
}

impl<'a> BodyProgramContext<'a> {
    pub fn empty() -> Self {
        Self {
            modules: None,
            defs: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BodyCheckInput<'a> {
    pub source_version: Option<SourceVersion>,
    pub origins: &'a NodeOriginTable,
    pub module: &'a Module,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub comptime: &'a ComptimeCheck,
    pub layouts: &'a Layouts,
    pub extensions: &'a VisibleExtensionMethods,
    pub extension_interner: Option<&'a TyInterner>,
    pub program: BodyProgramContext<'a>,
    pub program_signatures: ProgramSignatureMaps<'a>,
    pub program_comptime: ProgramComptimeMaps<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyCheckWithProgramSignaturesInput<'a> {
    pub source_version: Option<SourceVersion>,
    pub origins: &'a NodeOriginTable,
    pub module: &'a Module,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub comptime: &'a ComptimeCheck,
    pub extensions: &'a VisibleExtensionMethods,
    pub program: BodyProgramContext<'a>,
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
    let empty_comptimes = HashMap::new();
    let empty_program_comptime = HashMap::new();
    let empty_structs = HashMap::new();
    let empty_unions = HashMap::new();
    let empty_enums = HashMap::new();
    let empty_traits = HashMap::new();
    let empty_trait_impls = Vec::new();
    let empty_extensions = VisibleExtensionMethods::default();
    let empty_comptime = ComptimeCheck::default();
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        source_version: None,
        origins: &NodeOriginTable::default(),
        module,
        defs,
        values,
        locals,
        lowered,
        signatures,
        normalization: &empty_normalization,
        comptime: &empty_comptime,
        layouts: &layouts,
        extensions: &empty_extensions,
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: ProgramSignatureMaps {
            functions: &empty_functions,
            globals: &empty_globals,
            comptimes: &empty_comptimes,
            structs: &empty_structs,
            unions: &empty_unions,
            enums: &empty_enums,
            traits: &empty_traits,
            trait_impls: &empty_trait_impls,
        },
        program_comptime: ProgramComptimeMaps {
            comptimes: &empty_program_comptime,
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
        &|id| input.comptime.array_lengths.get(&id).copied(),
        nia_layout::TargetDataLayout::LP64,
    );
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        source_version: input.source_version,
        origins: input.origins,
        module: input.module,
        defs: input.defs,
        values: input.values,
        locals: input.locals,
        lowered: input.lowered,
        signatures: input.signatures,
        normalization: input.normalization,
        comptime: input.comptime,
        layouts: &layouts,
        extensions: input.extensions,
        extension_interner: None,
        program: input.program,
        program_signatures: input.program_signatures,
        program_comptime: ProgramComptimeMaps {
            comptimes: &HashMap::new(),
        },
    });
    checked.diagnostics.extend(layouts.diagnostics);
    checked
}

pub fn check_module_bodies_with_program_signatures_and_layouts(
    input: BodyCheckInput<'_>,
) -> BodyCheck {
    let mut checker = BodyChecker {
        source_version: input.source_version,
        origins: input.origins,
        module: input.module,
        defs: input.defs,
        program: input.program,
        values: input.values,
        locals: input.locals,
        interner: input
            .extension_interner
            .cloned()
            .unwrap_or_else(|| input.normalization.interner.clone()),
        type_uses: &input.lowered.type_uses,
        signatures: input.signatures,
        normalization: input.normalization,
        comptime: input.comptime,
        layouts: input.layouts,
        extensions: input.extensions,
        program_functions: input.program_signatures.functions,
        program_globals: input.program_signatures.globals,
        program_comptimes: input.program_signatures.comptimes,
        program_structs: input.program_signatures.structs,
        program_unions: input.program_signatures.unions,
        program_enums: input.program_signatures.enums,
        program_traits: input.program_signatures.traits,
        program_trait_impls: input.program_signatures.trait_impls,
        program_comptime: input.program_comptime.comptimes,
        expr_types: HashMap::new(),
        bracket_suffix_resolutions: HashMap::new(),
        array_to_slice_coercions: HashMap::new(),
        c_string_pointer_coercions: HashMap::new(),
        trait_object_coercions: HashMap::new(),
        trait_object_upcasts: HashMap::new(),
        builtin_values: HashMap::new(),
        resolved_calls: HashMap::new(),
        function_references: HashMap::new(),
        node_expr_types: HashMap::new(),
        node_bracket_suffix_resolutions: HashMap::new(),
        node_array_to_slice_coercions: HashMap::new(),
        node_c_string_pointer_coercions: HashMap::new(),
        node_trait_object_coercions: HashMap::new(),
        node_trait_object_upcasts: HashMap::new(),
        node_builtin_values: HashMap::new(),
        node_resolved_calls: HashMap::new(),
        node_function_references: HashMap::new(),
        generic_instantiations: Vec::new(),
        function_bodies: HashMap::new(),
        global_inits: HashMap::new(),
        local_types: HashMap::new(),
        global_types: HashMap::new(),
        comptime_types: HashMap::new(),
        comptime_if_selections: HashMap::new(),
        diagnostics: Vec::new(),
        current_return: input.normalization.interner.primitive(PrimitiveTy::Void),
        current_def_id: None,
        current_param_locals: Vec::new(),
        comptime_context_depth: 0,
        comptime_call_locals: Vec::new(),
    };
    checker.seed_global_types();
    checker.check_module(input.module);
    BodyCheck {
        ir: BodyIr {
            interner: checker.interner,
            function_bodies: checker.function_bodies,
            global_inits: checker.global_inits,
        },
        facts: BodyFacts {
            expr_types: checker.expr_types,
            bracket_suffix_resolutions: checker.bracket_suffix_resolutions,
            array_to_slice_coercions: checker.array_to_slice_coercions,
            c_string_pointer_coercions: checker.c_string_pointer_coercions,
            trait_object_coercions: checker.trait_object_coercions,
            trait_object_upcasts: checker.trait_object_upcasts,
            local_types: checker.local_types,
            comptime_if_selections: checker.comptime_if_selections,
            builtin_values: checker.builtin_values,
            resolved_calls: checker.resolved_calls,
            function_references: checker.function_references,
            generic_instantiations: checker.generic_instantiations,
            node_expr_types: checker.node_expr_types,
            node_bracket_suffix_resolutions: checker.node_bracket_suffix_resolutions,
            node_array_to_slice_coercions: checker.node_array_to_slice_coercions,
            node_c_string_pointer_coercions: checker.node_c_string_pointer_coercions,
            node_trait_object_coercions: checker.node_trait_object_coercions,
            node_trait_object_upcasts: checker.node_trait_object_upcasts,
            node_builtin_values: checker.node_builtin_values,
            node_resolved_calls: checker.node_resolved_calls,
            node_function_references: checker.node_function_references,
        },
        diagnostics: checker.diagnostics,
    }
}

struct BodyChecker<'a> {
    source_version: Option<SourceVersion>,
    origins: &'a NodeOriginTable,
    module: &'a Module,
    defs: &'a DefCollection,
    program: BodyProgramContext<'a>,
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    interner: TyInterner,
    type_uses: &'a HashMap<Span, InternedTyId>,
    signatures: &'a ItemSignatures,
    normalization: &'a TypeNormalization,
    comptime: &'a ComptimeCheck,
    layouts: &'a Layouts,
    extensions: &'a VisibleExtensionMethods,
    program_functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    program_globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    program_comptimes: &'a HashMap<GlobalDefId, ProgramComptimeSignature>,
    program_structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    program_unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    program_enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    program_traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    program_trait_impls: &'a [ProgramTraitImplSignature],
    program_comptime: &'a HashMap<ModuleId, ComptimeCheck>,
    expr_types: HashMap<Span, InternedTyId>,
    bracket_suffix_resolutions: HashMap<Span, BracketSuffixResolution>,
    array_to_slice_coercions: HashMap<Span, ArrayToSliceCoercion>,
    c_string_pointer_coercions: HashMap<Span, CStringPointerCoercion>,
    trait_object_coercions: HashMap<Span, TraitObjectCoercion>,
    trait_object_upcasts: HashMap<Span, TraitObjectUpcast>,
    builtin_values: HashMap<Span, BuiltinValue>,
    resolved_calls: HashMap<Span, ResolvedCall>,
    function_references: HashMap<Span, FunctionReference>,
    node_expr_types: HashMap<NodeKey, InternedTyId>,
    node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    node_c_string_pointer_coercions: HashMap<NodeKey, CStringPointerCoercion>,
    node_trait_object_coercions: HashMap<NodeKey, TraitObjectCoercion>,
    node_trait_object_upcasts: HashMap<NodeKey, TraitObjectUpcast>,
    node_builtin_values: HashMap<NodeKey, BuiltinValue>,
    node_resolved_calls: HashMap<NodeKey, ResolvedCall>,
    node_function_references: HashMap<NodeKey, FunctionReference>,
    generic_instantiations: Vec<GenericInstantiation>,
    function_bodies: HashMap<GlobalDefId, nia_body_ir::TypedBody>,
    global_inits: HashMap<GlobalDefId, nia_static_ir::StaticInit>,
    local_types: HashMap<LocalId, InternedTyId>,
    global_types: HashMap<DefId, InternedTyId>,
    comptime_types: HashMap<DefId, InternedTyId>,
    comptime_if_selections: HashMap<Span, ComptimeIfSelection>,
    diagnostics: Vec<Diagnostic>,
    current_return: InternedTyId,
    current_def_id: Option<GlobalDefId>,
    current_param_locals: Vec<LocalId>,
    comptime_context_depth: usize,
    comptime_call_locals: Vec<HashMap<LocalId, nia_comptime_check::ComptimeValue>>,
}

#[derive(Debug, Clone, PartialEq)]
struct ReceiverBase {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    from_pointer: bool,
    has_readonly_pointer: bool,
}

impl<'a> BodyChecker<'a> {
    fn record_expr_type(&mut self, span: Span, ty: InternedTyId) {
        self.expr_types.insert(span, ty);
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_expr_types.insert(key, ty);
        }
    }

    fn record_bracket_suffix_resolution(
        &mut self,
        span: Span,
        resolution: BracketSuffixResolution,
    ) {
        self.bracket_suffix_resolutions.insert(span, resolution);
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_bracket_suffix_resolutions.insert(key, resolution);
        }
    }

    fn record_resolved_call(&mut self, span: Span, call: ResolvedCall) {
        self.resolved_calls.insert(span, call.clone());
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_resolved_calls.insert(key, call);
        }
    }

    fn record_array_to_slice_coercion(&mut self, span: Span, coercion: ArrayToSliceCoercion) {
        self.array_to_slice_coercions.insert(span, coercion);
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_array_to_slice_coercions.insert(key, coercion);
        }
    }

    fn record_c_string_pointer_coercion(&mut self, span: Span, coercion: CStringPointerCoercion) {
        self.c_string_pointer_coercions.insert(span, coercion);
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_c_string_pointer_coercions.insert(key, coercion);
        }
    }

    fn record_trait_object_coercion(&mut self, span: Span, coercion: TraitObjectCoercion) {
        self.trait_object_coercions.insert(span, coercion);
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_trait_object_coercions.insert(key, coercion);
        }
    }

    fn record_trait_object_upcast(&mut self, span: Span, upcast: TraitObjectUpcast) {
        self.trait_object_upcasts.insert(span, upcast);
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_trait_object_upcasts.insert(key, upcast);
        }
    }

    fn record_builtin_value(&mut self, span: Span, value: BuiltinValue) {
        self.builtin_values.insert(span, value.clone());
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_builtin_values.insert(key, value);
        }
    }

    fn record_function_reference(&mut self, span: Span, reference: FunctionReference) {
        self.function_references.insert(span, reference.clone());
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_function_references.insert(key, reference);
        }
    }

    fn node_key(&self, kind: SyntaxKind, span: Span) -> Option<NodeKey> {
        self.origins.get(kind, span).cloned().or_else(|| {
            self.source_version
                .map(|version| NodeKey::span(version, kind, span))
        })
    }

    fn with_comptime_context<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.comptime_context_depth += 1;
        let result = f(self);
        self.comptime_context_depth -= 1;
        result
    }

    fn in_comptime_context(&self) -> bool {
        self.comptime_context_depth > 0
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<&DefCollection> {
        if module_id == self.defs.module_id {
            Some(self.defs)
        } else {
            self.program.defs?.get(&module_id)
        }
    }

    fn module_for_module(&self, module_id: ModuleId) -> Option<&Module> {
        if module_id == self.defs.module_id {
            Some(self.module)
        } else {
            self.program.modules?.get(&module_id)
        }
    }
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
                if binding.is_comptime {
                    self.check_comptime_binding(item.span, binding);
                } else {
                    self.check_global_binding(item.span, binding);
                }
            }
        }
        for item in &module.items {
            if let ItemKind::Function(function) = &item.kind {
                self.check_function_item(item.span, function);
            }
        }
        for item in &module.items {
            if let ItemKind::Trait(item_trait) = &item.kind {
                for method in &item_trait.methods {
                    self.check_trait_function_def(method.function.span, &method.function);
                }
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
        for (def_id, signature) in &self.signatures.comptimes {
            if let Some(ty) = signature.explicit_type {
                self.comptime_types.insert(*def_id, ty);
            }
        }
    }

    fn check_comptime_binding(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Comptime) else {
            return;
        };
        let Some(value) = &binding.value else {
            self.diagnostics.push(Diagnostic::error(
                item_span,
                "comptime binding requires an initializer",
            ));
            return;
        };
        let comptime_ty = match binding.ty.as_ref() {
            Some(ty) => {
                let explicit = self.ty_for_span(ty.span);
                let value_ty = self.with_comptime_context(|this| {
                    this.check_expr_with_expected(value, Some(explicit))
                });
                if !self.is_comptime_only_ty(value_ty) {
                    self.expect_expr_type(value, explicit, value_ty, "comptime initializer");
                }
                self.materialize_inferred_array_type(explicit, value_ty)
                    .unwrap_or(explicit)
            }
            None => {
                if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    self.with_comptime_context(|this| this.infer_array_literal_expr(value))
                } else {
                    self.with_comptime_context(|this| this.check_expr(value))
                }
            }
        };
        self.comptime_types.insert(def_id, comptime_ty);
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
                if self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "global initializer");
                    self.error()
                } else {
                    self.expect_expr_type(value, explicit, value_ty, "global initializer");
                    self.materialize_inferred_array_type(explicit, value_ty)
                        .unwrap_or(explicit)
                }
            }
            None => {
                let value_ty = if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    self.infer_array_literal_expr(value)
                } else {
                    self.check_expr(value)
                };
                if self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "global initializer");
                    self.error()
                } else {
                    value_ty
                }
            }
        };
        self.global_types.insert(def_id, global_ty);
        if global_ty != self.error() {
            let init = self.lower_static_init(value);
            self.global_inits.insert(self.global_def_id(def_id), init);
        }
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

    fn check_trait_function_def(&mut self, span: Span, function: &FunctionItem) {
        let Some(def_id) = self.def_id_for_span(span, DefKind::TraitMethod) else {
            return;
        };
        self.check_function(def_id, function);
    }

    fn check_function(&mut self, def_id: DefId, function: &FunctionItem) {
        let Some(signature) = self.signatures.functions.get(&def_id) else {
            return;
        };
        self.check_function_signature_projection_obligations(def_id, signature);
        let previous_return = self.current_return;
        let previous_def_id = self.current_def_id;
        let previous_param_locals = std::mem::take(&mut self.current_param_locals);
        self.current_return = signature.return_type;
        self.current_def_id = Some(self.global_def_id(def_id));
        let self_ty = self.method_self_type(def_id, signature);
        self.check_object_safe_types_in_signature(signature);
        self.seed_param_types(signature, function, self_ty);
        if signature.is_comptime {
            self.current_return = previous_return;
            self.current_def_id = previous_def_id;
            self.current_param_locals = previous_param_locals;
            return;
        }
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
            let body = self.lower_body(body);
            self.function_bodies
                .insert(self.global_def_id(def_id), body);
        }
        self.current_return = previous_return;
        self.current_def_id = previous_def_id;
        self.current_param_locals = previous_param_locals;
    }

    fn check_object_safe_types_in_signature(&mut self, signature: &FunctionSignature) {
        for param in &signature.params {
            self.check_object_safe_type(param.span, param.ty);
        }
        self.check_object_safe_type(signature.span, signature.return_type);
    }

    fn seed_param_types(
        &mut self,
        signature: &FunctionSignature,
        function: &FunctionItem,
        self_ty: Option<InternedTyId>,
    ) {
        for (param, param_sig) in function.params.iter().zip(&signature.params) {
            if let Some(local_id) = self.locals.local_defs.get(&param.span).copied() {
                let ty = if param_sig.receiver.is_some() {
                    self_ty.unwrap_or_else(|| self.error())
                } else {
                    param_sig.ty
                };
                self.local_types.insert(local_id, ty);
                self.current_param_locals.push(local_id);
            }
        }
    }

    fn check_block(&mut self, block: &Block) -> InternedTyId {
        self.check_block_with_expected(block, None)
    }

    fn check_block_with_expected(
        &mut self,
        block: &Block,
        expected_tail: Option<InternedTyId>,
    ) -> InternedTyId {
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
        } else if self.block_ends_with_never_stmt(block) {
            self.never()
        } else {
            self.void()
        }
    }

    fn block_ends_with_never_stmt(&self, block: &Block) -> bool {
        let Some(stmt) = block.stmts.last() else {
            return false;
        };
        match &stmt.kind {
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue => true,
            StmtKind::Expr(expr) => self
                .expr_types
                .get(&expr.span)
                .is_some_and(|ty| self.is_never(*ty)),
            StmtKind::Binding(_)
            | StmtKind::Using(_)
            | StmtKind::Defer(_)
            | StmtKind::ForIn(_)
            | StmtKind::While(_)
            | StmtKind::Loop(_) => false,
        }
    }

    fn is_empty_struct_type(&mut self, def_id: GlobalDefId, args: &[InternedTyId]) -> bool {
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
            StmtKind::ForIn(for_stmt) => {
                let explicit_binding_ty = for_stmt
                    .binding
                    .ty
                    .as_ref()
                    .map(|explicit| self.ty_for_span(explicit.span));
                let expected_iter_ty = explicit_binding_ty
                    .and_then(|item_ty| self.expected_for_iterator_ty(&for_stmt.iter, item_ty));
                let iter_ty = self.check_expr_with_expected(&for_stmt.iter, expected_iter_ty);
                let item_ty = self.for_iterator_item_type(&for_stmt.iter, iter_ty);
                let binding_ty = if let Some(explicit_ty) = explicit_binding_ty {
                    self.expect_type(for_stmt.binding.span, explicit_ty, item_ty, "for binding");
                    explicit_ty
                } else {
                    item_ty
                };
                if let Some(local_id) = self.locals.local_defs.get(&for_stmt.binding.span).copied()
                {
                    self.local_types.insert(local_id, binding_ty);
                }
                self.check_block(&for_stmt.body);
            }
            StmtKind::While(while_stmt) => {
                let cond_ty = self.check_expr(&while_stmt.cond);
                self.expect_type(
                    while_stmt.cond.span,
                    self.bool(),
                    cond_ty,
                    "while condition",
                );
                self.check_block(&while_stmt.body);
            }
            StmtKind::Loop(loop_stmt) => {
                self.check_block(&loop_stmt.body);
            }
        }
    }

    fn expected_for_iterator_ty(
        &mut self,
        iter: &Expr,
        item_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        let ExprKind::Range(range) = &iter.kind else {
            return None;
        };
        let kind = self.for_range_kind(range)?;
        Some(self.interner.intern(TyKind::Range {
            kind,
            bound: Some(item_ty),
        }))
    }

    fn for_range_kind(&self, range: &SliceRange) -> Option<RangeTyKind> {
        match (range.start.is_some(), range.end.is_some(), range.inclusive) {
            (true, true, false) => Some(RangeTyKind::Exclusive),
            (true, true, true) => Some(RangeTyKind::Inclusive),
            (true, false, false) => Some(RangeTyKind::From),
            _ => None,
        }
    }

    fn for_iterator_item_type(&mut self, iter: &Expr, iter_ty: InternedTyId) -> InternedTyId {
        match self.interner.get(iter_ty).cloned() {
            Some(TyKind::Range {
                kind:
                    nia_ty::RangeTyKind::Exclusive
                    | nia_ty::RangeTyKind::Inclusive
                    | nia_ty::RangeTyKind::From,
                bound: Some(bound),
            }) => bound,
            Some(TyKind::Range { bound: Some(_), .. }) => {
                self.diagnostics.push(Diagnostic::error(
                    iter.span,
                    "for-in range iterator requires a start bound",
                ));
                self.error()
            }
            Some(TyKind::Range { bound: None, .. }) => {
                self.diagnostics.push(Diagnostic::error(
                    iter.span,
                    "unbounded range cannot be used as a for iterator",
                ));
                self.error()
            }
            Some(_) | None => {
                self.diagnostics.push(Diagnostic::error(
                    iter.span,
                    "for-in expects an iterator expression; only bounded ranges are supported currently",
                ));
                self.error()
            }
        }
    }

    fn check_local_binding(&mut self, span: Span, binding: &BindingStmt) {
        if binding.is_comptime && binding.value.is_none() {
            self.diagnostics.push(Diagnostic::error(
                span,
                "comptime binding requires an initializer",
            ));
        }
        let binding_ty = match (&binding.ty, &binding.value) {
            (Some(ty), Some(value)) => {
                let explicit = self.ty_for_span(ty.span);
                let value_ty = if binding.is_comptime {
                    self.with_comptime_context(|this| {
                        this.check_expr_with_expected(value, Some(explicit))
                    })
                } else {
                    self.check_expr_with_expected(value, Some(explicit))
                };
                if binding.is_comptime && self.is_comptime_only_ty(value_ty) {
                    // The initializer is validated by nia-comptime-check and has no runtime value.
                } else if self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "binding initializer");
                    return self.record_error_local_binding(span);
                } else {
                    self.expect_expr_type(value, explicit, value_ty, "binding initializer");
                }
                self.materialize_inferred_array_type(explicit, value_ty)
                    .unwrap_or(explicit)
            }
            (Some(ty), None) => self.ty_for_span(ty.span),
            (None, Some(value)) => {
                let value_ty = if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    if binding.is_comptime {
                        self.with_comptime_context(|this| this.infer_array_literal_expr(value))
                    } else {
                        self.infer_array_literal_expr(value)
                    }
                } else {
                    if binding.is_comptime {
                        self.with_comptime_context(|this| this.check_expr(value))
                    } else {
                        self.check_expr(value)
                    }
                };
                if !binding.is_comptime && self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "binding initializer");
                    self.error()
                } else {
                    value_ty
                }
            }
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

    fn reject_runtime_comptime_only_value(&mut self, span: Span, context: &str) {
        self.diagnostics.push(Diagnostic::error(
            span,
            format!("{context} cannot use comptime-only value"),
        ));
    }

    fn record_error_local_binding(&mut self, span: Span) {
        if let Some(local_id) = self.locals.local_defs.get(&span).copied() {
            self.local_types.insert(local_id, self.error());
        }
    }

    pub(crate) fn check_switch_expr(
        &mut self,
        switch: &nia_ast::SwitchStmt,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let target_ty = self.check_expr(&switch.target);
        let enum_id = self.enum_global_def_id(target_ty);
        let mut has_default = false;
        let mut covered_variants = HashSet::new();
        let mut covered_intervals = Vec::new();
        let mut covered_enum_variants = HashMap::<DefId, Span>::new();
        let mut result_ty = expected;

        for (arm_index, arm) in switch.arms.iter().enumerate() {
            if has_default {
                self.diagnostics.push(Diagnostic::error(
                    arm.span,
                    "switch arm is unreachable because `_` default appears earlier",
                ));
            }
            for pattern in &arm.patterns {
                match pattern {
                    nia_ast::SwitchPattern::Default => {
                        if arm.patterns.len() != 1 {
                            self.diagnostics.push(Diagnostic::error(
                                arm.span,
                                "`_` default must be the only pattern in a switch arm",
                            ));
                        }
                        has_default = true;
                    }
                    nia_ast::SwitchPattern::OptionalSome { span, .. } => {
                        self.check_switch_binding_pattern_is_single(*span, arm);
                        self.check_switch_optional_some_pattern(
                            *span,
                            target_ty,
                            &mut covered_intervals,
                        );
                    }
                    nia_ast::SwitchPattern::OptionalNull { span } => {
                        self.check_switch_optional_null_pattern(
                            *span,
                            target_ty,
                            &mut covered_intervals,
                        );
                    }
                    nia_ast::SwitchPattern::ErrorOk { span, .. } => {
                        self.check_switch_binding_pattern_is_single(*span, arm);
                        self.check_switch_error_ok_pattern(
                            *span,
                            target_ty,
                            &mut covered_intervals,
                        );
                    }
                    nia_ast::SwitchPattern::ErrorErr { span, .. } => {
                        self.check_switch_binding_pattern_is_single(*span, arm);
                        self.check_switch_error_err_pattern(
                            *span,
                            target_ty,
                            &mut covered_intervals,
                        );
                    }
                    nia_ast::SwitchPattern::Expr(pattern) => {
                        self.check_switch_expr_pattern(
                            pattern,
                            target_ty,
                            enum_id,
                            &mut covered_variants,
                            &mut covered_enum_variants,
                            &mut covered_intervals,
                        );
                    }
                    nia_ast::SwitchPattern::Range {
                        start,
                        end,
                        inclusive,
                        span,
                    } => {
                        self.check_switch_range_pattern(
                            *span,
                            start,
                            end,
                            *inclusive,
                            target_ty,
                            &mut covered_intervals,
                        );
                    }
                }
            }
            self.record_switch_pattern_local_types(&arm.patterns, target_ty);
            if has_default && arm_index + 1 != switch.arms.len() {
                // The following arm will get the concrete unreachable diagnostic
                // above; this branch only keeps the default state explicit.
            }
            let arm_ty = self.check_switch_arm_body(&arm.body, result_ty);
            if let Some(expected) = result_ty {
                self.expect_switch_arm_type(&arm.body, expected, arm_ty);
            } else if !self.is_never(arm_ty) {
                result_ty = Some(arm_ty);
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
        self.check_optional_error_switch_exhaustive(
            switch.target.span,
            target_ty,
            has_default,
            &covered_intervals,
        );
        result_ty.unwrap_or_else(|| self.void())
    }

    fn check_switch_binding_pattern_is_single(&mut self, span: Span, arm: &nia_ast::SwitchArm) {
        if arm.patterns.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                span,
                "switch pattern binding must be the only pattern in its arm",
            ));
        }
    }

    fn record_switch_pattern_local_types(
        &mut self,
        patterns: &[nia_ast::SwitchPattern],
        target_ty: InternedTyId,
    ) {
        let normalized = self.normalization.normalize(target_ty);
        for pattern in patterns {
            let (span, ty) = match pattern {
                nia_ast::SwitchPattern::OptionalSome { span, .. } => {
                    let ty = match self.interner.get(normalized) {
                        Some(TyKind::Optional { elem }) => *elem,
                        _ => self.error(),
                    };
                    (*span, ty)
                }
                nia_ast::SwitchPattern::ErrorOk { span, .. } => {
                    let ty = match self.interner.get(normalized) {
                        Some(TyKind::ErrorUnion { value, .. }) => *value,
                        _ => self.error(),
                    };
                    (*span, ty)
                }
                nia_ast::SwitchPattern::ErrorErr { span, .. } => {
                    let ty = match self.interner.get(normalized) {
                        Some(TyKind::ErrorUnion { error, .. }) => *error,
                        _ => self.error(),
                    };
                    (*span, ty)
                }
                _ => continue,
            };
            if let Some(local_id) = self.locals.local_defs.get(&span).copied() {
                self.local_types.insert(local_id, ty);
            }
        }
    }

    fn check_switch_optional_some_pattern(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        match self.interner.get(self.normalization.normalize(target_ty)) {
            Some(TyKind::Optional { .. }) => self.check_switch_interval_overlap(
                SwitchInterval {
                    start: 1,
                    end: 1,
                    span,
                },
                covered_intervals,
            ),
            _ => self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "`?name` switch pattern requires an optional target, found `{}`",
                    self.ty_name(target_ty)
                ),
            )),
        }
    }

    fn check_switch_optional_null_pattern(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        match self.interner.get(self.normalization.normalize(target_ty)) {
            Some(TyKind::Optional { .. }) => self.check_switch_interval_overlap(
                SwitchInterval {
                    start: 0,
                    end: 0,
                    span,
                },
                covered_intervals,
            ),
            _ => self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "`null` switch pattern requires an optional target, found `{}`",
                    self.ty_name(target_ty)
                ),
            )),
        }
    }

    fn check_switch_error_ok_pattern(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        match self.interner.get(self.normalization.normalize(target_ty)) {
            Some(TyKind::ErrorUnion { .. }) => self.check_switch_interval_overlap(
                SwitchInterval {
                    start: 0,
                    end: 0,
                    span,
                },
                covered_intervals,
            ),
            _ => self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "`!name` switch pattern requires an error union target, found `{}`",
                    self.ty_name(target_ty)
                ),
            )),
        }
    }

    fn check_switch_error_err_pattern(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        match self.interner.get(self.normalization.normalize(target_ty)) {
            Some(TyKind::ErrorUnion { .. }) => self.check_switch_interval_overlap(
                SwitchInterval {
                    start: 1,
                    end: 1,
                    span,
                },
                covered_intervals,
            ),
            _ => self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "`name!` switch pattern requires an error union target, found `{}`",
                    self.ty_name(target_ty)
                ),
            )),
        }
    }

    fn check_optional_error_switch_exhaustive(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        has_default: bool,
        covered_intervals: &[SwitchInterval],
    ) {
        if has_default {
            return;
        }
        let normalized = self.normalization.normalize(target_ty);
        let Some(kind) = (match self.interner.get(normalized) {
            Some(TyKind::Optional { .. }) => Some("optional"),
            Some(TyKind::ErrorUnion { .. }) => Some("error union"),
            _ => None,
        }) else {
            return;
        };
        let covers = |tag: i128| {
            covered_intervals
                .iter()
                .any(|interval| interval.start <= tag && tag <= interval.end)
        };
        if !covers(0) || !covers(1) {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("non-exhaustive {kind} switch"),
            ));
        }
    }

    fn check_switch_expr_pattern(
        &mut self,
        pattern: &Expr,
        target_ty: InternedTyId,
        enum_id: Option<GlobalDefId>,
        covered_variants: &mut HashSet<DefId>,
        covered_enum_variants: &mut HashMap<DefId, Span>,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        let pattern_ty = self.check_expr_with_expected(pattern, Some(target_ty));
        if self.is_open_enum(target_ty)
            && self.check_integer_literal_enum_backing_range(pattern, target_ty, "switch pattern")
        {
            self.record_expr_type(pattern.span, target_ty);
        } else {
            self.expect_expr_type(pattern, target_ty, pattern_ty, "switch pattern");
        }
        if let Some(expected_enum) = enum_id
            && let Some((variant_enum, variant_id)) = self.enum_variant_info(pattern)
            && variant_enum == expected_enum
        {
            if let Some(previous) = covered_enum_variants.insert(variant_id, pattern.span) {
                self.diagnostics.push(Diagnostic::error(
                    pattern.span,
                    format!("switch pattern overlaps previous pattern at {previous:?}"),
                ));
            }
            covered_variants.insert(variant_id);
            return;
        }
        if self.is_integer(target_ty) || self.is_bool(target_ty) {
            let Some(value) = self.switch_pattern_int_value(pattern) else {
                self.diagnostics.push(Diagnostic::error(
                    pattern.span,
                    "switch pattern must be a compile-time integer constant",
                ));
                return;
            };
            self.check_switch_interval_overlap(
                SwitchInterval {
                    start: value,
                    end: value,
                    span: pattern.span,
                },
                covered_intervals,
            );
        }
    }

    fn check_switch_range_pattern(
        &mut self,
        span: Span,
        start: &Expr,
        end: &Expr,
        inclusive: bool,
        target_ty: InternedTyId,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        if !self.is_integer(target_ty) {
            self.diagnostics.push(Diagnostic::error(
                span,
                "switch range patterns require an integer switch target",
            ));
        }
        let start_ty = self.check_expr_with_expected(start, Some(target_ty));
        self.expect_expr_type(start, target_ty, start_ty, "switch range pattern");
        let end_ty = self.check_expr_with_expected(end, Some(target_ty));
        self.expect_expr_type(end, target_ty, end_ty, "switch range pattern");
        let Some(start_value) = self.switch_pattern_int_value(start) else {
            self.diagnostics.push(Diagnostic::error(
                start.span,
                "switch range start must be a compile-time integer constant",
            ));
            return;
        };
        let Some(end_value) = self.switch_pattern_int_value(end) else {
            self.diagnostics.push(Diagnostic::error(
                end.span,
                "switch range end must be a compile-time integer constant",
            ));
            return;
        };
        let Some(end_inclusive) = (if inclusive {
            Some(end_value)
        } else {
            end_value.checked_sub(1)
        }) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "switch range pattern endpoint is out of range",
            ));
            return;
        };
        if start_value > end_inclusive {
            self.diagnostics
                .push(Diagnostic::error(span, "switch range pattern is empty"));
            return;
        }
        self.check_switch_interval_overlap(
            SwitchInterval {
                start: start_value,
                end: end_inclusive,
                span,
            },
            covered_intervals,
        );
    }

    fn switch_pattern_int_value(&mut self, expr: &Expr) -> Option<i128> {
        if let ExprKind::Bool(value) = expr.kind {
            return Some(if value { 1 } else { 0 });
        }
        match self
            .with_comptime_context(|this| nia_comptime_engine::eval_expr(expr, this))
            .ok()?
        {
            nia_comptime_engine::ComptimeValue::Int(value) => Some(value),
            _ => None,
        }
    }

    fn check_switch_interval_overlap(
        &mut self,
        interval: SwitchInterval,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        if let Some(previous) = covered_intervals
            .iter()
            .find(|previous| interval.start <= previous.end && previous.start <= interval.end)
        {
            self.diagnostics.push(Diagnostic::error(
                interval.span,
                format!(
                    "switch pattern overlaps previous pattern at {:?}",
                    previous.span
                ),
            ));
        }
        covered_intervals.push(interval);
    }

    fn is_bool(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }

    fn check_switch_arm_body(
        &mut self,
        body: &nia_ast::SwitchArmBody,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        match body {
            nia_ast::SwitchArmBody::Expr(expr) => self.check_expr_with_expected(expr, expected),
            nia_ast::SwitchArmBody::Stmt(stmt) => {
                self.check_stmt(stmt);
                if matches!(
                    stmt.kind,
                    StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
                ) {
                    self.never()
                } else {
                    self.void()
                }
            }
            nia_ast::SwitchArmBody::Block(block) => self.check_block_with_expected(block, expected),
        }
    }

    fn expect_switch_arm_type(
        &mut self,
        body: &nia_ast::SwitchArmBody,
        expected: InternedTyId,
        actual: InternedTyId,
    ) {
        if self.is_never(actual) {
            return;
        }
        match body {
            nia_ast::SwitchArmBody::Expr(expr) => {
                self.expect_expr_type(expr, expected, actual, "switch arms");
            }
            nia_ast::SwitchArmBody::Block(block) => {
                self.expect_block_tail_type(block, expected, actual, "switch arms");
            }
            nia_ast::SwitchArmBody::Stmt(stmt) => {
                self.expect_type(stmt.span, expected, actual, "switch arms");
            }
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
