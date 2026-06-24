// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use nia_comptime_check::ComptimeCheck;
use nia_defs::{DefCollection, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{
    DefId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TyInternerId, TypeOwner,
};
use nia_item_signatures::{EnumSignature, ProgramEnumSignature, ProgramTraitImplSignature};
use nia_layout::Layouts;
use nia_mangle::{mangle_base_symbol, mangle_type_with, sanitize_symbol_part};
use nia_sema_ir::GenericInstantiation;
use nia_span::Span;
use nia_trait_solve::TraitSolverContext;
use nia_ty::{ArrayLenTy, AssociatedTypeBindingTy, ConstExprSummary, TyInterner, TyKind};
use nia_type_normalize::TypeNormalization;

#[derive(Debug, Clone, PartialEq)]
pub struct Monomorphization {
    pub instances: Vec<MonoInstance>,
    pub type_interners: HashMap<TyInternerId, TyInterner>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoInstance {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
    pub symbol: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonomorphizeModuleInput<'a> {
    pub module_id: ModuleId,
    pub defs: &'a DefCollection,
    pub interner: &'a TyInterner,
    pub normalization: &'a TypeNormalization,
    pub comptime: &'a ComptimeCheck,
    pub const_expr_summaries: &'a HashMap<GlobalConstExprId, ConstExprSummary>,
    pub layouts: Option<&'a Layouts>,
    pub local_enums: &'a HashMap<nia_ids::DefId, EnumSignature>,
    pub program_enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub instantiations: &'a [GenericInstantiation],
}

pub fn collect_monomorphizations(inputs: &[MonomorphizeModuleInput<'_>]) -> Monomorphization {
    let mut collector = MonoCollector {
        defs_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.defs))
            .collect(),
        interners_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.interner))
            .collect(),
        normalizations_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.normalization))
            .collect(),
        comptime_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.comptime))
            .collect(),
        const_expr_summaries_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.const_expr_summaries))
            .collect(),
        working_interners_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.interner.clone()))
            .collect(),
        layouts_by_module: inputs
            .iter()
            .filter_map(|input| input.layouts.map(|layouts| (input.module_id, layouts)))
            .collect(),
        local_enums_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.local_enums))
            .collect(),
        program_enums: inputs
            .first()
            .map(|input| input.program_enums)
            .unwrap_or(&EMPTY_PROGRAM_ENUMS),
        trait_impls: inputs.first().map(|input| input.trait_impls).unwrap_or(&[]),
        instantiations_by_source: collect_instantiations_by_source(inputs),
        source_instantiation_edges: collect_source_instantiation_edges(inputs),
        recorded_generics_by_def: collect_recorded_generics_by_def(inputs),
        instances: Vec::new(),
        seen: HashSet::new(),
        type_symbols: HashMap::new(),
        def_names: HashMap::new(),
        base_symbols: HashMap::new(),
        type_instantiations: HashMap::new(),
        type_substitutions: Vec::new(),
        type_substitution_ids: HashMap::new(),
        effective_generics: HashMap::new(),
        missing_array_len_diagnostics: HashSet::new(),
        diagnostics: Vec::new(),
    };
    for input in inputs {
        collector.collect_module(input);
    }
    Monomorphization {
        instances: collector.instances,
        type_interners: collector
            .working_interners_by_module
            .into_values()
            .map(|interner| (interner.interner_id(), interner))
            .collect(),
        diagnostics: collector.diagnostics,
    }
}

struct MonoCollector<'a> {
    defs_by_module: HashMap<ModuleId, &'a DefCollection>,
    interners_by_module: HashMap<ModuleId, &'a TyInterner>,
    normalizations_by_module: HashMap<ModuleId, &'a TypeNormalization>,
    comptime_by_module: HashMap<ModuleId, &'a ComptimeCheck>,
    const_expr_summaries_by_module:
        HashMap<ModuleId, &'a HashMap<GlobalConstExprId, ConstExprSummary>>,
    working_interners_by_module: HashMap<ModuleId, TyInterner>,
    layouts_by_module: HashMap<ModuleId, &'a Layouts>,
    local_enums_by_module: HashMap<ModuleId, &'a HashMap<nia_ids::DefId, EnumSignature>>,
    program_enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    trait_impls: &'a [ProgramTraitImplSignature],
    instantiations_by_source: HashMap<GlobalDefId, Vec<usize>>,
    source_instantiation_edges: Vec<SourceInstantiationEdge>,
    recorded_generics_by_def: HashMap<GlobalDefId, Vec<String>>,
    instances: Vec<MonoInstance>,
    seen: HashSet<MonoInstanceKey>,
    type_symbols: HashMap<(ModuleId, InternedTyId), String>,
    def_names: HashMap<GlobalDefId, String>,
    base_symbols: HashMap<GlobalDefId, String>,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    type_substitutions: Vec<HashMap<String, InternedTyId>>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
    effective_generics: HashMap<GlobalDefId, Vec<String>>,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    diagnostics: Vec<Diagnostic>,
}

static EMPTY_PROGRAM_ENUMS: std::sync::LazyLock<HashMap<GlobalDefId, ProgramEnumSignature>> =
    std::sync::LazyLock::new(HashMap::new);
