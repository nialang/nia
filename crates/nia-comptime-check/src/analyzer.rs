// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::{
    ComptimeArmType, ComptimeArrayLengths, ComptimeCheck, ComptimeEnumValues, ComptimeInput,
    ComptimeKey, ComptimeProgramContext, ComptimeTypedFacts, ComptimeValue, ComptimeValueFieldType,
    ComptimeValueType, ComptimeValues, TypedComptimeValue, resolved_pattern_local_id,
    support::{
        comptime_string_to_char_array, enum_next_value, float_literal_suffix_ty,
        int_const_in_i128_range, integer_literal_suffix_ty, integer_range, is_float_primitive,
        primitive_integer_layout, primitive_integer_range_for_target,
    },
};
use nia_comptime_engine::ComptimeError;
use nia_comptime_ir::{
    ComptimeBinaryOp, ComptimeNameResolution, ComptimeStringLiteral, ComptimeUnaryOp,
    ResolvedComptimeArrayElements, ResolvedComptimeArrayElementsKind, ResolvedComptimeBlock,
    ResolvedComptimeEnum, ResolvedComptimeExpr, ResolvedComptimeExprKind,
    ResolvedComptimeFieldInit, ResolvedComptimeFunction, ResolvedComptimeModule,
    ResolvedComptimePattern, ResolvedComptimePatternKind, ResolvedComptimeStmtKind,
    ResolvedComptimeSwitch, ResolvedComptimeSwitchArmBody, ResolvedComptimeSwitchArmBodyKind,
    ResolvedComptimeTypeArg,
};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{
    BuiltinFunction, BuiltinTraitMethod, GlobalConstExprId, GlobalDefId, InternedTyId,
    LayoutBuiltin, LocalId, ModuleId, ValueBuiltin,
};
use nia_item_signatures::{
    FunctionAttribute, FunctionSignature, GenericParamSignatureKind, ItemSignatures,
    ProgramTraitImplSignature, WherePredicateSignature,
};
use nia_local_resolve::LocalResolution;
use nia_sema::{
    ArityCheck, ArrayLiteralLenCheck, FieldSetCheck, NamedField, check_array_literal_len,
    check_exact_arity, check_required_field_set, check_value_field_set,
};
use nia_sema_ir::{AssociatedComptimeProjection, BuiltinAssociatedValue, SemanticUseTable};
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, symbol_text_or_unresolved};
use nia_target_config::TargetConfig;
use nia_trait_solve::{TraitGoal, TraitSolverContext};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, IntConst, PrimitiveTy, RangeTyKind, TraitId, TyInterner, TyKind,
    import_type_into,
};
use nia_value_resolve::ValueResolution;

mod context;
mod env_impl;
mod expr_types;
mod generics;
mod traits;
mod ty_substitution;
mod type_infer;

#[derive(Debug, Clone, Default)]
pub struct TypedComptimeFrame {
    pub module_id: Option<ModuleId>,
    pub function_id: Option<GlobalDefId>,
    pub local_types: HashMap<LocalId, ComptimeValueType>,
    pub type_substitutions: SymbolMap<InternedTyId>,
    pub const_substitutions: SymbolMap<ConstGenericArg>,
}

#[derive(Debug, Clone, Copy)]
pub struct TypedComptimeQueryInput<'a> {
    pub module: &'a ResolvedComptimeModule,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub symbols: &'a nia_symbol_table::SymbolTable,
    pub lowered: &'a nia_type_lower::TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub interner: &'a TyInterner,
    pub normalized: &'a HashMap<InternedTyId, InternedTyId>,
    pub target: &'a TargetConfig,
    pub source_path: &'a SourcePath,
    pub program: ComptimeProgramContext<'a>,
    pub typed_values: &'a HashMap<ComptimeKey, TypedComptimeValue>,
    pub array_lengths: &'a HashMap<GlobalConstExprId, u64>,
    pub frames: &'a [TypedComptimeFrame],
}

#[derive(Debug, Clone)]
pub struct ComptimeGenericInstantiation {
    pub type_substitutions: SymbolMap<InternedTyId>,
    pub const_substitutions: SymbolMap<ConstGenericArg>,
}

