// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    ConstArmType, ConstArrayLengths, ConstCheck, ConstEnumValues, ConstInput, ConstKey,
    ConstProgramContext, ConstTypedFacts, ConstValue, ConstValueType, ConstValues, TypedConstValue,
    resolved_pattern_local_id,
    support::{
        const_string_to_char_array, enum_next_value, float_literal_suffix_ty,
        int_const_in_i128_range, integer_literal_suffix_ty, integer_range, is_float_primitive,
        primitive_integer_layout, primitive_integer_range_for_target,
    },
};
use nia_const_eval::{
    ConstAbiType, ConstAllocationId, ConstEndianness, ConstError, ConstPointerValue,
};
use nia_const_ir::{
    ConstAssignOp, ConstBinaryOp, ConstEnumPatternFields, ConstNameResolution,
    ConstNamedPatternField, ConstStringLiteral, ConstUnaryOp, ResolvedConstArrayElements,
    ResolvedConstArrayElementsKind, ResolvedConstAssign, ResolvedConstAssignPathElemKind,
    ResolvedConstAssignTargetKind, ResolvedConstAssociatedTarget, ResolvedConstBlock,
    ResolvedConstEnum, ResolvedConstExpr, ResolvedConstExprKind, ResolvedConstFieldInit,
    ResolvedConstGenericArg, ResolvedConstMatch, ResolvedConstMatchArmBody,
    ResolvedConstMatchArmBodyKind, ResolvedConstModule, ResolvedConstPattern,
    ResolvedConstPatternBinding, ResolvedConstPatternKind, ResolvedConstStmtKind,
};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{
    BuiltinFunction, GlobalConstExprId, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId,
    ModuleId, ValueBuiltin,
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
use nia_sema_ir::{AssociatedConstProjection, BuiltinAssociatedValue, SemanticUseTable};
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, symbol_text_or_unresolved};
use nia_target_config::TargetConfig;
use nia_trait_solve::{TraitGoal, TraitResolution, TraitSolverContext};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, ConstGenericValue, IntConst, PrimitiveTy, RangeTyKind, TraitId,
    TyKind, TypeStoreAppend,
};
use nia_value_resolve::ValueResolution;

mod aggregate_literals;
mod array_literals;
mod context;
mod env_impl;
mod expr_types;
mod generics;
mod indexing;
mod match_patterns;
mod traits;
mod ty_substitution;
mod type_infer;

#[derive(Debug, Clone, Default)]
/// Type context inherited by a standalone const-expression query.
///
/// Frames are ordered outermost to innermost. A frame with `module_id` marks a
/// function boundary; locals and substitutions below the nearest such boundary
/// are caller state and are not visible while typing the callee.
pub struct TypedConstFrame {
    /// Module boundary for this frame; `None` denotes a lexical scope.
    pub module_id: Option<ModuleId>,
    /// Function identity at an execution boundary.
    pub function_id: Option<GlobalDefId>,
    /// Locals visible in this frame.
    pub local_types: HashMap<LocalId, ConstValueType>,
    /// Type substitutions active in this frame.
    pub type_substitutions: SymbolMap<InternedTyId>,
    /// Const-generic substitutions active in this frame.
    pub const_substitutions: SymbolMap<ConstGenericArg>,
}

#[derive(Debug)]
/// All semantic inputs needed to type one already-resolved const expression.
///
/// This query-shaped input avoids rerunning whole-module const analysis for
/// callers such as body checking and generic call instantiation.
pub struct TypedConstQueryInput<'a> {
    /// Resolved const IR for the module being queried.
    pub module: &'a ResolvedConstModule,
    /// Definition identities used by name and item lookup.
    pub defs: &'a DefCollection,
    /// Value-resolution identities for globals and associated values.
    pub values: &'a ValueResolution,
    /// Resolved local identities used by bindings and assignments.
    pub locals: &'a LocalResolution,
    /// Semantic use information needed for resolved references.
    pub semantic_uses: &'a SemanticUseTable,
    /// Symbol table used to render stable diagnostic names.
    pub symbols: &'a nia_symbol_table::SymbolTable,
    /// Lowered type information for runtime representation checks.
    pub lowered: &'a nia_type_lower::TypeLowering,
    /// Function and item signatures used during generic inference.
    pub signatures: &'a ItemSignatures,
    /// Interned type store used by the analyzer.
    pub type_store: &'a nia_ty::TypeStore,
    /// Normalized type relations used by assignability and trait queries.
    pub normalization: &'a nia_type_normalize::TypeNormalization,
    /// Target layout and primitive-width configuration.
    pub target: &'a TargetConfig,
    /// Source path attached to diagnostics from this module.
    pub source_path: &'a SourcePath,
    /// Optional cross-module providers used by const evaluation.
    pub program: ConstProgramContext<'a>,
    /// Values already computed for the current module.
    pub typed_values: &'a HashMap<ConstKey, TypedConstValue>,
    /// Array lengths computed by the prerequisite phase.
    pub array_lengths: &'a HashMap<GlobalConstExprId, u64>,
    /// Active lexical and function frames for standalone expression queries.
    pub frames: &'a [TypedConstFrame],
}