static EMPTY_LOCAL_ENUMS: std::sync::LazyLock<HashMap<DefId, EnumSignature>> =
    std::sync::LazyLock::new(HashMap::new);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MonoInstanceKey {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TypeInstantiationKey {
    module_id: ModuleId,
    ty: InternedTyId,
    substitutions: TypeSubstitutionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectionInstantiationKey {
    self_ty: InternedTyId,
    trait_id: nia_ty::TraitId,
    trait_args: Vec<InternedTyId>,
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TypeSubstitutionId(usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TypeSubstitutionKey {
    substitutions: Vec<(String, InternedTyId)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceInstantiationEdge {
    source_module_id: ModuleId,
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingMonoInstance {
    key: MonoInstanceKey,
    span: Span,
}

const MAX_MONOMORPHIZED_INSTANCES: usize = 1024;
const MAX_MONOMORPHIZED_INSTANCE_TYPE_DEPTH: usize = 256;

impl MonoCollector<'_> {
    fn type_owner(ty: InternedTyId) -> TypeOwner {
        ty.owner()
    }

    fn collect_module(&mut self, input: &MonomorphizeModuleInput<'_>) {
        let mut pending = VecDeque::new();
        for instantiation in input.instantiations {
            if !self.is_generic_def(instantiation.def_id) {
                continue;
            }
            if instantiation
                .source_def_id
                .is_some_and(|source_def_id| self.is_generic_def(source_def_id))
                && self.args_contain_generic_param(input.module_id, &instantiation.args)
            {
                continue;
            }
            let key = MonoInstanceKey {
                def_id: instantiation.def_id,
                arg_module_id: input.module_id,
                args: instantiation.args.clone(),
            };
            self.enqueue_instance(&mut pending, key, instantiation.span);
        }
        self.expand_pending_instances(pending);
    }

    fn is_generic_def(&mut self, def_id: GlobalDefId) -> bool {
        if self.has_recorded_generics(def_id) {
            return true;
        }
        let Some(defs) = self.defs_by_module.get(&def_id.module_id) else {
            return false;
        };
        let Some(def) = defs.defs.get(def_id.def_id) else {
            return false;
        };
        if !matches!(
            def.kind,
            DefKind::Function | DefKind::Method | DefKind::TraitMethod
        ) {
            return false;
        }
        !self.effective_generics_for(def_id).is_empty()
    }

    fn compute_effective_generics(&self, def_id: GlobalDefId) -> Vec<String> {
        if let Some(generics) = self.recorded_generics(def_id) {
            return generics.to_vec();
        }
        let Some(defs) = self.defs_by_module.get(&def_id.module_id) else {
            return Vec::new();
        };
        let Some(def) = defs.defs.get(def_id.def_id) else {
            return Vec::new();
        };
        if def.kind == DefKind::TraitMethod {
            let mut generics = vec!["Self".to_string()];
            generics.extend(
                def.parent
                    .and_then(|parent| defs.defs.get(parent))
                    .map(|parent| parent.generics.clone())
                    .unwrap_or_default(),
            );
            generics.extend(def.generics.clone());
            return generics;
        }
        let mut generics = def
            .parent
            .and_then(|parent| defs.defs.get(parent))
            .map(|parent| parent.generics.clone())
            .unwrap_or_default();
        generics.extend(def.generics.clone());
        generics
    }

    fn effective_generics_for(&mut self, def_id: GlobalDefId) -> &[String] {
        if !self.effective_generics.contains_key(&def_id) {
            let generics = self.compute_effective_generics(def_id);
            self.effective_generics.insert(def_id, generics);
        }
        self.effective_generics
            .get(&def_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn has_recorded_generics(&self, def_id: GlobalDefId) -> bool {
        self.recorded_generics_by_def.contains_key(&def_id)
    }

    fn args_contain_generic_param(&self, module_id: ModuleId, args: &[InternedTyId]) -> bool {
        args.iter()
            .copied()
            .any(|arg| self.ty_contains_generic_param(module_id, arg))
    }

    fn ty_contains_generic_param(&self, module_id: ModuleId, ty: InternedTyId) -> bool {
        let Some(kind) = self
            .working_interners_by_module
            .get(&module_id)
            .and_then(|interner| interner.get(ty))
        else {
            return false;
        };
        match kind {
            TyKind::GenericParam(_) => true,
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem } => self.ty_contains_generic_param(module_id, *elem),
            TyKind::Array { elem, .. } => self.ty_contains_generic_param(module_id, *elem),
            TyKind::Range { bound, .. } => {
                bound.is_some_and(|bound| self.ty_contains_generic_param(module_id, bound))
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                params
                    .iter()
                    .any(|param| self.ty_contains_generic_param(module_id, *param))
                    || self.ty_contains_generic_param(module_id, *return_type)
            }
            TyKind::Optional { elem } => self.ty_contains_generic_param(module_id, *elem),
            TyKind::ErrorUnion { error, value } => {
                self.ty_contains_generic_param(module_id, *error)
                    || self.ty_contains_generic_param(module_id, *value)
            }
            TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. } => args
                .iter()
                .any(|arg| self.ty_contains_generic_param(module_id, *arg)),
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                trait_args
                    .iter()
                    .any(|arg| self.ty_contains_generic_param(module_id, *arg))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.ty_contains_generic_param(module_id, *arg))
                            || self.ty_contains_generic_param(module_id, binding.ty)
                    })
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                self.ty_contains_generic_param(module_id, *self_ty)
                    || trait_args
                        .iter()
                        .any(|arg| self.ty_contains_generic_param(module_id, *arg))
            }
            TyKind::Primitive(_) | TyKind::Vector { .. } | TyKind::ComptimeOnly | TyKind::Error => {
                false
            }
        }
    }

    fn recorded_generics(&self, def_id: GlobalDefId) -> Option<&[String]> {
        self.recorded_generics_by_def
            .get(&def_id)
            .map(Vec::as_slice)
    }

    fn expand_pending_instances(&mut self, mut pending: VecDeque<PendingMonoInstance>) {
        let mut expanded = HashSet::new();
        while let Some(pending_instance) = pending.pop_front() {
            if !expanded.insert(pending_instance.key.clone()) {
                continue;
            }

            let Some(edge_indices) = self
                .instantiations_by_source
                .get(&pending_instance.key.def_id)
                .cloned()
            else {
                continue;
            };
            let substitutions = self.generic_substitutions_for_instance(&pending_instance.key);
            let substitution_id = self.intern_ordered_type_substitutions(substitutions);
            for edge_index in edge_indices {
                let Some(edge) = self.source_instantiation_edges.get(edge_index) else {
                    continue;
                };
                let source_module_id = edge.source_module_id;
                let edge_def_id = edge.def_id;
                let edge_span = edge.span;
                let edge_args = edge.args.clone();
                if !self.is_generic_def(edge_def_id) {
                    continue;
                }
                let args = self.instantiate_args(source_module_id, &edge_args, substitution_id);
                let edge_key = MonoInstanceKey {
                    def_id: edge_def_id,
                    arg_module_id: source_module_id,
                    args,
                };
                self.enqueue_instance(&mut pending, edge_key, edge_span);
            }
        }
    }

    fn enqueue_instance(
        &mut self,
        pending: &mut VecDeque<PendingMonoInstance>,
        key: MonoInstanceKey,
        span: Span,
    ) {
        if !self.seen.insert(key.clone()) {
            return;
        }
        if self.args_contain_generic_param(key.arg_module_id, &key.args) {
            return;
        }
        if self.instances.len() >= MAX_MONOMORPHIZED_INSTANCES {
            self.report_instance_limit(span, &key);
            return;
        }
        if key.args.iter().any(|arg| {
            self.ty_exceeds_instance_depth(
                key.arg_module_id,
                *arg,
                MAX_MONOMORPHIZED_INSTANCE_TYPE_DEPTH,
            )
        }) {
            self.report_instance_type_depth_limit(span, &key);
            return;
        }
        let symbol = self.instance_symbol(&key);
        self.instances.push(MonoInstance {
            def_id: key.def_id,
            arg_module_id: key.arg_module_id,
            args: key.args.clone(),
            symbol,
            span,
        });
        pending.push_back(PendingMonoInstance { key, span });
    }

    fn report_instance_limit(&mut self, span: Span, key: &MonoInstanceKey) {
        let name = self.def_name(key.def_id);
        self.diagnostics.push(
            Diagnostic::user_error(codes::LLVM_CODEGEN,
                "generic instantiation did not converge before the instance limit",
            )
            .primary(
                span,
                format!(
                    "instantiating `{name}` would exceed the monomorphization instance limit"
                ),
            )
            .note(
                "recursive calls to an already-seen concrete generic instance are allowed and reuse the existing monomorphized function",
            )
            .note(
                "this usually means a recursive generic call keeps producing new type arguments, such as T, &T, &&T, ...",
            )
            .help("move the recursion behind a runtime representation or make the recursive call reuse a finite set of concrete type arguments")
            .debug("limit", MAX_MONOMORPHIZED_INSTANCES)
            .debug("known_instances", self.instances.len())
            .debug("def_id", key.def_id)
            .debug("arg_module_id", key.arg_module_id)
            .finish(),
        );
    }

    fn report_instance_type_depth_limit(&mut self, span: Span, key: &MonoInstanceKey) {
        let name = self.def_name(key.def_id);
        self.diagnostics.push(
            Diagnostic::user_error(codes::LLVM_CODEGEN,
                "generic instantiation did not converge before the type depth limit",
            )
            .primary(
                span,
                format!(
                    "instantiating `{name}` would exceed the monomorphization type depth limit"
                ),
            )
            .note(
                "recursive calls to an already-seen concrete generic instance are allowed and reuse the existing monomorphized function",
            )
            .note(
                "this usually means a recursive generic call keeps growing a type argument, such as T, &T, &&T, ...",
            )
            .help("move the recursion behind a runtime representation or make the recursive call reuse a finite set of concrete type arguments")
            .debug("type_depth_limit", MAX_MONOMORPHIZED_INSTANCE_TYPE_DEPTH)
            .debug("known_instances", self.instances.len())
            .debug("def_id", key.def_id)
            .debug("arg_module_id", key.arg_module_id)
            .finish(),
        );
    }

    fn ty_exceeds_instance_depth(
        &self,
        module_id: ModuleId,
        ty: InternedTyId,
        remaining: usize,
    ) -> bool {
        if remaining == 0 {
            return true;
        }
        let Some(kind) = self
            .working_interners_by_module
            .get(&module_id)
            .or_else(|| self.interners_by_module.get(&module_id).copied())
            .and_then(|interner| interner.get(ty))
        else {
            return false;
        };
        let next = remaining - 1;
        match kind {
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem } => {
                self.ty_exceeds_instance_depth(module_id, *elem, next)
            }
            TyKind::Array { elem, .. } => self.ty_exceeds_instance_depth(module_id, *elem, next),
            TyKind::Range { bound, .. } => {
                bound.is_some_and(|bound| self.ty_exceeds_instance_depth(module_id, bound, next))
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                params
                    .iter()
                    .any(|param| self.ty_exceeds_instance_depth(module_id, *param, next))
                    || self.ty_exceeds_instance_depth(module_id, *return_type, next)
            }
            TyKind::Optional { elem } => self.ty_exceeds_instance_depth(module_id, *elem, next),
            TyKind::ErrorUnion { error, value } => {
                self.ty_exceeds_instance_depth(module_id, *error, next)
                    || self.ty_exceeds_instance_depth(module_id, *value, next)
            }
            TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. } => args
                .iter()
                .any(|arg| self.ty_exceeds_instance_depth(module_id, *arg, next)),
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                trait_args
                    .iter()
                    .any(|arg| self.ty_exceeds_instance_depth(module_id, *arg, next))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.ty_exceeds_instance_depth(module_id, *arg, next))
                            || self.ty_exceeds_instance_depth(module_id, binding.ty, next)
                    })
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                self.ty_exceeds_instance_depth(module_id, *self_ty, next)
                    || trait_args
                        .iter()
                        .any(|arg| self.ty_exceeds_instance_depth(module_id, *arg, next))
            }
            TyKind::GenericParam(_)
            | TyKind::Primitive(_)
            | TyKind::Vector { .. }
            | TyKind::ComptimeOnly
            | TyKind::Error => false,
        }
    }

    fn generic_substitutions_for_instance(
        &mut self,
        key: &MonoInstanceKey,
    ) -> Vec<(String, InternedTyId)> {
        let def_module_id = key.def_id.module_id;
        let generics = self.effective_generics_for(key.def_id).to_vec();
        generics
            .into_iter()
            .zip(key.args.iter().copied())
            .map(|(generic, arg)| (generic, self.import_ty_to_module(def_module_id, arg)))
            .collect()
    }

    fn instantiate_args(
        &mut self,
        module_id: ModuleId,
        args: &[InternedTyId],
        substitutions: TypeSubstitutionId,
    ) -> Vec<InternedTyId> {
        args.iter()
            .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
            .collect()
    }

    fn instantiate_ty(
        &mut self,
        module_id: ModuleId,
        ty: InternedTyId,
        substitutions: TypeSubstitutionId,
    ) -> InternedTyId {
        self.instantiate_ty_inner(module_id, ty, substitutions, &mut HashSet::new())
    }

    fn instantiate_ty_inner(
        &mut self,
        module_id: ModuleId,
        ty: InternedTyId,
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> InternedTyId {
        let key = TypeInstantiationKey {
            module_id,
            ty,
            substitutions,
        };
        let can_use_cache = active_projections.is_empty();
        if can_use_cache && let Some(cached) = self.type_instantiations.get(&key).copied() {
            return cached;
        }
        let Some(kind) = self
            .working_interners_by_module
            .get(&module_id)
            .and_then(|interner| interner.get(ty))
            .cloned()
        else {
            return ty;
        };
        let instantiated = match kind {
            TyKind::GenericParam(name) => self
                .type_substitutions
                .get(substitutions.0)
                .and_then(|substitutions| substitutions.get(&name))
                .copied()
                .unwrap_or(ty),
            TyKind::Pointer { is_readonly, elem } => {
                let elem =
                    self.instantiate_ty_inner(module_id, elem, substitutions, active_projections);
                self.intern_working_ty(module_id, TyKind::Pointer { is_readonly, elem })
            }
            TyKind::VolatilePointer { is_readonly, elem } => {
                let elem =
                    self.instantiate_ty_inner(module_id, elem, substitutions, active_projections);
                self.intern_working_ty(module_id, TyKind::VolatilePointer { is_readonly, elem })
            }
            TyKind::Slice { is_readonly, elem } => {
                let elem =
                    self.instantiate_ty_inner(module_id, elem, substitutions, active_projections);
                self.intern_working_ty(module_id, TyKind::Slice { is_readonly, elem })
            }
            TyKind::SlicePointee { elem } => {
                let elem =
                    self.instantiate_ty_inner(module_id, elem, substitutions, active_projections);
                self.intern_working_ty(module_id, TyKind::SlicePointee { elem })
            }
            TyKind::Array { len, elem } => {
                let elem =
                    self.instantiate_ty_inner(module_id, elem, substitutions, active_projections);
                self.intern_working_ty(module_id, TyKind::Array { len, elem })
            }
            TyKind::Nominal { def_id, args } => {
                let args = args
                    .iter()
                    .map(|arg| {
                        self.instantiate_ty_inner(
                            module_id,
                            *arg,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                self.intern_working_ty(module_id, TyKind::Nominal { def_id, args })
            }
            TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            } => {
                let self_ty = self.instantiate_ty_inner(
                    module_id,
                    self_ty,
                    substitutions,
                    active_projections,
                );
                let trait_args: Vec<InternedTyId> = trait_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_ty_inner(
                            module_id,
                            *arg,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                let projection_key = ProjectionInstantiationKey {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    name: name.clone(),
                };
                let projection = self.intern_working_ty(
                    module_id,
                    TyKind::Projection {
                        self_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        name: name.clone(),
                    },
                );
                if !active_projections.insert(projection_key.clone()) {
                    projection
                } else {
                    let resolved = self
                        .resolve_associated_type_projection(
                            module_id,
                            self_ty,
                            trait_id,
                            &trait_args,
                            &name,
                        )
                        .map(|resolved| {
                            self.instantiate_ty_inner(
                                module_id,
                                resolved,
                                substitutions,
                                active_projections,
                            )
                        });
                    active_projections.remove(&projection_key);
                    resolved.unwrap_or(projection)
                }
            }
            TyKind::Range { kind, bound } => {
                let bound = bound.map(|bound| {
                    self.instantiate_ty_inner(module_id, bound, substitutions, active_projections)
                });
                self.intern_working_ty(module_id, TyKind::Range { kind, bound })
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                let params = params
                    .iter()
                    .map(|param| {
                        self.instantiate_ty_inner(
                            module_id,
                            *param,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                let return_type = self.instantiate_ty_inner(
                    module_id,
                    return_type,
                    substitutions,
                    active_projections,
                );
                self.intern_working_ty(
                    module_id,
                    TyKind::FunctionPointer {
                        params,
                        return_type,
                        is_variadic,
                    },
                )
            }
            TyKind::Optional { elem } => {
                let elem =
                    self.instantiate_ty_inner(module_id, elem, substitutions, active_projections);
                self.intern_working_ty(module_id, TyKind::Optional { elem })
            }
            TyKind::ErrorUnion { error, value } => {
                let error =
                    self.instantiate_ty_inner(module_id, error, substitutions, active_projections);
                let value =
                    self.instantiate_ty_inner(module_id, value, substitutions, active_projections);
                self.intern_working_ty(module_id, TyKind::ErrorUnion { error, value })
            }
            TyKind::BuiltinTrait { trait_id, args } => {
                let args = args
                    .iter()
                    .map(|arg| {
                        self.instantiate_ty_inner(
                            module_id,
                            *arg,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                self.intern_working_ty(module_id, TyKind::BuiltinTrait { trait_id, args })
            }
            TyKind::TraitObject {
                trait_id,
                trait_args,
                associated_type_bindings,
                is_readonly,
            } => {
                let trait_args = trait_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_ty_inner(
                            module_id,
                            *arg,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .iter()
                    .map(|binding| AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .iter()
                            .map(|arg| {
                                self.instantiate_ty_inner(
                                    module_id,
                                    *arg,
                                    substitutions,
                                    active_projections,
                                )
                            })
                            .collect(),
                        name: binding.name.clone(),
                        ty: self.instantiate_ty_inner(
                            module_id,
                            binding.ty,
                            substitutions,
                            active_projections,
                        ),
                    })
                    .collect();
                self.intern_working_ty(
                    module_id,
                    TyKind::TraitObject {
                        trait_id,
                        trait_args,
                        associated_type_bindings,
                        is_readonly,
                    },
                )
            }
            TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            } => {
                let trait_args = trait_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_ty_inner(
                            module_id,
                            *arg,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .iter()
                    .map(|binding| AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .iter()
                            .map(|arg| {
                                self.instantiate_ty_inner(
                                    module_id,
                                    *arg,
                                    substitutions,
                                    active_projections,
                                )
                            })
                            .collect(),
                        name: binding.name.clone(),
                        ty: self.instantiate_ty_inner(
                            module_id,
                            binding.ty,
                            substitutions,
                            active_projections,
                        ),
                    })
                    .collect();
                self.intern_working_ty(
                    module_id,
                    TyKind::TraitObjectPointee {
                        trait_id,
                        trait_args,
                        associated_type_bindings,
                    },
                )
            }
            TyKind::Primitive(_) | TyKind::Vector { .. } | TyKind::ComptimeOnly | TyKind::Error => {
                ty
            }
        };
        if can_use_cache {
            self.type_instantiations.insert(key, instantiated);
        }
        instantiated
    }

    fn resolve_associated_type_projection(
        &mut self,
        module_id: ModuleId,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        let normalization = *self.normalizations_by_module.get(&module_id)?;
        let local_enums = self
            .local_enums_by_module
            .get(&module_id)
            .copied()
            .unwrap_or(&EMPTY_LOCAL_ENUMS);
        let layouts = self.layouts_by_module.get(&module_id).copied();
        let interner = self.working_interners_by_module.get_mut(&module_id)?;
        let context = TraitSolverContext {
            normalization,
            trait_impls: self.trait_impls,
            layouts,
            local_module_id: module_id,
            local_enums,
            program_enums: Some(self.program_enums),
        };
        let mut solver = context.solver_with_associated_type_assumptions(interner, &[], &[]);
        solver.resolve_associated_type(self_ty, trait_id, trait_args, name)
    }

    fn import_ty_to_module(
        &mut self,
        target_module_id: ModuleId,
        ty: InternedTyId,
    ) -> InternedTyId {
        let source_module_id = Self::type_owner(ty).module_id();
        if source_module_id == target_module_id {
            return ty;
        }
        let Some(kind) = self
            .working_interners_by_module
            .get(&source_module_id)
            .or_else(|| self.interners_by_module.get(&source_module_id).copied())
            .and_then(|interner| interner.get(ty))
            .cloned()
        else {
            panic!(
                "Nia ICE: cannot import type {ty:?} from missing source module interner {source_module_id:?}"
            );
        };
        let imported = match kind {
            TyKind::Error => TyKind::Error,
            TyKind::ComptimeOnly => TyKind::ComptimeOnly,
            TyKind::Primitive(primitive) => TyKind::Primitive(primitive),
            TyKind::Vector { elem, lanes } => TyKind::Vector { elem, lanes },
            TyKind::GenericParam(name) => TyKind::GenericParam(name),
            TyKind::Pointer { is_readonly, elem } => TyKind::Pointer {
                is_readonly,
                elem: self.import_ty_to_module(target_module_id, elem),
            },
            TyKind::VolatilePointer { is_readonly, elem } => TyKind::VolatilePointer {
                is_readonly,
                elem: self.import_ty_to_module(target_module_id, elem),
            },
            TyKind::Slice { is_readonly, elem } => TyKind::Slice {
                is_readonly,
                elem: self.import_ty_to_module(target_module_id, elem),
            },
            TyKind::SlicePointee { elem } => TyKind::SlicePointee {
                elem: self.import_ty_to_module(target_module_id, elem),
            },
            TyKind::Array { len, elem } => TyKind::Array {
                len: self.import_array_len_to_module(target_module_id, len),
                elem: self.import_ty_to_module(target_module_id, elem),
            },
            TyKind::Range { kind, bound } => TyKind::Range {
                kind,
                bound: bound.map(|bound| self.import_ty_to_module(target_module_id, bound)),
            },
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => TyKind::FunctionPointer {
                params: params
                    .into_iter()
                    .map(|param| self.import_ty_to_module(target_module_id, param))
                    .collect(),
                return_type: self.import_ty_to_module(target_module_id, return_type),
                is_variadic,
            },
            TyKind::Optional { elem } => TyKind::Optional {
                elem: self.import_ty_to_module(target_module_id, elem),
            },
            TyKind::ErrorUnion { error, value } => TyKind::ErrorUnion {
                error: self.import_ty_to_module(target_module_id, error),
                value: self.import_ty_to_module(target_module_id, value),
            },
            TyKind::Nominal { def_id, args } => TyKind::Nominal {
                def_id,
                args: args
                    .into_iter()
                    .map(|arg| self.import_ty_to_module(target_module_id, arg))
                    .collect(),
            },
            TyKind::BuiltinTrait { trait_id, args } => TyKind::BuiltinTrait {
                trait_id,
                args: args
                    .into_iter()
                    .map(|arg| self.import_ty_to_module(target_module_id, arg))
                    .collect(),
            },
            TyKind::TraitObject {
                trait_id,
                trait_args,
                associated_type_bindings,
                is_readonly,
            } => TyKind::TraitObject {
                trait_id,
                trait_args: trait_args
                    .into_iter()
                    .map(|arg| self.import_ty_to_module(target_module_id, arg))
                    .collect(),
                associated_type_bindings: associated_type_bindings
                    .into_iter()
                    .map(|binding| AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.import_ty_to_module(target_module_id, arg))
                            .collect(),
                        name: binding.name,
                        ty: self.import_ty_to_module(target_module_id, binding.ty),
                    })
                    .collect(),
                is_readonly,
            },
            TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            } => TyKind::TraitObjectPointee {
                trait_id,
                trait_args: trait_args
                    .into_iter()
                    .map(|arg| self.import_ty_to_module(target_module_id, arg))
                    .collect(),
                associated_type_bindings: associated_type_bindings
                    .into_iter()
                    .map(|binding| AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.import_ty_to_module(target_module_id, arg))
                            .collect(),
                        name: binding.name,
                        ty: self.import_ty_to_module(target_module_id, binding.ty),
                    })
                    .collect(),
            },
            TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            } => TyKind::Projection {
                self_ty: self.import_ty_to_module(target_module_id, self_ty),
                trait_id,
                trait_args: trait_args
                    .into_iter()
                    .map(|arg| self.import_ty_to_module(target_module_id, arg))
                    .collect(),
                name,
            },
        };
        self.intern_working_ty(target_module_id, imported)
    }

    fn import_array_len_to_module(
        &mut self,
        target_module_id: ModuleId,
        len: ArrayLenTy,
    ) -> ArrayLenTy {
        match len {
            ArrayLenTy::Builtin { builtin, ty } => ArrayLenTy::Builtin {
                builtin,
                ty: self.import_ty_to_module(target_module_id, ty),
            },
            ArrayLenTy::Infer | ArrayLenTy::ConstValue(_) | ArrayLenTy::ConstExpr(_) => len,
        }
    }

    fn intern_working_ty(&mut self, module_id: ModuleId, kind: TyKind) -> InternedTyId {
        if let Some(interner) = self.working_interners_by_module.get_mut(&module_id) {
            return interner.intern(kind);
        }
        let Some(interner) = self.interners_by_module.get(&module_id).cloned() else {
            panic!("Nia ICE: cannot intern working type for missing module interner {module_id:?}");
        };
        let mut interner = interner.clone();
        let ty = interner.intern(kind);
        self.working_interners_by_module.insert(module_id, interner);
        ty
    }

    fn intern_ordered_type_substitutions(
        &mut self,
        substitutions: Vec<(String, InternedTyId)>,
    ) -> TypeSubstitutionId {
        self.intern_type_substitution_key(TypeSubstitutionKey { substitutions })
    }

    fn intern_type_substitution_key(&mut self, key: TypeSubstitutionKey) -> TypeSubstitutionId {
        if let Some(id) = self.type_substitution_ids.get(&key) {
            return *id;
        }
        let id = TypeSubstitutionId(self.type_substitutions.len());
        self.type_substitutions
            .push(key.substitutions.iter().cloned().collect());
        self.type_substitution_ids.insert(key, id);
        id
    }

    fn instance_symbol(&mut self, key: &MonoInstanceKey) -> String {
        let args = key
            .args
            .iter()
            .map(|arg| self.type_symbol(key.arg_module_id, *arg))
            .collect::<Vec<_>>()
            .join("_");
        let base_symbol = self.base_symbol(key.def_id);
        if args.is_empty() {
            base_symbol
        } else {
            format!("{base_symbol}__inst__{args}")
        }
    }

    fn base_symbol(&mut self, def_id: GlobalDefId) -> String {
        if let Some(symbol) = self.base_symbols.get(&def_id) {
            return symbol.clone();
        }
        let name = self.def_name(def_id);
        let symbol = mangle_base_symbol(def_id, &name);
        self.base_symbols.insert(def_id, symbol.clone());
        symbol
    }

    fn def_name(&mut self, def_id: GlobalDefId) -> String {
        if let Some(name) = self.def_names.get(&def_id) {
            return name.clone();
        }
        let name = def_name(&self.defs_by_module, def_id);
        self.def_names.insert(def_id, name.clone());
        name
    }

    fn type_symbol(&mut self, module_id: ModuleId, ty: InternedTyId) -> String {
        if let Some(symbol) = self.type_symbols.get(&(module_id, ty)) {
            return symbol.clone();
        }
        let Some(interner) = self
            .working_interners_by_module
            .get(&module_id)
            .or_else(|| self.interners_by_module.get(&module_id).copied())
        else {
            panic!("Nia ICE: cannot mangle type {ty:?} without module interner {module_id:?}");
        };
        if interner.get(ty).is_none() {
            panic!("Nia ICE: cannot mangle missing type {ty:?} in module interner {module_id:?}");
        }
        let defs_by_module = &self.defs_by_module;
        let def_names = &mut self.def_names;
        let comptime_by_module = &self.comptime_by_module;
        let const_expr_summaries_by_module = &self.const_expr_summaries_by_module;
        let missing_array_len_diagnostics = &mut self.missing_array_len_diagnostics;
        let diagnostics = &mut self.diagnostics;
        let symbol = mangle_type_with(
            interner,
            ty,
            |def_id| cached_def_name(defs_by_module, def_names, def_id),
            |id| {
                array_len(
                    comptime_by_module,
                    const_expr_summaries_by_module,
                    missing_array_len_diagnostics,
                    diagnostics,
                    id,
                )
            },
        );
        self.type_symbols.insert((module_id, ty), symbol.clone());
        symbol
    }
}

fn array_len(
    comptime_by_module: &HashMap<ModuleId, &ComptimeCheck>,
    const_expr_summaries_by_module: &HashMap<
        ModuleId,
        &HashMap<GlobalConstExprId, ConstExprSummary>,
    >,
    missing_array_len_diagnostics: &mut HashSet<GlobalConstExprId>,
    diagnostics: &mut Vec<Diagnostic>,
    id: GlobalConstExprId,
) -> Option<u64> {
    let value = comptime_by_module
        .get(&id.module_id)
        .and_then(|comptime| comptime.array_lengths.get(&id).copied());
    if value.is_none() && missing_array_len_diagnostics.insert(id) {
        let span = const_expr_summaries_by_module
            .get(&id.module_id)
            .and_then(|summaries| summaries.get(&id))
            .map(|summary| summary.span)
            .unwrap_or_default();
        diagnostics.push(Diagnostic::user_error_at(
            codes::LLVM_CODEGEN,
            span,
            format!(
                "array length {id:?} was not evaluated before monomorphization symbol generation"
            ),
        ));
    }
    value
}

fn def_name(defs_by_module: &HashMap<ModuleId, &DefCollection>, def_id: GlobalDefId) -> String {
    defs_by_module
        .get(&def_id.module_id)
        .and_then(|defs| defs.defs.get(def_id.def_id))
        .map(|def| sanitize_symbol_part(&def.name))
        .unwrap_or_else(|| format!("def{}", def_id.def_id.0))
}

fn cached_def_name(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    def_names: &mut HashMap<GlobalDefId, String>,
    def_id: GlobalDefId,
) -> String {
    if let Some(name) = def_names.get(&def_id) {
        return name.clone();
    }
    let name = def_name(defs_by_module, def_id);
    def_names.insert(def_id, name.clone());
    name
}

fn collect_instantiations_by_source(
    inputs: &[MonomorphizeModuleInput<'_>],
) -> HashMap<GlobalDefId, Vec<usize>> {
    let mut by_source: HashMap<GlobalDefId, Vec<usize>> = HashMap::new();
    let mut edge_index = 0;
    for input in inputs {
        for instantiation in input.instantiations {
            let Some(source_def_id) = instantiation.source_def_id else {
                continue;
            };
            by_source.entry(source_def_id).or_default().push(edge_index);
            edge_index += 1;
        }
    }
    by_source
}

fn collect_source_instantiation_edges(
    inputs: &[MonomorphizeModuleInput<'_>],
) -> Vec<SourceInstantiationEdge> {
    let mut edges = Vec::new();
    for input in inputs {
        for instantiation in input.instantiations {
            if instantiation.source_def_id.is_none() {
                continue;
            }
            let source_def_id = instantiation
                .source_def_id
                .expect("source instantiation edge source checked above");
            edges.push(SourceInstantiationEdge {
                source_module_id: source_def_id.module_id,
                def_id: instantiation.def_id,
                args: instantiation.args.clone(),
                span: instantiation.span,
            });
        }
    }
    edges
}

fn collect_recorded_generics_by_def(
    inputs: &[MonomorphizeModuleInput<'_>],
) -> HashMap<GlobalDefId, Vec<String>> {
    let mut generics = HashMap::<GlobalDefId, Vec<String>>::new();
    for input in inputs {
        for instantiation in input.instantiations {
            if !instantiation.generics.is_empty() {
                generics
                    .entry(instantiation.def_id)
                    .or_insert_with(|| instantiation.generics.clone());
            }
        }
    }
    generics
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_ids::ConstExprId;
    use nia_parser::parse_module;
    use nia_sema_ir::GenericInstantiation;
    use nia_span::Span;
    use nia_ty::{ArrayLenTy, PrimitiveTy};

    fn normalization_for(interner: &TyInterner) -> TypeNormalization {
        TypeNormalization {
            interner: interner.clone(),
            normalized: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    fn mono_input<'a>(
        defs: &'a DefCollection,
        interner: &'a TyInterner,
        normalization: &'a TypeNormalization,
        comptime: &'a ComptimeCheck,
        const_expr_summaries: &'a HashMap<GlobalConstExprId, ConstExprSummary>,
        instantiations: &'a [GenericInstantiation],
    ) -> MonomorphizeModuleInput<'a> {
        MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs,
            interner,
            normalization,
            comptime,
            const_expr_summaries,
            layouts: None,
            local_enums: &EMPTY_LOCAL_ENUMS,
            program_enums: &EMPTY_PROGRAM_ENUMS,
            trait_impls: &[],
            instantiations,
        }
    }

    #[test]
    fn deduplicates_generic_instances() {
        let (module, errors) = parse_module("fn id[T](value: T) T { value }");
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let def_id = defs.module_scope.values.get("id").expect("id def");
        let interner = TyInterner::new(ModuleId(0));
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let instantiations = vec![
            GenericInstantiation {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id,
                },
                args: vec![i32_ty],
                generics: vec!["T".to_string()],
                span: Span::new(1, 2),
                source_def_id: None,
            },
            GenericInstantiation {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id,
                },
                args: vec![i32_ty],
                generics: vec!["T".to_string()],
                span: Span::new(3, 4),
                source_def_id: None,
            },
        ];

        let normalization = normalization_for(&interner);
        let comptime = ComptimeCheck::default();
        let const_exprs = HashMap::new();
        let mono = collect_monomorphizations(&[mono_input(
            &defs,
            &interner,
            &normalization,
            &comptime,
            &const_exprs,
            &instantiations,
        )]);

        assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
        assert_eq!(mono.instances.len(), 1);
    }

    #[test]
    fn generic_body_instantiations_are_expanded_from_concrete_roots_only() {
        let (module, errors) = parse_module(
            r#"
fn inner[T](value: T) T { value }
fn outer[T](value: T) T { inner[T](value) }
fn main() i32 { outer(1) }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let inner_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("inner").expect("inner def"),
        };
        let outer_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("outer").expect("outer def"),
        };
        let mut interner = TyInterner::new(ModuleId(0));
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let generic_t = interner.intern(TyKind::GenericParam("T".to_string()));
        let instantiations = vec![
            GenericInstantiation {
                def_id: inner_id,
                args: vec![generic_t],
                generics: vec!["T".to_string()],
                span: Span::new(1, 2),
                source_def_id: Some(outer_id),
            },
            GenericInstantiation {
                def_id: outer_id,
                args: vec![i32_ty],
                generics: vec!["T".to_string()],
                span: Span::new(3, 4),
                source_def_id: None,
            },
        ];

        let normalization = normalization_for(&interner);
        let comptime = ComptimeCheck::default();
        let const_exprs = HashMap::new();
        let mono = collect_monomorphizations(&[mono_input(
            &defs,
            &interner,
            &normalization,
            &comptime,
            &const_exprs,
            &instantiations,
        )]);

        assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
        assert_eq!(mono.instances.len(), 2);
        assert!(
            mono.instances
                .iter()
                .any(|instance| instance.def_id == outer_id && instance.args == vec![i32_ty])
        );
        assert!(
            mono.instances
                .iter()
                .any(|instance| instance.def_id == inner_id && instance.args == vec![i32_ty])
        );
        assert!(
            !mono
                .instances
                .iter()
                .any(|instance| instance.def_id == inner_id && instance.args == vec![generic_t])
        );
    }

    #[test]
    fn nested_generic_body_instantiations_reuse_working_interner() {
        let (module, errors) = parse_module(
            r#"
fn inner[T](value: T) T { value }
fn outer[T](value: &T) &T { inner[&T](value) }
fn main() i32 { 0 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let inner_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("inner").expect("inner def"),
        };
        let outer_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("outer").expect("outer def"),
        };
        let mut interner = TyInterner::new(ModuleId(0));
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let generic_t = interner.intern(TyKind::GenericParam("T".to_string()));
        let generic_ptr = interner.intern(TyKind::Pointer {
            is_readonly: true,
            elem: generic_t,
        });
        let i32_ptr = interner.intern(TyKind::Pointer {
            is_readonly: true,
            elem: i32_ty,
        });
        let instantiations = vec![
            GenericInstantiation {
                def_id: inner_id,
                args: vec![generic_ptr],
                generics: vec!["T".to_string()],
                span: Span::new(1, 2),
                source_def_id: Some(outer_id),
            },
            GenericInstantiation {
                def_id: outer_id,
                args: vec![i32_ty],
                generics: vec!["T".to_string()],
                span: Span::new(3, 4),
                source_def_id: None,
            },
        ];

        let normalization = normalization_for(&interner);
        let comptime = ComptimeCheck::default();
        let const_exprs = HashMap::new();
        let mono = collect_monomorphizations(&[mono_input(
            &defs,
            &interner,
            &normalization,
            &comptime,
            &const_exprs,
            &instantiations,
        )]);

        assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
        assert!(
            mono.instances
                .iter()
                .any(|instance| instance.def_id == inner_id && instance.args == vec![i32_ptr])
        );
        assert!(
            !mono
                .instances
                .iter()
                .any(|instance| instance.def_id == inner_id && instance.args == vec![generic_ptr])
        );
    }

    #[test]
    fn recursive_generic_body_reuses_same_concrete_instance() {
        let (module, errors) = parse_module("fn recurse[T](value: T) T { recurse[T](value) }");
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let recurse_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs
                .module_scope
                .values
                .get("recurse")
                .expect("recurse def"),
        };
        let mut interner = TyInterner::new(ModuleId(0));
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let generic_t = interner.intern(TyKind::GenericParam("T".to_string()));
        let instantiations = vec![
            GenericInstantiation {
                def_id: recurse_id,
                args: vec![generic_t],
                generics: vec!["T".to_string()],
                span: Span::new(1, 2),
                source_def_id: Some(recurse_id),
            },
            GenericInstantiation {
                def_id: recurse_id,
                args: vec![i32_ty],
                generics: vec!["T".to_string()],
                span: Span::new(3, 4),
                source_def_id: None,
            },
        ];

        let normalization = normalization_for(&interner);
        let comptime = ComptimeCheck::default();
        let const_exprs = HashMap::new();
        let mono = collect_monomorphizations(&[mono_input(
            &defs,
            &interner,
            &normalization,
            &comptime,
            &const_exprs,
            &instantiations,
        )]);

        assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
        assert_eq!(mono.instances.len(), 1);
        assert_eq!(mono.instances[0].def_id, recurse_id);
        assert_eq!(mono.instances[0].args, vec![i32_ty]);
    }

    #[test]
    fn growing_recursive_generic_body_reports_type_depth_limit() {
        let (module, errors) = parse_module("fn grow[T](value: &T) i32 { grow[&T](&value) }");
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let grow_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("grow").expect("grow def"),
        };
        let mut interner = TyInterner::new(ModuleId(0));
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let i32_ptr = interner.intern(TyKind::Pointer {
            is_readonly: true,
            elem: i32_ty,
        });
        let generic_t = interner.intern(TyKind::GenericParam("T".to_string()));
        let generic_ptr = interner.intern(TyKind::Pointer {
            is_readonly: true,
            elem: generic_t,
        });
        let instantiations = vec![
            GenericInstantiation {
                def_id: grow_id,
                args: vec![generic_ptr],
                generics: vec!["T".to_string()],
                span: Span::new(10, 20),
                source_def_id: Some(grow_id),
            },
            GenericInstantiation {
                def_id: grow_id,
                args: vec![i32_ty],
                generics: vec!["T".to_string()],
                span: Span::new(1, 2),
                source_def_id: None,
            },
        ];

        let normalization = normalization_for(&interner);
        let comptime = ComptimeCheck::default();
        let const_exprs = HashMap::new();
        let mono = collect_monomorphizations(&[mono_input(
            &defs,
            &interner,
            &normalization,
            &comptime,
            &const_exprs,
            &instantiations,
        )]);

        assert!(
            mono.instances
                .iter()
                .any(|instance| { instance.def_id == grow_id && instance.args == vec![i32_ptr] })
        );
        let diagnostic = mono
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.summary.contains("type depth limit"))
            .expect("type depth diagnostic");
        assert_eq!(diagnostic.code.as_str(), "E0601");
        assert_eq!(diagnostic.primary_span(), Some(Span::new(10, 20)));
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note.contains("already-seen concrete generic instance"))
        );
        assert!(
            diagnostic
                .help
                .iter()
                .any(|help| help.contains("finite set of concrete type arguments"))
        );
    }

    #[test]
    fn unresolved_array_lengths_in_symbols_are_diagnostic_not_panic() {
        let (module, errors) = parse_module("fn take[T](value: T) T { value }");
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let take_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("take").expect("take def"),
        };
        let mut interner = TyInterner::new(ModuleId(0));
        let len_id = GlobalConstExprId {
            module_id: ModuleId(0),
            const_expr_id: ConstExprId(0),
        };
        let elem = interner.primitive(PrimitiveTy::I32);
        let array_ty = interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstExpr(len_id),
            elem,
        });
        let instantiations = vec![GenericInstantiation {
            def_id: take_id,
            args: vec![array_ty],
            generics: vec!["T".to_string()],
            span: Span::new(1, 2),
            source_def_id: None,
        }];
        let mut const_expr_summaries = HashMap::new();
        const_expr_summaries.insert(
            len_id,
            ConstExprSummary {
                span: Span::new(10, 12),
                literal_array_len: None,
            },
        );

        let normalization = normalization_for(&interner);
        let comptime = ComptimeCheck::default();
        let mono = collect_monomorphizations(&[mono_input(
            &defs,
            &interner,
            &normalization,
            &comptime,
            &const_expr_summaries,
            &instantiations,
        )]);

        assert_eq!(mono.instances.len(), 1);
        assert!(
            mono.instances[0].symbol.contains("len_unresolved__m0__c0"),
            "{}",
            mono.instances[0].symbol
        );
        assert_eq!(mono.diagnostics.len(), 1);
        assert!(
            mono.diagnostics[0]
                .summary
                .contains("was not evaluated before monomorphization")
        );
        assert_eq!(mono.diagnostics[0].primary_span(), Some(Span::new(10, 12)));
    }

    #[test]
    fn repeated_unresolved_array_length_symbol_reuses_cached_diagnostic() {
        let (module, errors) = parse_module(
            r#"
fn take[T](value: T) T { value }
fn wrap[T](value: T) T { value }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let take_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("take").expect("take def"),
        };
        let wrap_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get("wrap").expect("wrap def"),
        };
        let mut interner = TyInterner::new(ModuleId(0));
        let len_id = GlobalConstExprId {
            module_id: ModuleId(0),
            const_expr_id: ConstExprId(0),
        };
        let elem = interner.primitive(PrimitiveTy::I32);
        let array_ty = interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstExpr(len_id),
            elem,
        });
        let instantiations = vec![
            GenericInstantiation {
                def_id: take_id,
                args: vec![array_ty],
                generics: vec!["T".to_string()],
                span: Span::new(1, 2),
                source_def_id: None,
            },
            GenericInstantiation {
                def_id: wrap_id,
                args: vec![array_ty],
                generics: vec!["T".to_string()],
                span: Span::new(3, 4),
                source_def_id: None,
            },
        ];

        let normalization = normalization_for(&interner);
        let comptime = ComptimeCheck::default();
        let const_exprs = HashMap::new();
        let mono = collect_monomorphizations(&[mono_input(
            &defs,
            &interner,
            &normalization,
            &comptime,
            &const_exprs,
            &instantiations,
        )]);

        assert_eq!(mono.instances.len(), 2);
        assert_eq!(mono.diagnostics.len(), 1);
    }

    #[test]
    fn effective_generics_cache_uses_recorded_generics_by_reference() {
        let def_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: nia_ids::DefId(0),
        };
        let mut collector = empty_collector();
        collector
            .recorded_generics_by_def
            .insert(def_id, vec!["T".to_string(), "U".to_string()]);

        assert_eq!(
            collector.effective_generics_for(def_id),
            &["T".to_string(), "U".to_string()]
        );
        collector.recorded_generics_by_def.clear();
        assert_eq!(
            collector.effective_generics_for(def_id),
            &["T".to_string(), "U".to_string()]
        );
    }

    #[test]
    fn ordered_type_substitutions_reuse_existing_ids() {
        let mut collector = empty_collector();
        let i32_ty = InternedTyId::new(
            TyInternerId::for_module(ModuleId(0)),
            nia_ids::TyInternerIndex::from_interner_index(1),
        );
        let bool_ty = InternedTyId::new(
            TyInternerId::for_module(ModuleId(0)),
            nia_ids::TyInternerIndex::from_interner_index(2),
        );

        let first = collector.intern_ordered_type_substitutions(vec![
            ("T".to_string(), i32_ty),
            ("U".to_string(), bool_ty),
        ]);
        let second = collector.intern_ordered_type_substitutions(vec![
            ("T".to_string(), i32_ty),
            ("U".to_string(), bool_ty),
        ]);

        assert_eq!(first, second);
        assert_eq!(collector.type_substitutions.len(), 1);
        assert_eq!(
            collector.type_substitutions[first.0],
            HashMap::from([("T".to_string(), i32_ty), ("U".to_string(), bool_ty)])
        );
    }

    fn empty_collector() -> MonoCollector<'static> {
        MonoCollector {
            defs_by_module: HashMap::new(),
            interners_by_module: HashMap::new(),
            normalizations_by_module: HashMap::new(),
            comptime_by_module: HashMap::new(),
            const_expr_summaries_by_module: HashMap::new(),
            working_interners_by_module: HashMap::new(),
            layouts_by_module: HashMap::new(),
            local_enums_by_module: HashMap::new(),
            program_enums: &EMPTY_PROGRAM_ENUMS,
            trait_impls: &[],
            instantiations_by_source: HashMap::new(),
            source_instantiation_edges: Vec::new(),
            recorded_generics_by_def: HashMap::new(),
            instances: Vec::new(),
            seen: HashSet::new(),
            type_symbols: HashMap::new(),
            def_names: HashMap::new(),
            base_symbols: HashMap::new(),
            type_instantiations: HashMap::new(),
            type_substitutions: Vec::new(),
            type_substitution_ids: HashMap::new(),
            effective_generics: HashMap::new(),
            missing_array_len_diagnostics: HashSet::new(),
            diagnostics: Vec::new(),
        }
    }
}