pub fn instantiate_resolved_comptime_function_generics(
    input: TypedComptimeQueryInput<'_>,
    span: Span,
    signature_module_id: ModuleId,
    signature: &FunctionSignature,
    type_args: &[ResolvedComptimeTypeArg],
    arg_exprs: &[ResolvedComptimeExpr],
    expected_return: Option<InternedTyId>,
) -> Result<ComptimeGenericInstantiation, ComptimeError> {
    let mut analyzer = Analyzer::for_typed_query(input);
    analyzer.instantiate_resolved_function_generics(
        span,
        signature_module_id,
        signature,
        type_args,
        arg_exprs,
        expected_return,
    )
}

pub fn infer_resolved_comptime_expr_type(
    input: TypedComptimeQueryInput<'_>,
    expr: &ResolvedComptimeExpr,
    expected: Option<InternedTyId>,
) -> Option<ComptimeValueType> {
    let mut analyzer = Analyzer::for_typed_query(input);
    analyzer.resolved_comptime_expr_type(expr, expected)
}

pub fn check_module_comptime(input: ComptimeInput<'_>) -> ComptimeCheck {
    let array_lengths = compute_module_comptime_array_lengths(input);
    let enum_values = compute_module_comptime_enum_values(input, array_lengths.clone());
    let values = compute_module_comptime_values(input, array_lengths.clone(), enum_values.clone());
    let typed_facts = compute_module_comptime_typed_facts(
        input,
        array_lengths.clone(),
        enum_values.clone(),
        values.clone(),
    );
    check_module_comptime_with_all_phases(input, array_lengths, enum_values, values, typed_facts)
}

pub fn compute_module_comptime_array_lengths(input: ComptimeInput<'_>) -> ComptimeArrayLengths {
    let mut analyzer = Analyzer::new(input);
    analyzer.analyze_array_lengths();
    ComptimeArrayLengths {
        interner: analyzer.finish_local_interner(),
        values: analyzer.array_lengths,
        diagnostics: analyzer.diagnostics,
    }
}

pub fn check_module_comptime_with_array_lengths(
    input: ComptimeInput<'_>,
    array_lengths: ComptimeArrayLengths,
) -> ComptimeCheck {
    let enum_values = compute_module_comptime_enum_values(input, array_lengths.clone());
    check_module_comptime_with_phases(input, array_lengths, enum_values)
}

pub fn compute_module_comptime_enum_values(
    input: ComptimeInput<'_>,
    array_lengths: ComptimeArrayLengths,
) -> ComptimeEnumValues {
    let mut analyzer = Analyzer::with_local_interner(input, array_lengths.interner);
    analyzer.array_lengths = array_lengths.values;
    analyzer.diagnostics = array_lengths.diagnostics;
    analyzer.analyze_enum_values();
    ComptimeEnumValues {
        interner: analyzer.finish_local_interner(),
        values: analyzer.enum_values,
        typed_values: analyzer.typed_enum_values,
        diagnostics: analyzer.diagnostics,
    }
}

pub fn check_module_comptime_with_phases(
    input: ComptimeInput<'_>,
    array_lengths: ComptimeArrayLengths,
    enum_values: ComptimeEnumValues,
) -> ComptimeCheck {
    let values = compute_module_comptime_values(input, array_lengths.clone(), enum_values.clone());
    let typed_facts = compute_module_comptime_typed_facts(
        input,
        array_lengths.clone(),
        enum_values.clone(),
        values.clone(),
    );
    check_module_comptime_with_all_phases(input, array_lengths, enum_values, values, typed_facts)
}

pub fn compute_module_comptime_values(
    input: ComptimeInput<'_>,
    array_lengths: ComptimeArrayLengths,
    enum_values: ComptimeEnumValues,
) -> ComptimeValues {
    let mut analyzer = Analyzer::with_local_interner(input, enum_values.interner);
    analyzer.array_lengths = array_lengths.values;
    analyzer.enum_values = enum_values.values;
    analyzer.typed_enum_values = enum_values.typed_values;
    analyzer.diagnostics = enum_values.diagnostics;
    analyzer.analyze_values();
    ComptimeValues {
        interner: analyzer.finish_local_interner(),
        values: analyzer.values,
        typed_values: analyzer.typed_values,
        diagnostics: analyzer.diagnostics,
    }
}