#[derive(Debug, Clone, Default)]
/// Concrete type and value arguments inferred for a const function call.
pub struct ConstGenericInstantiation {
    /// Inferred type-parameter substitutions.
    pub type_substitutions: SymbolMap<InternedTyId>,
    /// Inferred const-parameter substitutions.
    pub const_substitutions: SymbolMap<ConstGenericArg>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedConstCallee {
    pub function_id: GlobalDefId,
    pub receiver: Option<ResolvedConstExpr>,
    pub target_instantiation: ConstGenericInstantiation,
}

pub(crate) enum ResolvedConstCalleeSelection {
    NoMatch,
    Unique(ResolvedConstCallee),
    Ambiguous,
}

pub(crate) struct ConstFunctionInstantiationInput<'a> {
    pub signature_module_id: ModuleId,
    pub signature: &'a FunctionSignature,
    pub generic_args: &'a [ResolvedConstGenericArg],
    pub arg_exprs: &'a [ResolvedConstExpr],
    pub expected_return: Option<InternedTyId>,
    pub initial: ConstGenericInstantiation,
}

/// Infers omitted generic arguments for a resolved const function call.
///
/// Explicit arguments are validated against the signature. With no explicit
/// arguments, inference combines the expected return type with every available
/// argument type and rejects missing or conflicting substitutions.
pub fn instantiate_resolved_const_function_generics(
    input: TypedConstQueryInput<'_>,
    span: Span,
    signature_module_id: ModuleId,
    signature: &FunctionSignature,
    generic_args: &[ResolvedConstGenericArg],
    arg_exprs: &[ResolvedConstExpr],
    expected_return: Option<InternedTyId>,
) -> Result<ConstGenericInstantiation, ConstError> {
    let mut analyzer = Analyzer::for_typed_query(input);
    analyzer.instantiate_resolved_function_generics(
        span,
        ConstFunctionInstantiationInput {
            signature_module_id,
            signature,
            generic_args,
            arg_exprs,
            expected_return,
            initial: ConstGenericInstantiation::default(),
        },
    )
}

/// Infers the semantic value type of one resolved const expression.
///
/// Literal-only values retain a [`ConstValueType`] such as `Int` until an
/// expected runtime type provides the representation required by execution.
pub fn infer_resolved_const_expr_type(
    input: TypedConstQueryInput<'_>,
    expr: &ResolvedConstExpr,
    expected: Option<InternedTyId>,
) -> Option<ConstValueType> {
    let mut analyzer = Analyzer::for_typed_query(input);
    analyzer.resolved_const_expr_type(expr, expected)
}

/// Runs every const-analysis phase for a module in dependency order.
///
/// The pipeline is intentionally monotonic: array lengths feed enum
/// discriminants, both feed initializer values, and those values feed runtime
/// type facts. Each phase receives the prior phase's diagnostics and cached
/// maps, so a caller can reuse any completed prefix without changing results.
pub fn check_module_const(input: ConstInput<'_>) -> ConstCheck {
    let array_lengths = compute_module_const_array_lengths(input);
    let enum_values = compute_module_const_enum_values(input, array_lengths.clone());
    let values = compute_module_const_values(input, array_lengths.clone(), enum_values.clone());
    let typed_facts = compute_module_const_typed_facts(
        input,
        array_lengths.clone(),
        enum_values.clone(),
        values.clone(),
    );
    check_module_const_with_all_phases(array_lengths, enum_values, values, typed_facts)
}

/// Evaluates array-length expressions needed by the module's lowered types.
pub fn compute_module_const_array_lengths(input: ConstInput<'_>) -> ConstArrayLengths {
    let mut analyzer = Analyzer::new(input);
    analyzer.analyze_array_lengths();
    ConstArrayLengths {
        values: Arc::new(analyzer.array_lengths),
        diagnostics: analyzer.diagnostics,
    }
}

/// Completes module const checking from a previously cached array-length phase.
pub fn check_module_const_with_array_lengths(
    input: ConstInput<'_>,
    array_lengths: ConstArrayLengths,
) -> ConstCheck {
    let enum_values = compute_module_const_enum_values(input, array_lengths.clone());
    check_module_const_with_phases(input, array_lengths, enum_values)
}

/// Evaluates enum discriminants after array lengths are available.
pub fn compute_module_const_enum_values(
    input: ConstInput<'_>,
    array_lengths: ConstArrayLengths,
) -> ConstEnumValues {
    let mut analyzer = Analyzer::new(input);
    analyzer.array_lengths = Arc::unwrap_or_clone(array_lengths.values);
    analyzer.diagnostics = array_lengths.diagnostics;
    analyzer.analyze_enum_values();
    ConstEnumValues {
        values: Arc::new(analyzer.enum_values),
        typed_values: Arc::new(analyzer.typed_enum_values),
        diagnostics: analyzer.diagnostics,
    }
}