pub fn check_module_comptime_with_all_phases(
    input: ComptimeInput<'_>,
    array_lengths: ComptimeArrayLengths,
    enum_values: ComptimeEnumValues,
    values: ComptimeValues,
    typed_facts: ComptimeTypedFacts,
) -> ComptimeCheck {
    let diagnostics = typed_facts.diagnostics;
    let mut analyzer = Analyzer::with_local_interner(input, typed_facts.interner);
    analyzer.array_lengths = array_lengths.values;
    analyzer.enum_values = enum_values.values;
    analyzer.typed_enum_values = enum_values.typed_values;
    analyzer.values = values.values;
    analyzer.typed_values = typed_facts.typed_values;
    analyzer.diagnostics = diagnostics;
    ComptimeCheck {
        interner: analyzer.finish_local_interner(),
        values: analyzer.values,
        typed_values: analyzer.typed_values,
        enum_values: analyzer.enum_values,
        typed_enum_values: analyzer.typed_enum_values,
        array_lengths: analyzer.array_lengths,
        diagnostics: analyzer.diagnostics,
    }
}

pub fn compute_module_comptime_typed_facts(
    input: ComptimeInput<'_>,
    array_lengths: ComptimeArrayLengths,
    enum_values: ComptimeEnumValues,
    values: ComptimeValues,
) -> ComptimeTypedFacts {
    let mut analyzer = Analyzer::with_local_interner(input, values.interner);
    analyzer.array_lengths = array_lengths.values;
    analyzer.enum_values = enum_values.values;
    analyzer.typed_enum_values = enum_values.typed_values;
    analyzer.values = values.values;
    analyzer.typed_values = values.typed_values;
    analyzer.diagnostics = values.diagnostics;
    analyzer.analyze_functions();
    ComptimeTypedFacts {
        interner: analyzer.finish_local_interner(),
        typed_values: analyzer.typed_values,
        diagnostics: analyzer.diagnostics,
    }
}

pub(crate) struct Analyzer<'a> {
    input: ComptimeInput<'a>,
    values: HashMap<ComptimeKey, ComptimeValue>,
    typed_values: HashMap<ComptimeKey, TypedComptimeValue>,
    external_typed_values: Option<&'a HashMap<ComptimeKey, TypedComptimeValue>>,
    call_locals: Vec<ComptimeCallFrame>,
    execution_module_overrides: Vec<ModuleId>,
    enum_values: HashMap<DefId, ComptimeValue>,
    typed_enum_values: HashMap<DefId, TypedComptimeValue>,
    array_lengths: HashMap<GlobalConstExprId, u64>,
    diagnostics: Vec<Diagnostic>,
    active: HashSet<ComptimeKey>,
    working_interners: HashMap<ModuleId, TyInterner>,
    program_type_normalizations: RefCell<HashMap<ModuleId, nia_type_normalize::TypeNormalization>>,
    program_value_type_normalizations:
        RefCell<HashMap<ModuleId, nia_type_normalize::TypeNormalization>>,
    program_trait_impls: RefCell<HashMap<ModuleId, Vec<ProgramTraitImplSignature>>>,
    program_global_initializers:
        RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    resolved_call_type_substitutions: HashMap<Span, SymbolMap<InternedTyId>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ComptimeCallFrame {
    module_id: Option<ModuleId>,
    function_id: Option<GlobalDefId>,
    locals: HashMap<LocalId, ComptimeValue>,
    local_types: HashMap<LocalId, ComptimeValueType>,
    mutable_locals: HashSet<LocalId>,
    type_substitutions: SymbolMap<InternedTyId>,
    const_substitutions: SymbolMap<ConstGenericArg>,
}

impl From<TypedComptimeFrame> for ComptimeCallFrame {
    fn from(frame: TypedComptimeFrame) -> Self {
        Self {
            module_id: frame.module_id,
            function_id: frame.function_id,
            locals: HashMap::new(),
            local_types: frame.local_types,
            mutable_locals: HashSet::new(),
            type_substitutions: frame.type_substitutions,
            const_substitutions: frame.const_substitutions,
        }
    }
}