/// Completes module const checking from cached array-length and enum phases.
pub fn check_module_const_with_phases(
    input: ConstInput<'_>,
    array_lengths: ConstArrayLengths,
    enum_values: ConstEnumValues,
) -> ConstCheck {
    let values = compute_module_const_values(input, array_lengths.clone(), enum_values.clone());
    let typed_facts = compute_module_const_typed_facts(
        input,
        array_lengths.clone(),
        enum_values.clone(),
        values.clone(),
    );
    check_module_const_with_all_phases(array_lengths, enum_values, values, typed_facts)
}

/// Evaluates global and local const initializers after prerequisite phases.
pub fn compute_module_const_values(
    input: ConstInput<'_>,
    array_lengths: ConstArrayLengths,
    enum_values: ConstEnumValues,
) -> ConstValues {
    let mut analyzer = Analyzer::new(input);
    analyzer.array_lengths = Arc::unwrap_or_clone(array_lengths.values);
    analyzer.enum_values = Arc::unwrap_or_clone(enum_values.values);
    analyzer.typed_enum_values = Arc::unwrap_or_clone(enum_values.typed_values);
    analyzer.diagnostics = enum_values.diagnostics;
    analyzer.analyze_values();
    ConstValues {
        values: Arc::new(analyzer.values),
        typed_values: Arc::new(analyzer.typed_values),
        diagnostics: analyzer.diagnostics,
    }
}

/// Assembles independently cached phase outputs into the public result.
pub fn check_module_const_with_all_phases(
    array_lengths: ConstArrayLengths,
    enum_values: ConstEnumValues,
    values: ConstValues,
    typed_facts: ConstTypedFacts,
) -> ConstCheck {
    ConstCheck {
        values: values.values,
        typed_values: typed_facts.typed_values,
        enum_values: enum_values.values,
        typed_enum_values: enum_values.typed_values,
        array_lengths: array_lengths.values,
        diagnostics: typed_facts.diagnostics,
    }
}

/// Computes runtime type facts for values produced by const evaluation.
pub fn compute_module_const_typed_facts(
    input: ConstInput<'_>,
    array_lengths: ConstArrayLengths,
    enum_values: ConstEnumValues,
    values: ConstValues,
) -> ConstTypedFacts {
    let mut analyzer = Analyzer::new(input);
    analyzer.array_lengths = Arc::unwrap_or_clone(array_lengths.values);
    analyzer.enum_values = Arc::unwrap_or_clone(enum_values.values);
    analyzer.typed_enum_values = Arc::unwrap_or_clone(enum_values.typed_values);
    analyzer.values = Arc::unwrap_or_clone(values.values);
    analyzer.typed_values = Arc::unwrap_or_clone(values.typed_values);
    analyzer.diagnostics = values.diagnostics;
    ConstTypedFacts {
        typed_values: Arc::new(analyzer.typed_values),
        diagnostics: analyzer.diagnostics,
    }
}

struct ConstTypeCx<'a> {
    store: &'a nia_ty::TypeStore,
    append: TypeStoreAppend,
}

impl<'a> ConstTypeCx<'a> {
    fn new(store: &'a nia_ty::TypeStore, module_id: ModuleId) -> Self {
        Self {
            store,
            append: store.append_for_module(module_id),
        }
    }

    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.store.get(ty)
    }

    fn intern(&self, kind: TyKind) -> InternedTyId {
        self.append.intern(kind)
    }
}

pub(crate) struct Analyzer<'a> {
    input: ConstInput<'a>,
    values: HashMap<ConstKey, ConstValue>,
    typed_values: HashMap<ConstKey, TypedConstValue>,
    external_typed_values: Option<&'a HashMap<ConstKey, TypedConstValue>>,
    call_locals: Vec<ConstCallFrame>,
    execution_module_overrides: Vec<ModuleId>,
    enum_values: HashMap<DefId, ConstValue>,
    typed_enum_values: HashMap<DefId, TypedConstValue>,
    array_lengths: HashMap<GlobalConstExprId, u64>,
    diagnostics: Vec<Diagnostic>,
    active: HashSet<ConstKey>,
    type_contexts: HashMap<ModuleId, ConstTypeCx<'a>>,
    program_type_normalizations:
        RefCell<HashMap<ModuleId, Arc<nia_type_normalize::TypeNormalization>>>,
    program_trait_impls: RefCell<HashMap<ModuleId, Vec<ProgramTraitImplSignature>>>,
    program_global_initializers:
        RefCell<HashMap<GlobalDefId, Option<nia_const_ir::ResolvedConstExpr>>>,
    resolved_call_instantiations: HashMap<Span, ConstGenericInstantiation>,
    // Each active const root or function instance owns its transient expression facts.
    resolved_expr_types: Vec<HashMap<Span, InternedTyId>>,
    const_eval_budget: nia_const_eval::ConstEvalBudget,
    next_const_allocation_serial: u64,
}