impl Analyzer<'_> {
    pub(crate) fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_or_unresolved(self.input.symbols, symbol)
    }

    fn new(input: ComptimeInput<'_>) -> Analyzer<'_> {
        Self::with_local_interner(input, input.interner.clone())
    }

    fn with_local_interner(input: ComptimeInput<'_>, local_interner: TyInterner) -> Analyzer<'_> {
        assert!(
            input.interner.is_prefix_of(&local_interner),
            "Nia ICE: comptime working interner is not an append-only extension of its input"
        );
        Analyzer {
            input,
            values: HashMap::new(),
            typed_values: HashMap::new(),
            external_typed_values: None,
            call_locals: Vec::new(),
            execution_module_overrides: Vec::new(),
            enum_values: HashMap::new(),
            typed_enum_values: HashMap::new(),
            array_lengths: HashMap::new(),
            diagnostics: Vec::new(),
            active: HashSet::new(),
            working_interners: HashMap::from([(input.defs.module_id, local_interner)]),
            program_type_normalizations: RefCell::new(HashMap::new()),
            program_value_type_normalizations: RefCell::new(HashMap::new()),
            program_trait_impls: RefCell::new(HashMap::new()),
            program_global_initializers: RefCell::new(HashMap::new()),
            resolved_call_type_substitutions: HashMap::new(),
        }
    }

    fn finish_local_interner(&mut self) -> TyInterner {
        self.working_interners
            .remove(&self.input.defs.module_id)
            .expect("Nia ICE: comptime analyzer lost its local working interner")
    }

    fn for_typed_query(input: TypedComptimeQueryInput<'_>) -> Analyzer<'_> {
        Analyzer {
            input: ComptimeInput {
                module: input.module,
                defs: input.defs,
                values: input.values,
                locals: input.locals,
                semantic_uses: input.semantic_uses,
                symbols: input.symbols,
                lowered: input.lowered,
                signatures: input.signatures,
                interner: input.interner,
                normalized: input.normalized,
                target: input.target,
                source_path: input.source_path,
                program: input.program,
            },
            values: HashMap::new(),
            typed_values: HashMap::new(),
            external_typed_values: Some(input.typed_values),
            call_locals: input
                .frames
                .iter()
                .cloned()
                .map(ComptimeCallFrame::from)
                .collect(),
            execution_module_overrides: Vec::new(),
            enum_values: HashMap::new(),
            typed_enum_values: HashMap::new(),
            array_lengths: input.array_lengths.clone(),
            diagnostics: Vec::new(),
            active: HashSet::new(),
            working_interners: HashMap::from([(input.defs.module_id, input.interner.clone())]),
            program_type_normalizations: RefCell::new(HashMap::new()),
            program_value_type_normalizations: RefCell::new(HashMap::new()),
            program_trait_impls: RefCell::new(HashMap::new()),
            program_global_initializers: RefCell::new(HashMap::new()),
            resolved_call_type_substitutions: HashMap::new(),
        }
    }

    fn analyze_array_lengths(&mut self) {
        let mut needed = HashSet::new();
        let mut seen = HashSet::new();
        let lowered_types = self
            .input
            .lowered
            .type_uses
            .values()
            .copied()
            .collect::<Vec<_>>();
        for ty in lowered_types {
            self.collect_array_len_const_exprs_in_ty_inner(ty, &mut needed, &mut seen);
        }
        for id in needed {
            self.eval_array_len_const_expr_id(id);
        }
    }

    fn analyze_enum_values(&mut self) {
        let enums = self.input.module.enums().to_vec();
        for item_enum in &enums {
            self.eval_enum(item_enum);
        }
    }

    fn analyze_functions(&mut self) {
        let functions = self.input.module.functions().clone();
        for (function_id, function) in functions {
            self.check_comptime_function_body(function_id, &function);
        }
    }

    fn analyze_values(&mut self) {
        let global_initializers = self
            .input
            .module
            .global_initializers()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for global_id in global_initializers {
            let span = self
                .input
                .defs
                .defs
                .get(global_id.def_id)
                .map(|def| def.span)
                .unwrap_or(Span::new(0, 0));
            let _ = self.eval_key(ComptimeKey::Global(global_id), span);
        }
        let local_initializers = self
            .input
            .module
            .local_initializers()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for local_id in local_initializers {
            let span = self
                .input
                .locals
                .locals
                .get(local_id)
                .map(|local| local.span)
                .unwrap_or(Span::new(0, 0));
            let _ = self.eval_key(ComptimeKey::Local(local_id), span);
        }
    }

    fn check_comptime_function_body(
        &mut self,
        function_id: GlobalDefId,
        function: &ResolvedComptimeFunction,
    ) {
        let mut frame = ComptimeCallFrame {
            module_id: Some(function_id.module_id),
            function_id: Some(function_id),
            ..ComptimeCallFrame::default()
        };
        for param in function.params() {
            if let Some(ty) = param.ty() {
                frame
                    .local_types
                    .insert(param.local_id(), ComptimeValueType::Runtime(ty));
            }
        }
        self.call_locals.push(frame);
        let _ = self.with_execution_module(function_id.module_id, |this| {
            this.check_resolved_comptime_block(function.body())
        });
        self.call_locals.pop();
    }

    fn eval_enum(&mut self, item_enum: &ResolvedComptimeEnum) {
        let enum_id = item_enum.def_id();
        let range = self.enum_backing_range(enum_id.def_id);
        let mut next_value = IntConst::from_i128(0);
        for variant in item_enum.variants() {
            let value = if let Some(expr) = variant.value() {
                match nia_comptime_engine::eval_resolved_comptime_int_expr(expr, self) {
                    Ok(value) => value,
                    Err(err) => {
                        self.push_engine_error(err);
                        next_value = enum_next_value(next_value);
                        continue;
                    }
                }
            } else {
                next_value
            };
            if let Some((min, max)) = range
                && !int_const_in_i128_range(value, min, max)
            {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::COMPTIME,
                    variant.span(),
                    format!("enum variant value {value:?} is out of range for backing type"),
                ));
            }
            let variant_id = variant.def_id();
            self.enum_values
                .insert(variant_id.def_id, ComptimeValue::Int(value));
            let ty = self
                .input
                .signatures
                .enums
                .get(&enum_id.def_id)
                .map(|signature| signature.backing_type)
                .unwrap_or_else(|| self.input.interner.primitive(PrimitiveTy::Isize));
            self.typed_enum_values.insert(
                variant_id.def_id,
                TypedComptimeValue {
                    value: ComptimeValue::Int(value),
                    ty: ComptimeValueType::Runtime(ty),
                },
            );
            next_value = enum_next_value(value);
        }
    }

    fn enum_backing_range(&self, enum_id: DefId) -> Option<(i128, i128)> {
        let signature = self.input.signatures.enums.get(&enum_id)?;
        let Some(TyKind::Primitive(primitive)) = self.input.interner.get(signature.backing_type)
        else {
            return None;
        };
        integer_range(*primitive)
    }

    fn eval_resolved_array_len_expr(&mut self, expr: &ResolvedComptimeExpr) -> Option<u64> {
        match nia_comptime_engine::eval_resolved_comptime_array_len_expr(expr, self) {
            Ok(value) => Some(value),
            Err(err) => {
                self.push_engine_error(err);
                None
            }
        }
    }

    fn eval_key(&mut self, key: ComptimeKey, span: Span) -> Option<ComptimeValue> {
        if let Some(value) = self.values.get(&key).cloned() {
            return Some(value);
        }
        if !self.active.insert(key) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::COMPTIME,
                span,
                "cyclic comptime dependency",
            ));
            return None;
        }
        let module_id = self.key_module_id(key);
        let result = self.initializer_for_key(key).and_then(|expr| {
            self.with_execution_module(module_id, |this| {
                let expected = this.explicit_type_for_key(key);
                let _ = this.resolved_comptime_expr_type(&expr, expected);
                match nia_comptime_engine::eval_resolved_comptime_expr(&expr, this) {
                    Ok(value) => Some(value),
                    Err(err) => {
                        this.push_engine_error(err);
                        None
                    }
                }
            })
        });
        self.active.remove(&key);
        result.map(|value| {
            let value = self.insert_typed_key_value(key, value);
            self.values.insert(key, value.clone());
            value
        })
    }

    fn insert_typed_key_value(&mut self, key: ComptimeKey, value: ComptimeValue) -> ComptimeValue {
        let module_id = self.key_module_id(key);
        self.with_execution_module(module_id, |this| {
            let Some(ty) = this.comptime_value_type_for_key(key) else {
                return value;
            };
            let value = this.normalize_typed_comptime_value(value, &ty);
            if let Some(span) = this.initializer_span_for_key(key) {
                this.validate_typed_value(span, &value, &ty);
            }
            this.typed_values.insert(
                key,
                TypedComptimeValue {
                    value: value.clone(),
                    ty,
                },
            );
            value
        })
    }

    fn typed_value_for_key(&self, key: ComptimeKey) -> Option<&TypedComptimeValue> {
        self.typed_values.get(&key).or_else(|| {
            self.external_typed_values
                .and_then(|values| values.get(&key))
        })
    }

    fn comptime_value_type_for_key(&mut self, key: ComptimeKey) -> Option<ComptimeValueType> {
        self.explicit_type_for_key(key)
            .map(ComptimeValueType::Runtime)
            .or_else(|| self.inferred_type_for_key(key))
    }

    fn inferred_type_for_key(&mut self, key: ComptimeKey) -> Option<ComptimeValueType> {
        let expr = self.initializer_for_key(key)?;
        let module_id = self.key_module_id(key);
        self.with_execution_module(module_id, |this| {
            this.resolved_comptime_expr_type(&expr, None)
        })
    }

    fn key_module_id(&self, key: ComptimeKey) -> ModuleId {
        match key {
            ComptimeKey::Global(global_id) => global_id.module_id,
            ComptimeKey::Local(_) => self.input.defs.module_id,
        }
    }

    fn initializer_span_for_key(&self, key: ComptimeKey) -> Option<Span> {
        self.initializer_for_key(key).map(|expr| expr.span())
    }

    fn validate_typed_value(&mut self, span: Span, value: &ComptimeValue, ty: &ComptimeValueType) {
        match ty {
            ComptimeValueType::Runtime(ty) => self.validate_runtime_typed_value(span, value, *ty),
            ComptimeValueType::Array { elem, .. } => {
                let ComptimeValue::Array(values) = value else {
                    self.push_comptime_type_mismatch(span, "array");
                    return;
                };
                if let ComptimeValueType::Array { len: Some(len), .. } = ty
                    && values.len() as u64 != *len
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::COMPTIME,
                        span,
                        format!(
                            "comptime array length {} does not match expected length {len}",
                            values.len()
                        ),
                    ));
                }
                for value in values {
                    self.validate_typed_value(span, value, elem);
                }
            }
            ComptimeValueType::Struct(fields) => {
                let ComptimeValue::Struct(values) = value else {
                    self.push_comptime_type_mismatch(span, "struct");
                    return;
                };
                let field_set: FieldSetCheck<SymbolId> = check_value_field_set(
                    values.keys().cloned(),
                    fields.iter().map(|field| field.name),
                );
                for field in fields {
                    if let Some(value) = values.get(&field.name) {
                        self.validate_typed_value(span, value, &field.ty);
                    }
                }
                for name in &field_set.missing_fields {
                    self.push_comptime_missing_struct_field(span, name);
                }
                for field in &field_set.unknown_fields {
                    self.push_comptime_extra_struct_field(span, &field.name);
                }
            }
            ComptimeValueType::Int => {
                if !matches!(value, ComptimeValue::Int(_)) {
                    self.push_comptime_type_mismatch(span, "int");
                }
            }
            ComptimeValueType::Bool => {
                if !matches!(value, ComptimeValue::Bool(_)) {
                    self.push_comptime_type_mismatch(span, "bool");
                }
            }
            ComptimeValueType::String => {
                if !matches!(value, ComptimeValue::String(_)) {
                    self.push_comptime_type_mismatch(span, "string");
                }
            }
        }
    }

    fn normalize_typed_comptime_value(
        &mut self,
        value: ComptimeValue,
        ty: &ComptimeValueType,
    ) -> ComptimeValue {
        match ty {
            ComptimeValueType::Runtime(ty) => {
                self.normalize_runtime_typed_comptime_value(value, *ty)
            }
            ComptimeValueType::Array { elem, .. } => match value {
                ComptimeValue::Array(values) => ComptimeValue::Array(
                    values
                        .into_iter()
                        .map(|value| self.normalize_typed_comptime_value(value, elem))
                        .collect(),
                ),
                value => value,
            },
            ComptimeValueType::Struct(fields) => match value {
                ComptimeValue::Struct(mut values) => {
                    for field in fields {
                        if let Some(value) = values.remove(&field.name) {
                            values.insert(
                                field.name,
                                self.normalize_typed_comptime_value(value, &field.ty),
                            );
                        }
                    }
                    ComptimeValue::Struct(values)
                }
                value => value,
            },
            ComptimeValueType::Int | ComptimeValueType::Bool | ComptimeValueType::String => value,
        }
    }

    fn normalize_runtime_typed_comptime_value(
        &mut self,
        value: ComptimeValue,
        ty: InternedTyId,
    ) -> ComptimeValue {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. }) => {
                if self.runtime_array_accepts_comptime_string(&value, ty)
                    && let ComptimeValue::String(value) = value
                {
                    return ComptimeValue::Array(comptime_string_to_char_array(&value));
                }
                match value {
                    ComptimeValue::Array(values) => ComptimeValue::Array(
                        values
                            .into_iter()
                            .map(|value| self.normalize_runtime_typed_comptime_value(value, elem))
                            .collect(),
                    ),
                    value => value,
                }
            }
            Some(TyKind::Pointer { elem, .. }) => match value {
                ComptimeValue::Pointer(value) => ComptimeValue::Pointer(Box::new(
                    self.normalize_runtime_typed_comptime_value(*value, elem),
                )),
                value => value,
            },
            Some(TyKind::Optional { elem }) => match value {
                ComptimeValue::Optional(Some(value)) => ComptimeValue::Optional(Some(Box::new(
                    self.normalize_runtime_typed_comptime_value(*value, elem),
                ))),
                value => value,
            },
            Some(TyKind::ErrorUnion { error, value: ok }) => match value {
                ComptimeValue::ErrorUnion(Ok(value)) => ComptimeValue::ErrorUnion(Ok(Box::new(
                    self.normalize_runtime_typed_comptime_value(*value, ok),
                ))),
                ComptimeValue::ErrorUnion(Err(value)) => ComptimeValue::ErrorUnion(Err(Box::new(
                    self.normalize_runtime_typed_comptime_value(*value, error),
                ))),
                value => value,
            },
            Some(TyKind::Nominal { .. }) => self.normalize_nominal_struct_value(value, ty),
            _ => value,
        }
    }

    fn normalize_nominal_struct_value(
        &mut self,
        value: ComptimeValue,
        ty: InternedTyId,
    ) -> ComptimeValue {
        let ComptimeValue::Struct(mut values) = value else {
            return value;
        };
        let Some((def_id, args)) = self.expected_nominal_parts(ty) else {
            return ComptimeValue::Struct(values);
        };
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return ComptimeValue::Struct(values);
        }
        let Some(signature) = self.struct_signature_for(def_id) else {
            return ComptimeValue::Struct(values);
        };
        let Some(field_tys) = self.comptime_struct_field_types(&signature, &args) else {
            return ComptimeValue::Struct(values);
        };
        for (name, ty) in field_tys {
            if let Some(value) = values.remove(&name) {
                values.insert(name, self.normalize_runtime_typed_comptime_value(value, ty));
            }
        }
        ComptimeValue::Struct(values)
    }

    fn validate_runtime_typed_value(
        &mut self,
        span: Span,
        value: &ComptimeValue,
        ty: InternedTyId,
    ) {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => {
                self.validate_primitive_typed_value(span, value, primitive)
            }
            Some(TyKind::Array { elem, .. }) => {
                if self.runtime_array_accepts_comptime_string(value, ty) {
                    return;
                }
                if let ComptimeValue::String(value) = value
                    && self.runtime_array_is_char_array(ty)
                    && let Some(expected_len) = self.runtime_array_len(ty)
                {
                    self.diagnostics.push(Diagnostic::user_error_at(codes::COMPTIME,
                        span,
                        format!(
                            "comptime array length {} does not match expected length {expected_len}",
                            value.chars().count()
                        ),
                    ));
                    return;
                }
                let ComptimeValue::Array(values) = value else {
                    self.push_comptime_type_mismatch(span, "array");
                    return;
                };
                if let Some(expected_len) = self.runtime_array_len(ty)
                    && values.len() as u64 != expected_len
                {
                    self.diagnostics.push(Diagnostic::user_error_at(codes::COMPTIME,
                        span,
                        format!(
                            "comptime array length {} does not match expected length {expected_len}",
                            values.len()
                        ),
                    ));
                }
                for value in values {
                    self.validate_runtime_typed_value(span, value, elem);
                }
            }
            Some(TyKind::Pointer { elem, .. }) => {
                let ComptimeValue::Pointer(value) = value else {
                    self.push_comptime_type_mismatch(span, "pointer");
                    return;
                };
                self.validate_runtime_typed_value(span, value, elem);
            }
            Some(TyKind::Optional { elem }) => match value {
                ComptimeValue::Optional(Some(value)) => {
                    self.validate_runtime_typed_value(span, value, elem);
                }
                ComptimeValue::Optional(None) => {}
                _ => self.push_comptime_type_mismatch(span, "optional"),
            },
            Some(TyKind::ErrorUnion { error, value: ok }) => {
                let ComptimeValue::ErrorUnion(value) = value else {
                    self.push_comptime_type_mismatch(span, "error union");
                    return;
                };
                match value {
                    Ok(value) => self.validate_runtime_typed_value(span, value, ok),
                    Err(value) => self.validate_runtime_typed_value(span, value, error),
                }
            }
            Some(TyKind::Nominal { .. }) => {
                self.validate_nominal_struct_value(span, value, ty);
            }
            _ => {}
        }
    }

    fn validate_nominal_struct_value(
        &mut self,
        span: Span,
        value: &ComptimeValue,
        ty: InternedTyId,
    ) {
        let ComptimeValue::Struct(values) = value else {
            self.push_comptime_type_mismatch(span, "struct");
            return;
        };
        let Some((def_id, args)) = self.expected_nominal_parts(ty) else {
            return;
        };
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return;
        }
        let Some(signature) = self.struct_signature_for(def_id) else {
            return;
        };
        let Some(field_tys) = self.comptime_struct_field_types(&signature, &args) else {
            return;
        };
        let field_set: FieldSetCheck<SymbolId> =
            check_value_field_set(values.keys().cloned(), field_tys.keys().cloned());
        for (name, field_ty) in &field_tys {
            if let Some(value) = values.get(name) {
                self.validate_runtime_typed_value(span, value, *field_ty);
            }
        }
        for name in &field_set.missing_fields {
            self.push_comptime_missing_struct_field(span, name);
        }
        for field in &field_set.unknown_fields {
            self.push_comptime_extra_struct_field(span, &field.name);
        }
    }

    fn validate_primitive_typed_value(
        &mut self,
        span: Span,
        value: &ComptimeValue,
        primitive: PrimitiveTy,
    ) {
        match (value, primitive) {
            (ComptimeValue::Int(value), primitive) => {
                let Some((min, max)) =
                    primitive_integer_range_for_target(primitive, self.input.target.pointer_width)
                else {
                    self.push_comptime_primitive_mismatch(span, primitive);
                    return;
                };
                if !int_const_in_i128_range(*value, min, max) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::COMPTIME,
                        span,
                        format!(
                            "comptime integer value {value:?} is out of range for {}",
                            primitive.name()
                        ),
                    ));
                }
            }
            (ComptimeValue::Bool(_), PrimitiveTy::Bool) => {}
            (ComptimeValue::Float(value), PrimitiveTy::F32) => {
                let value = *value as f32;
                if !value.is_finite() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::COMPTIME,
                        span,
                        "comptime float value is out of range for f32",
                    ));
                }
            }
            (ComptimeValue::Float(value), PrimitiveTy::F64) => {
                if !value.is_finite() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::COMPTIME,
                        span,
                        "comptime float value is out of range for f64",
                    ));
                }
            }
            (_, primitive) => {
                self.push_comptime_primitive_mismatch(span, primitive);
            }
        }
    }

    fn runtime_array_len(&mut self, ty: InternedTyId) -> Option<u64> {
        let Some(TyKind::Array { len, .. }) = self.ty_kind(ty) else {
            return None;
        };
        self.array_len_const_value(len)
    }

    fn runtime_array_accepts_comptime_string(
        &mut self,
        value: &ComptimeValue,
        ty: InternedTyId,
    ) -> bool {
        let ComptimeValue::String(value) = value else {
            return false;
        };
        if !self.runtime_array_is_char_array(ty) {
            return false;
        }
        self.runtime_array_len(ty)
            .is_none_or(|len| value.chars().count() as u64 == len)
    }

    fn runtime_array_is_char_array(&self, ty: InternedTyId) -> bool {
        let Some(TyKind::Array { elem, .. }) = self.ty_kind(ty) else {
            return false;
        };
        matches!(
            self.ty_kind(elem),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn push_comptime_type_mismatch(&mut self, span: Span, expected: &str) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::COMPTIME,
            span,
            format!("comptime value does not match expected {expected} type"),
        ));
    }

    fn push_comptime_missing_struct_field(&mut self, span: Span, name: &SymbolId) {
        let name = self.symbol_name(*name);
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::COMPTIME,
            span,
            format!("comptime struct value is missing field `{name}`"),
        ));
    }

    fn push_comptime_extra_struct_field(&mut self, span: Span, name: &SymbolId) {
        let name = self.symbol_name(*name);
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::COMPTIME,
            span,
            format!("comptime struct value has extra field `{name}`"),
        ));
    }

    fn push_comptime_primitive_mismatch(&mut self, span: Span, primitive: PrimitiveTy) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::COMPTIME,
            span,
            format!(
                "comptime value does not match primitive type {}",
                primitive.name()
            ),
        ));
    }
}