#[derive(Debug, Clone, Default)]
/// One lexical or function frame used while executing and typing const IR.
///
/// `module_id` is present only on function boundaries. Frames above that
/// boundary are lexical scopes in the same invocation; frames below it belong
/// to callers and must not participate in local or substitution lookup.
pub(crate) struct ConstCallFrame {
    is_execution_frame: bool,
    module_id: Option<ModuleId>,
    function_id: Option<GlobalDefId>,
    return_type: Option<InternedTyId>,
    locals: HashMap<LocalId, ConstValue>,
    allocation_ids: HashMap<LocalId, ConstAllocationId>,
    temporary_allocations: HashMap<ConstAllocationId, ConstValue>,
    local_types: HashMap<LocalId, ConstValueType>,
    mutable_locals: HashSet<LocalId>,
    type_substitutions: SymbolMap<InternedTyId>,
    const_substitutions: SymbolMap<ConstGenericArg>,
    try_error_conversions: HashMap<Span, ResolvedConstCallee>,
    checked_try_error_conversions: HashSet<Span>,
}

impl From<TypedConstFrame> for ConstCallFrame {
    fn from(frame: TypedConstFrame) -> Self {
        Self {
            is_execution_frame: false,
            module_id: frame.module_id,
            function_id: frame.function_id,
            return_type: None,
            locals: HashMap::new(),
            allocation_ids: HashMap::new(),
            temporary_allocations: HashMap::new(),
            local_types: frame.local_types,
            mutable_locals: HashSet::new(),
            type_substitutions: frame.type_substitutions,
            const_substitutions: frame.const_substitutions,
            try_error_conversions: HashMap::new(),
            checked_try_error_conversions: HashSet::new(),
        }
    }
}

impl Analyzer<'_> {
    pub(crate) fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_or_unresolved(self.input.symbols, symbol)
    }

    fn new<'a>(input: ConstInput<'a>) -> Analyzer<'a> {
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
            type_contexts: HashMap::from([(
                input.defs.module_id,
                ConstTypeCx::new(input.type_store, input.defs.module_id),
            )]),
            program_type_normalizations: RefCell::new(HashMap::new()),
            program_trait_impls: RefCell::new(HashMap::new()),
            program_global_initializers: RefCell::new(HashMap::new()),
            resolved_call_instantiations: HashMap::new(),
            resolved_expr_types: Vec::new(),
            const_eval_budget: nia_const_eval::ConstEvalBudget::default(),
            next_const_allocation_serial: 0,
        }
    }

    fn for_typed_query(input: TypedConstQueryInput<'_>) -> Analyzer<'_> {
        Analyzer {
            input: ConstInput {
                module: input.module,
                defs: input.defs,
                values: input.values,
                locals: input.locals,
                semantic_uses: input.semantic_uses,
                symbols: input.symbols,
                lowered: input.lowered,
                signatures: input.signatures,
                type_store: input.type_store,
                normalization: input.normalization,
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
                .map(ConstCallFrame::from)
                .collect(),
            execution_module_overrides: Vec::new(),
            enum_values: HashMap::new(),
            typed_enum_values: HashMap::new(),
            array_lengths: input.array_lengths.clone(),
            diagnostics: Vec::new(),
            active: HashSet::new(),
            type_contexts: HashMap::from([(
                input.defs.module_id,
                ConstTypeCx::new(input.type_store, input.defs.module_id),
            )]),
            program_type_normalizations: RefCell::new(HashMap::new()),
            program_trait_impls: RefCell::new(HashMap::new()),
            program_global_initializers: RefCell::new(HashMap::new()),
            resolved_call_instantiations: HashMap::new(),
            resolved_expr_types: Vec::new(),
            const_eval_budget: nia_const_eval::ConstEvalBudget::default(),
            next_const_allocation_serial: 0,
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
            let _ = self.eval_key(ConstKey::Global(global_id), span);
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
            let _ = self.eval_key(ConstKey::Local(local_id), span);
        }
    }

    fn eval_enum(&mut self, item_enum: &ResolvedConstEnum) {
        let enum_id = item_enum.def_id();
        let range = self.enum_backing_range(enum_id.def_id);
        let mut next_value = IntConst::from_i128(0);
        for variant in item_enum.variants() {
            let value = if let Some(expr) = variant.value() {
                match nia_const_eval::eval_resolved_const_int_expr(expr, self) {
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
                    codes::CONST,
                    variant.span(),
                    format!("enum variant value {value:?} is out of range for backing type"),
                ));
            }
            let variant_id = variant.def_id();
            self.enum_values
                .insert(variant_id.def_id, ConstValue::Int(value));
            let ty = self
                .input
                .signatures
                .enums
                .get(&enum_id.def_id)
                .map(|signature| signature.backing_type)
                .unwrap_or_else(|| {
                    self.input
                        .type_store
                        .append_for_module(self.input.defs.module_id)
                        .intern(TyKind::Primitive(PrimitiveTy::Isize))
                });
            self.typed_enum_values.insert(
                variant_id.def_id,
                TypedConstValue {
                    value: ConstValue::Int(value),
                    ty: ConstValueType::Runtime(ty),
                },
            );
            next_value = enum_next_value(value);
        }
    }

    fn enum_backing_range(&self, enum_id: DefId) -> Option<(i128, i128)> {
        let signature = self.input.signatures.enums.get(&enum_id)?;
        let Some(TyKind::Primitive(primitive)) = self.input.type_store.get(signature.backing_type)
        else {
            return None;
        };
        integer_range(*primitive)
    }

    fn eval_resolved_array_len_expr(&mut self, expr: &ResolvedConstExpr) -> Option<u64> {
        let usize_ty = self.current_runtime_primitive_type(PrimitiveTy::Usize);
        self.resolved_expr_types.push(HashMap::new());
        let _ = self.resolved_const_expr_type(expr, Some(usize_ty));
        let result = nia_const_eval::eval_resolved_const_array_len_expr(expr, self);
        self.resolved_expr_types.pop();
        match result {
            Ok(value) => Some(value),
            Err(err) => {
                self.push_engine_error(err);
                None
            }
        }
    }

    fn eval_key(&mut self, key: ConstKey, span: Span) -> Option<ConstValue> {
        if let Some(value) = self.values.get(&key).cloned() {
            return Some(value);
        }
        if !self.active.insert(key) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::CONST,
                span,
                "cyclic const dependency",
            ));
            return None;
        }
        let module_id = self.key_module_id(key);
        let result = self.initializer_for_key(key).and_then(|expr| {
            self.with_execution_module(module_id, |this| {
                let expected = this.explicit_type_for_key(key);
                this.resolved_expr_types.push(HashMap::new());
                let _ = this.resolved_const_expr_type(&expr, expected);
                let result = nia_const_eval::eval_resolved_const_expr(&expr, this);
                this.resolved_expr_types.pop();
                match result {
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

    fn insert_typed_key_value(&mut self, key: ConstKey, value: ConstValue) -> ConstValue {
        let module_id = self.key_module_id(key);
        self.with_execution_module(module_id, |this| {
            let Some(ty) = this.const_value_type_for_key(key) else {
                return value;
            };
            let value = this.normalize_typed_const_value(value, &ty);
            if let Some(span) = this.initializer_span_for_key(key) {
                this.validate_typed_value(span, &value, &ty);
            }
            this.typed_values.insert(
                key,
                TypedConstValue {
                    value: value.clone(),
                    ty,
                },
            );
            value
        })
    }

    fn typed_value_for_key(&self, key: ConstKey) -> Option<&TypedConstValue> {
        self.typed_values.get(&key).or_else(|| {
            self.external_typed_values
                .and_then(|values| values.get(&key))
        })
    }

    fn const_value_type_for_key(&mut self, key: ConstKey) -> Option<ConstValueType> {
        self.explicit_type_for_key(key)
            .map(ConstValueType::Runtime)
            .or_else(|| self.inferred_type_for_key(key))
    }

    fn inferred_type_for_key(&mut self, key: ConstKey) -> Option<ConstValueType> {
        let expr = self.initializer_for_key(key)?;
        let module_id = self.key_module_id(key);
        self.with_execution_module(module_id, |this| this.resolved_const_expr_type(&expr, None))
    }

    fn key_module_id(&self, key: ConstKey) -> ModuleId {
        match key {
            ConstKey::Global(global_id) => global_id.module_id,
            ConstKey::Local(_) => self.input.defs.module_id,
        }
    }

    fn initializer_span_for_key(&self, key: ConstKey) -> Option<Span> {
        self.initializer_for_key(key).map(|expr| expr.span())
    }

    fn validate_typed_value(&mut self, span: Span, value: &ConstValue, ty: &ConstValueType) {
        match ty {
            ConstValueType::Runtime(ty) => self.validate_runtime_typed_value(span, value, *ty),
            ConstValueType::Array { elem, .. } => {
                let ConstValue::Array(values) = value else {
                    self.push_const_type_mismatch(span, "array");
                    return;
                };
                if let ConstValueType::Array { len: Some(len), .. } = ty
                    && u64::try_from(values.len()) != Ok(*len)
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        span,
                        format!(
                            "const array length {} does not match expected length {len}",
                            values.len()
                        ),
                    ));
                }
                for value in values {
                    self.validate_typed_value(span, value, elem);
                }
            }
            ConstValueType::Int => {
                if !matches!(value, ConstValue::Int(_)) {
                    self.push_const_type_mismatch(span, "int");
                }
            }
            ConstValueType::Bool => {
                if !matches!(value, ConstValue::Bool(_)) {
                    self.push_const_type_mismatch(span, "bool");
                }
            }
            ConstValueType::String => {
                if !matches!(value, ConstValue::String(_)) {
                    self.push_const_type_mismatch(span, "string");
                }
            }
        }
    }

    fn normalize_typed_const_value(
        &mut self,
        value: ConstValue,
        ty: &ConstValueType,
    ) -> ConstValue {
        match ty {
            ConstValueType::Runtime(ty) => self.normalize_runtime_typed_const_value(value, *ty),
            ConstValueType::Array { elem, .. } => match value {
                ConstValue::Array(values) => ConstValue::Array(
                    values
                        .into_iter()
                        .map(|value| self.normalize_typed_const_value(value, elem))
                        .collect(),
                ),
                value => value,
            },
            ConstValueType::Int | ConstValueType::Bool | ConstValueType::String => value,
        }
    }

    fn normalize_runtime_typed_const_value(
        &mut self,
        value: ConstValue,
        ty: InternedTyId,
    ) -> ConstValue {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. }) => {
                if self.runtime_array_accepts_const_string(&value, ty)
                    && let ConstValue::String(value) = value
                {
                    return ConstValue::Array(const_string_to_char_array(&value));
                }
                match value {
                    ConstValue::Array(values) => ConstValue::Array(
                        values
                            .into_iter()
                            .map(|value| self.normalize_runtime_typed_const_value(value, elem))
                            .collect(),
                    ),
                    value => value,
                }
            }
            Some(TyKind::Pointer { elem, .. }) => match value {
                ConstValue::Pointer(ConstPointerValue::Frozen {
                    origin,
                    is_readonly,
                    pointee,
                }) => ConstValue::Pointer(ConstPointerValue::Frozen {
                    origin,
                    is_readonly,
                    pointee: Box::new(self.normalize_runtime_typed_const_value(*pointee, elem)),
                }),
                value => value,
            },
            Some(TyKind::Optional { elem }) => match value {
                ConstValue::Optional(Some(value)) => ConstValue::Optional(Some(Box::new(
                    self.normalize_runtime_typed_const_value(*value, elem),
                ))),
                value => value,
            },
            Some(TyKind::ErrorUnion { error, value: ok }) => match value {
                ConstValue::ErrorUnion(Ok(value)) => ConstValue::ErrorUnion(Ok(Box::new(
                    self.normalize_runtime_typed_const_value(*value, ok),
                ))),
                ConstValue::ErrorUnion(Err(value)) => ConstValue::ErrorUnion(Err(Box::new(
                    self.normalize_runtime_typed_const_value(*value, error),
                ))),
                value => value,
            },
            Some(TyKind::Nominal { .. }) => self.normalize_nominal_struct_value(value, ty),
            _ => value,
        }
    }

    fn normalize_nominal_struct_value(
        &mut self,
        value: ConstValue,
        ty: InternedTyId,
    ) -> ConstValue {
        let ConstValue::Struct(mut values) = value else {
            return value;
        };
        let Some((def_id, args, const_args)) = self.expected_nominal_parts(ty) else {
            return ConstValue::Struct(values);
        };
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return ConstValue::Struct(values);
        }
        let Some(signature) = self.struct_signature_for(def_id) else {
            return ConstValue::Struct(values);
        };
        let Some(field_tys) = self.const_struct_field_types(&signature, &args, &const_args) else {
            return ConstValue::Struct(values);
        };
        for (name, ty) in field_tys {
            if let Some(value) = values.remove(&name) {
                values.insert(name, self.normalize_runtime_typed_const_value(value, ty));
            }
        }
        ConstValue::Struct(values)
    }

    fn validate_runtime_typed_value(&mut self, span: Span, value: &ConstValue, ty: InternedTyId) {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => {
                self.validate_primitive_typed_value(span, value, primitive)
            }
            Some(TyKind::Array { elem, .. }) => {
                if self.runtime_array_accepts_const_string(value, ty) {
                    return;
                }
                if let ConstValue::String(value) = value
                    && self.runtime_array_is_char_array(ty)
                    && let Some(expected_len) = self.runtime_array_len(ty)
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        span,
                        format!(
                            "const array length {} does not match expected length {expected_len}",
                            value.chars().count()
                        ),
                    ));
                    return;
                }
                let ConstValue::Array(values) = value else {
                    self.push_const_type_mismatch(span, "array");
                    return;
                };
                if let Some(expected_len) = self.runtime_array_len(ty)
                    && u64::try_from(values.len()) != Ok(expected_len)
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        span,
                        format!(
                            "const array length {} does not match expected length {expected_len}",
                            values.len()
                        ),
                    ));
                }
                for value in values {
                    self.validate_runtime_typed_value(span, value, elem);
                }
            }
            Some(TyKind::Vector { elem, lanes }) => {
                let ConstValue::Vector(values) = value else {
                    self.push_const_type_mismatch(span, "vector");
                    return;
                };
                if usize::try_from(lanes).ok() != Some(values.len()) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        span,
                        format!(
                            "const vector lane count {} does not match expected lane count {lanes}",
                            values.len()
                        ),
                    ));
                }
                for value in values {
                    self.validate_primitive_typed_value(span, value, elem);
                }
            }
            Some(TyKind::Pointer { elem, .. }) => {
                let ConstValue::Pointer(pointer) = value else {
                    self.push_const_type_mismatch(span, "pointer");
                    return;
                };
                if let ConstPointerValue::Frozen { pointee, .. } = pointer {
                    self.validate_runtime_typed_value(span, pointee, elem);
                }
            }
            Some(TyKind::Optional { elem }) => match value {
                ConstValue::Optional(Some(value)) => {
                    self.validate_runtime_typed_value(span, value, elem);
                }
                ConstValue::Optional(None) => {}
                _ => self.push_const_type_mismatch(span, "optional"),
            },
            Some(TyKind::ErrorUnion { error, value: ok }) => {
                let ConstValue::ErrorUnion(value) = value else {
                    self.push_const_type_mismatch(span, "error union");
                    return;
                };
                match value {
                    Ok(value) => self.validate_runtime_typed_value(span, value, ok),
                    Err(value) => self.validate_runtime_typed_value(span, value, error),
                }
            }
            Some(TyKind::Nominal { def_id, .. }) => match self.def_kind_of(def_id) {
                Some(DefKind::Struct) => self.validate_nominal_struct_value(span, value, ty),
                Some(DefKind::Union) => self.validate_nominal_union_value(span, value, ty),
                Some(DefKind::Enum) => self.validate_nominal_enum_value(span, value, def_id),
                _ => {}
            },
            _ => {}
        }
    }

    fn validate_nominal_struct_value(&mut self, span: Span, value: &ConstValue, ty: InternedTyId) {
        let ConstValue::Struct(values) = value else {
            self.push_const_type_mismatch(span, "struct");
            return;
        };
        let Some((def_id, args, const_args)) = self.expected_nominal_parts(ty) else {
            return;
        };
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return;
        }
        let Some(signature) = self.struct_signature_for(def_id) else {
            return;
        };
        let Some(field_tys) = self.const_struct_field_types(&signature, &args, &const_args) else {
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
            self.push_const_missing_struct_field(span, name);
        }
        for field in &field_set.unknown_fields {
            self.push_const_extra_struct_field(span, &field.name);
        }
    }

    fn validate_nominal_union_value(&mut self, span: Span, value: &ConstValue, ty: InternedTyId) {
        let ConstValue::Union(value) = value else {
            self.push_const_type_mismatch(span, "union");
            return;
        };
        let Some((_def_id, _args, _const_args)) = self.expected_nominal_parts(ty) else {
            return;
        };
        let Some(target) =
            nia_layout::TargetDataLayout::from_pointer_width(self.input.target.pointer_width)
        else {
            return;
        };
        let Some((ConstAbiType::Union { fields, size }, _)) =
            self.const_union_abi_type(span, ty, target)
        else {
            self.push_const_type_mismatch(span, "union");
            return;
        };
        let Some(endianness) = ConstEndianness::from_target_name(&self.input.target.endian) else {
            return;
        };
        if let Err(message) = value.validate_abi(&fields, size, endianness) {
            self.diagnostics
                .push(Diagnostic::user_error_at(codes::CONST, span, message));
        }
    }

    fn validate_nominal_enum_value(
        &mut self,
        span: Span,
        value: &ConstValue,
        enum_id: GlobalDefId,
    ) {
        let Some(signature) = self
            .signatures_for_module(enum_id.module_id)
            .and_then(|signatures| signatures.as_ref().enums.get(&enum_id.def_id).cloned())
        else {
            self.push_const_type_mismatch(span, "enum");
            return;
        };
        if let ConstValue::Int(value) = value
            && signature.is_open
            && signature.variants.iter().all(|variant| {
                matches!(
                    variant.payload,
                    nia_item_signatures::EnumVariantPayloadSignature::Unit
                )
            })
        {
            self.validate_runtime_typed_value(
                span,
                &ConstValue::Int(*value),
                signature.backing_type,
            );
            return;
        }
        let ConstValue::Enum { variant, payload } = value else {
            self.push_const_type_mismatch(span, "enum");
            return;
        };
        let Some(defs) = self.global_defs(variant.module_id) else {
            self.push_const_type_mismatch(span, "enum");
            return;
        };
        let Some(owner) = defs
            .as_ref()
            .defs
            .get(variant.def_id)
            .and_then(|def| def.parent)
        else {
            self.push_const_type_mismatch(span, "enum");
            return;
        };
        if variant.module_id != enum_id.module_id || owner != enum_id.def_id {
            self.push_const_type_mismatch(span, "enum");
            return;
        }
        let Some(variant) = signature
            .variants
            .iter()
            .find(|candidate| candidate.def_id == variant.def_id)
        else {
            self.push_const_type_mismatch(span, "enum");
            return;
        };
        let current_module = self.current_execution_module_id();
        match (&variant.payload, payload) {
            (
                nia_item_signatures::EnumVariantPayloadSignature::Unit,
                nia_const_eval::ConstEnumPayload::Unit,
            ) => {}
            (
                nia_item_signatures::EnumVariantPayloadSignature::Tuple(field_tys),
                nia_const_eval::ConstEnumPayload::Tuple(values),
            ) if field_tys.len() == values.len() => {
                for (value, ty) in values.iter().zip(field_tys) {
                    let Some(ty) = self.type_for_module_or_none(*ty, current_module) else {
                        continue;
                    };
                    self.validate_runtime_typed_value(span, value, ty);
                }
            }
            (
                nia_item_signatures::EnumVariantPayloadSignature::Named(field_tys),
                nia_const_eval::ConstEnumPayload::Named(values),
            ) => {
                let field_set: FieldSetCheck<SymbolId> = check_value_field_set(
                    values.keys().copied(),
                    field_tys.iter().map(|field| field.name),
                );
                if !field_set.is_valid() {
                    self.push_const_type_mismatch(span, "enum payload");
                    return;
                }
                for field in field_tys {
                    let Some(value) = values.get(&field.name) else {
                        continue;
                    };
                    let Some(ty) = self.type_for_module_or_none(field.ty, current_module) else {
                        continue;
                    };
                    self.validate_runtime_typed_value(span, value, ty);
                }
            }
            _ => self.push_const_type_mismatch(span, "enum payload"),
        }
    }

    fn validate_primitive_typed_value(
        &mut self,
        span: Span,
        value: &ConstValue,
        primitive: PrimitiveTy,
    ) {
        match (value, primitive) {
            (ConstValue::Int(value), primitive) => {
                let fits = if primitive == PrimitiveTy::Char {
                    !value.is_signed()
                        && u32::try_from(value.bits())
                            .ok()
                            .and_then(char::from_u32)
                            .is_some()
                } else if primitive.is_integer() {
                    value.fits_primitive_int(primitive, self.input.target.pointer_width)
                } else {
                    self.push_const_primitive_mismatch(span, primitive);
                    return;
                };
                if !fits {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        span,
                        format!(
                            "const integer value {value:?} is out of range for {}",
                            primitive.name()
                        ),
                    ));
                }
            }
            (ConstValue::Bool(_), PrimitiveTy::Bool) => {}
            (ConstValue::Float(value), PrimitiveTy::F32) => {
                let value = *value as f32;
                if !value.is_finite() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        span,
                        "const float value is out of range for f32",
                    ));
                }
            }
            (ConstValue::Float(value), PrimitiveTy::F64) => {
                if !value.is_finite() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        span,
                        "const float value is out of range for f64",
                    ));
                }
            }
            (_, primitive) => {
                self.push_const_primitive_mismatch(span, primitive);
            }
        }
    }

    fn runtime_array_len(&mut self, ty: InternedTyId) -> Option<u64> {
        let Some(TyKind::Array { len, .. }) = self.ty_kind(ty) else {
            return None;
        };
        self.array_len_const_value(len)
    }

    fn runtime_array_accepts_const_string(&mut self, value: &ConstValue, ty: InternedTyId) -> bool {
        let ConstValue::String(value) = value else {
            return false;
        };
        if !self.runtime_array_is_char_array(ty) {
            return false;
        }
        self.runtime_array_len(ty)
            .is_none_or(|len| u64::try_from(value.chars().count()) == Ok(len))
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

    fn push_const_type_mismatch(&mut self, span: Span, expected: &str) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::CONST,
            span,
            format!("const value does not match expected {expected} type"),
        ));
    }

    fn push_const_missing_struct_field(&mut self, span: Span, name: &SymbolId) {
        let name = self.symbol_name(*name);
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::CONST,
            span,
            format!("const struct value is missing field `{name}`"),
        ));
    }

    fn push_const_extra_struct_field(&mut self, span: Span, name: &SymbolId) {
        let name = self.symbol_name(*name);
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::CONST,
            span,
            format!("const struct value has extra field `{name}`"),
        ));
    }

    fn push_const_primitive_mismatch(&mut self, span: Span, primitive: PrimitiveTy) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::CONST,
            span,
            format!(
                "const value does not match primitive type {}",
                primitive.name()
            ),
        ));
    }
}
