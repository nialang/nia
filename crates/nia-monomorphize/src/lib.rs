// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use nia_const_check::ConstCheck;
use nia_defs::{DefCollection, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{DefId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::{
    EnumSignature, ProgramEnumSignature, ProgramTraitImplIndex, ProgramTraitImplSignature,
};
use nia_layout::Layouts;
use nia_mangle::{
    MangleModuleId, MangleResolvers, mangle_base_symbol_id, mangle_symbol_id, mangle_type_with,
};
use nia_sema_ir::GenericInstantiation;
use nia_source::SourceIdentity;
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap};
use nia_trait_solve::TraitSolverContext;
#[cfg(test)]
use nia_ty::TypeStoreAppend;
use nia_ty::{
    AssociatedTypeBindingTy, ConstExprSummary, ConstGenericArg, ConstGenericValue, TyKind,
    TypeStore,
};
use nia_type_normalize::TypeNormalization;

#[derive(Debug, Clone, PartialEq)]
pub struct Monomorphization {
    pub instances: Vec<MonoInstance>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoInstance {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub self_arg: Option<InternedTyId>,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
    pub symbol: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonomorphizeModuleInput<'a> {
    pub module_id: ModuleId,
    pub source_identity: SourceIdentity,
    pub defs: &'a DefCollection,
    pub normalization: &'a TypeNormalization,
    pub const_eval: &'a ConstCheck,
    pub const_expr_summaries: &'a HashMap<GlobalConstExprId, ConstExprSummary>,
    pub layouts: Option<&'a Layouts>,
    pub local_enums: &'a HashMap<nia_ids::DefId, EnumSignature>,
    pub program_enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub trait_impl_index: &'a ProgramTraitImplIndex,
    pub instantiations: &'a [GenericInstantiation],
}

pub fn collect_monomorphizations(
    inputs: &[MonomorphizeModuleInput<'_>],
    source_identities: impl IntoIterator<Item = (ModuleId, SourceIdentity)>,
    type_store: &TypeStore,
) -> Monomorphization {
    let empty_trait_impl_index = ProgramTraitImplIndex::default();
    let mut collector = MonoCollector {
        type_store,
        source_identities: source_identities.into_iter().collect(),
        defs_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.defs))
            .collect(),
        normalizations_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.normalization))
            .collect(),
        const_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.const_eval))
            .collect(),
        const_expr_summaries_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.const_expr_summaries))
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
        trait_impl_index: inputs
            .first()
            .map(|input| input.trait_impl_index)
            .unwrap_or(&empty_trait_impl_index),
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
        diagnostics: collector.diagnostics,
    }
}

struct MonoCollector<'a> {
    type_store: &'a TypeStore,
    source_identities: HashMap<ModuleId, SourceIdentity>,
    defs_by_module: HashMap<ModuleId, &'a DefCollection>,
    normalizations_by_module: HashMap<ModuleId, &'a TypeNormalization>,
    const_by_module: HashMap<ModuleId, &'a ConstCheck>,
    const_expr_summaries_by_module:
        HashMap<ModuleId, &'a HashMap<GlobalConstExprId, ConstExprSummary>>,
    layouts_by_module: HashMap<ModuleId, &'a Layouts>,
    local_enums_by_module: HashMap<ModuleId, &'a HashMap<nia_ids::DefId, EnumSignature>>,
    program_enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    trait_impls: &'a [ProgramTraitImplSignature],
    trait_impl_index: &'a ProgramTraitImplIndex,
    instantiations_by_source: HashMap<GlobalDefId, Vec<usize>>,
    source_instantiation_edges: Vec<SourceInstantiationEdge>,
    recorded_generics_by_def: HashMap<GlobalDefId, Vec<SymbolId>>,
    instances: Vec<MonoInstance>,
    seen: HashSet<MonoInstanceKey>,
    type_symbols: HashMap<(ModuleId, InternedTyId), String>,
    def_names: HashMap<GlobalDefId, SymbolId>,
    base_symbols: HashMap<GlobalDefId, String>,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    type_substitutions: Vec<TypeSubstitution>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
    effective_generics: HashMap<GlobalDefId, Vec<SymbolId>>,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    diagnostics: Vec<Diagnostic>,
}

static EMPTY_PROGRAM_ENUMS: std::sync::LazyLock<HashMap<GlobalDefId, ProgramEnumSignature>> =
    std::sync::LazyLock::new(HashMap::new);
static EMPTY_LOCAL_ENUMS: std::sync::LazyLock<HashMap<DefId, EnumSignature>> =
    std::sync::LazyLock::new(HashMap::new);
#[cfg(test)]
static EMPTY_PROGRAM_TRAIT_IMPL_INDEX: std::sync::LazyLock<ProgramTraitImplIndex> =
    std::sync::LazyLock::new(ProgramTraitImplIndex::default);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MonoInstanceKey {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    self_arg: Option<InternedTyId>,
    args: Vec<InternedTyId>,
    const_args: Vec<ConstGenericArg>,
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
    trait_const_args: Vec<ConstGenericArg>,
    name: SymbolId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TypeSubstitutionId(usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TypeSubstitutionKey {
    self_arg: Option<InternedTyId>,
    substitutions: Vec<(SymbolId, InternedTyId)>,
}

#[derive(Debug, Clone, Default)]
struct TypeSubstitution {
    self_arg: Option<InternedTyId>,
    substitutions: SymbolMap<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceInstantiationEdge {
    source_module_id: ModuleId,
    def_id: GlobalDefId,
    self_arg: Option<InternedTyId>,
    args: Vec<InternedTyId>,
    const_args: Vec<ConstGenericArg>,
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
    fn collect_module(&mut self, input: &MonomorphizeModuleInput<'_>) {
        let mut pending = VecDeque::new();
        for instantiation in input.instantiations {
            if !self.is_generic_def(instantiation.def_id) {
                continue;
            }
            if instantiation
                .source_def_id
                .is_some_and(|source_def_id| self.is_generic_def(source_def_id))
                && self.args_contain_generic_param(&instantiation.args)
            {
                continue;
            }
            let key = MonoInstanceKey {
                def_id: instantiation.def_id,
                arg_module_id: input.module_id,
                self_arg: instantiation.self_arg,
                args: instantiation.args.clone(),
                const_args: instantiation.const_args.clone(),
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

    fn compute_effective_generics(&self, def_id: GlobalDefId) -> Vec<SymbolId> {
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
            let mut generics = def
                .parent
                .and_then(|parent| defs.defs.get(parent))
                .map(|parent| parent.generics.clone())
                .unwrap_or_default();
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

    fn effective_generics_for(&mut self, def_id: GlobalDefId) -> &[SymbolId] {
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

    fn args_contain_generic_param(&self, args: &[InternedTyId]) -> bool {
        args.iter()
            .copied()
            .any(|arg| self.ty_contains_generic_param(arg))
    }

    fn ty_contains_generic_param(&self, ty: InternedTyId) -> bool {
        let Some(kind) = self.type_kind(ty) else {
            return false;
        };
        match kind {
            TyKind::GenericParam(_) | TyKind::SelfParam => true,
            TyKind::Tuple(elems) => elems
                .iter()
                .any(|elem| self.ty_contains_generic_param(*elem)),
            TyKind::ClosureState {
                captures,
                params,
                return_type,
                ..
            } => {
                captures
                    .iter()
                    .chain(params.iter())
                    .any(|ty| self.ty_contains_generic_param(*ty))
                    || self.ty_contains_generic_param(return_type)
            }
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem } => self.ty_contains_generic_param(elem),
            TyKind::Array { elem, .. } => self.ty_contains_generic_param(elem),
            TyKind::Range { bound, .. } => {
                bound.is_some_and(|bound| self.ty_contains_generic_param(bound))
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                params
                    .iter()
                    .any(|param| self.ty_contains_generic_param(*param))
                    || self.ty_contains_generic_param(return_type)
            }
            TyKind::Optional { elem } => self.ty_contains_generic_param(elem),
            TyKind::ErrorUnion { error, value } => {
                self.ty_contains_generic_param(error) || self.ty_contains_generic_param(value)
            }
            TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. } => {
                args.iter().any(|arg| self.ty_contains_generic_param(*arg))
            }
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
                    .any(|arg| self.ty_contains_generic_param(*arg))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.ty_contains_generic_param(*arg))
                            || self.ty_contains_generic_param(binding.ty)
                    })
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                self.ty_contains_generic_param(self_ty)
                    || trait_args
                        .iter()
                        .any(|arg| self.ty_contains_generic_param(*arg))
            }
            TyKind::Primitive(_)
            | TyKind::Opaque
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. }
            | TyKind::ConstOnly
            | TyKind::Error => false,
        }
    }

    fn recorded_generics(&self, def_id: GlobalDefId) -> Option<&[SymbolId]> {
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
            let self_arg = pending_instance.key.self_arg;
            let substitution_id = self.intern_ordered_type_substitutions(self_arg, substitutions);
            for edge_index in edge_indices {
                let Some(edge) = self.source_instantiation_edges.get(edge_index) else {
                    continue;
                };
                let source_module_id = edge.source_module_id;
                let edge_def_id = edge.def_id;
                let edge_span = edge.span;
                let edge_self_arg = edge.self_arg;
                let edge_args = edge.args.clone();
                let edge_const_args = edge.const_args.clone();
                if !self.is_generic_def(edge_def_id) {
                    continue;
                }
                let self_arg = edge_self_arg.map(|self_arg| {
                    self.instantiate_ty(source_module_id, self_arg, substitution_id)
                });
                let args = self.instantiate_args(source_module_id, &edge_args, substitution_id);
                let edge_key = MonoInstanceKey {
                    def_id: edge_def_id,
                    arg_module_id: source_module_id,
                    self_arg,
                    args,
                    const_args: edge_const_args,
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
        if key
            .self_arg
            .is_some_and(|self_arg| self.ty_contains_generic_param(self_arg))
            || self.args_contain_generic_param(&key.args)
        {
            return;
        }
        if self.instances.len() >= MAX_MONOMORPHIZED_INSTANCES {
            self.report_instance_limit(span, &key);
            return;
        }
        if key.self_arg.is_some_and(|self_arg| {
            self.ty_exceeds_instance_depth(self_arg, MAX_MONOMORPHIZED_INSTANCE_TYPE_DEPTH)
        }) || key
            .args
            .iter()
            .any(|arg| self.ty_exceeds_instance_depth(*arg, MAX_MONOMORPHIZED_INSTANCE_TYPE_DEPTH))
        {
            self.report_instance_type_depth_limit(span, &key);
            return;
        }
        let symbol = self.instance_symbol(&key);
        self.instances.push(MonoInstance {
            def_id: key.def_id,
            arg_module_id: key.arg_module_id,
            self_arg: key.self_arg,
            args: key.args.clone(),
            const_args: key.const_args.clone(),
            symbol,
            span,
        });
        pending.push_back(PendingMonoInstance { key, span });
    }

    fn report_instance_limit(&mut self, span: Span, key: &MonoInstanceKey) {
        let name = mangle_symbol_id(self.def_name(key.def_id));
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
        let name = mangle_symbol_id(self.def_name(key.def_id));
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

    fn ty_exceeds_instance_depth(&self, ty: InternedTyId, remaining: usize) -> bool {
        if remaining == 0 {
            return true;
        }
        let Some(kind) = self.type_kind(ty) else {
            return false;
        };
        let next = remaining - 1;
        match kind {
            TyKind::Tuple(elems) => elems
                .iter()
                .any(|elem| self.ty_exceeds_instance_depth(*elem, next)),
            TyKind::ClosureState {
                captures,
                params,
                return_type,
                ..
            } => {
                captures
                    .iter()
                    .chain(params.iter())
                    .any(|ty| self.ty_exceeds_instance_depth(*ty, next))
                    || self.ty_exceeds_instance_depth(return_type, next)
            }
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem } => self.ty_exceeds_instance_depth(elem, next),
            TyKind::Array { elem, .. } => self.ty_exceeds_instance_depth(elem, next),
            TyKind::Range { bound, .. } => {
                bound.is_some_and(|bound| self.ty_exceeds_instance_depth(bound, next))
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                params
                    .iter()
                    .any(|param| self.ty_exceeds_instance_depth(*param, next))
                    || self.ty_exceeds_instance_depth(return_type, next)
            }
            TyKind::Optional { elem } => self.ty_exceeds_instance_depth(elem, next),
            TyKind::ErrorUnion { error, value } => {
                self.ty_exceeds_instance_depth(error, next)
                    || self.ty_exceeds_instance_depth(value, next)
            }
            TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. } => args
                .iter()
                .any(|arg| self.ty_exceeds_instance_depth(*arg, next)),
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
                    .any(|arg| self.ty_exceeds_instance_depth(*arg, next))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.ty_exceeds_instance_depth(*arg, next))
                            || self.ty_exceeds_instance_depth(binding.ty, next)
                    })
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                self.ty_exceeds_instance_depth(self_ty, next)
                    || trait_args
                        .iter()
                        .any(|arg| self.ty_exceeds_instance_depth(*arg, next))
            }
            TyKind::GenericParam(_)
            | TyKind::SelfParam
            | TyKind::Opaque
            | TyKind::Primitive(_)
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. }
            | TyKind::ConstOnly
            | TyKind::Error => false,
        }
    }

    fn generic_substitutions_for_instance(
        &mut self,
        key: &MonoInstanceKey,
    ) -> Vec<(SymbolId, InternedTyId)> {
        let generics = self.effective_generics_for(key.def_id).to_vec();
        generics.into_iter().zip(key.args.iter().copied()).collect()
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
        let Some(kind) = self.type_kind(ty) else {
            return ty;
        };
        let instantiated = match kind {
            TyKind::GenericParam(name) => self
                .type_substitutions
                .get(substitutions.0)
                .and_then(|substitutions| substitutions.substitutions.get(&name))
                .copied()
                .unwrap_or(ty),
            TyKind::SelfParam => self
                .type_substitutions
                .get(substitutions.0)
                .and_then(|substitutions| substitutions.self_arg)
                .unwrap_or(ty),
            TyKind::Opaque | TyKind::BuiltinType(_) => ty,
            TyKind::Tuple(elems) => {
                let elems = elems
                    .iter()
                    .map(|elem| {
                        self.instantiate_ty_inner(
                            module_id,
                            *elem,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                self.intern_working_ty(module_id, TyKind::Tuple(elems))
            }
            TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            } => {
                let captures = captures
                    .iter()
                    .map(|ty| {
                        self.instantiate_ty_inner(module_id, *ty, substitutions, active_projections)
                    })
                    .collect();
                let params = params
                    .iter()
                    .map(|ty| {
                        self.instantiate_ty_inner(module_id, *ty, substitutions, active_projections)
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
                    TyKind::ClosureState {
                        closure_id,
                        captures,
                        params,
                        return_type,
                    },
                )
            }
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
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
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
                let const_args = const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.instantiate_ty_inner(
                            module_id,
                            arg.ty,
                            substitutions,
                            active_projections,
                        );
                        arg
                    })
                    .collect();
                self.intern_working_ty(
                    module_id,
                    TyKind::Nominal {
                        def_id,
                        args,
                        const_args,
                    },
                )
            }
            TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
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
                let trait_const_args: Vec<ConstGenericArg> = trait_const_args
                    .iter()
                    .map(|arg| {
                        let mut arg = arg.clone();
                        arg.ty = self.instantiate_ty_inner(
                            module_id,
                            arg.ty,
                            substitutions,
                            active_projections,
                        );
                        arg
                    })
                    .collect();
                let projection_key = ProjectionInstantiationKey {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                    name,
                };
                let projection = self.intern_working_ty(
                    module_id,
                    TyKind::Projection {
                        self_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        trait_const_args: trait_const_args.clone(),
                        name,
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
                            &trait_const_args,
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
                trait_const_args,
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
                let trait_const_args = trait_const_args
                    .iter()
                    .map(|arg| {
                        let mut arg = arg.clone();
                        arg.ty = self.instantiate_ty_inner(
                            module_id,
                            arg.ty,
                            substitutions,
                            active_projections,
                        );
                        arg
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
                        trait_const_args: binding
                            .trait_const_args
                            .iter()
                            .map(|arg| {
                                let mut arg = arg.clone();
                                arg.ty = self.instantiate_ty_inner(
                                    module_id,
                                    arg.ty,
                                    substitutions,
                                    active_projections,
                                );
                                arg
                            })
                            .collect(),
                        name: binding.name,
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
                        trait_const_args,
                        associated_type_bindings,
                        is_readonly,
                    },
                )
            }
            TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
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
                let trait_const_args = trait_const_args
                    .iter()
                    .map(|arg| {
                        let mut arg = arg.clone();
                        arg.ty = self.instantiate_ty_inner(
                            module_id,
                            arg.ty,
                            substitutions,
                            active_projections,
                        );
                        arg
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
                        trait_const_args: binding
                            .trait_const_args
                            .iter()
                            .map(|arg| {
                                let mut arg = arg.clone();
                                arg.ty = self.instantiate_ty_inner(
                                    module_id,
                                    arg.ty,
                                    substitutions,
                                    active_projections,
                                );
                                arg
                            })
                            .collect(),
                        name: binding.name,
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
                        trait_const_args,
                        associated_type_bindings,
                    },
                )
            }
            TyKind::Primitive(_) | TyKind::Vector { .. } | TyKind::ConstOnly | TyKind::Error => ty,
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
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let normalization = *self.normalizations_by_module.get(&module_id)?;
        let local_enums = self
            .local_enums_by_module
            .get(&module_id)
            .copied()
            .unwrap_or(&EMPTY_LOCAL_ENUMS);
        let layouts = self.layouts_by_module.get(&module_id).copied();
        let program_is_enum = |def_id| self.program_enums.contains_key(&def_id);
        let context = TraitSolverContext {
            type_store: self.type_store,
            normalization,
            trait_impls: self.trait_impls,
            trait_impl_index: Some(self.trait_impl_index),
            layouts,
            local_module_id: module_id,
            local_enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver_with_associated_type_assumptions(&[], &[]);
        solver.resolve_associated_type(self_ty, trait_id, trait_args, trait_const_args, name)
    }

    fn intern_working_ty(&mut self, module_id: ModuleId, kind: TyKind) -> InternedTyId {
        self.type_store.append_for_module(module_id).intern(kind)
    }

    fn type_kind(&self, ty: InternedTyId) -> Option<TyKind> {
        self.type_store.get(ty).cloned()
    }

    fn intern_ordered_type_substitutions(
        &mut self,
        self_arg: Option<InternedTyId>,
        substitutions: Vec<(SymbolId, InternedTyId)>,
    ) -> TypeSubstitutionId {
        self.intern_type_substitution_key(TypeSubstitutionKey {
            self_arg,
            substitutions,
        })
    }

    fn intern_type_substitution_key(&mut self, key: TypeSubstitutionKey) -> TypeSubstitutionId {
        if let Some(id) = self.type_substitution_ids.get(&key) {
            return *id;
        }
        let id = TypeSubstitutionId(self.type_substitutions.len());
        self.type_substitutions.push(TypeSubstitution {
            self_arg: key.self_arg,
            substitutions: key.substitutions.iter().cloned().collect(),
        });
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
        let self_arg = key
            .self_arg
            .map(|self_arg| format!("self_{}", self.type_symbol(key.arg_module_id, self_arg)));
        let const_args = key
            .const_args
            .iter()
            .map(|arg| self.const_arg_symbol(key.arg_module_id, arg))
            .collect::<Vec<_>>()
            .join("_");
        let base_symbol = self.base_symbol(key.def_id);
        let mut parts = Vec::new();
        if let Some(self_arg) = self_arg {
            parts.push(self_arg);
        }
        if !args.is_empty() {
            parts.push(args);
        }
        if !const_args.is_empty() {
            parts.push(const_args);
        }
        if parts.is_empty() {
            base_symbol
        } else {
            format!("{base_symbol}__inst__{}", parts.join("_"))
        }
    }

    fn const_arg_symbol(&mut self, module_id: ModuleId, arg: &ConstGenericArg) -> String {
        let ty = self.type_symbol(module_id, arg.ty);
        let value = match &arg.value {
            ConstGenericValue::GenericParam(name) => {
                format!("g{}", mangle_symbol_id(*name))
            }
            ConstGenericValue::ConstExpr(id) => {
                format!(
                    "expr__s{:016x}__c{}",
                    self.module_mangle_id(id.module_id).raw(),
                    id.const_expr_id.0
                )
            }
            ConstGenericValue::Int(value) => {
                let sign = if value.is_signed() { "i" } else { "u" };
                format!("{sign}{}", value.bits())
            }
            ConstGenericValue::Bool(value) => format!("b{}", u8::from(*value)),
            ConstGenericValue::Char(value) => format!("c{}", *value as u32),
        };
        format!("c_{ty}_{value}")
    }

    fn base_symbol(&mut self, def_id: GlobalDefId) -> String {
        if let Some(symbol) = self.base_symbols.get(&def_id) {
            return symbol.clone();
        }
        let name = self.def_name(def_id);
        let symbol = mangle_base_symbol_id(def_id, self.module_mangle_id(def_id.module_id), name);
        self.base_symbols.insert(def_id, symbol.clone());
        symbol
    }

    fn def_name(&mut self, def_id: GlobalDefId) -> SymbolId {
        if let Some(name) = self.def_names.get(&def_id) {
            return *name;
        }
        let name = def_name(&self.defs_by_module, def_id);
        self.def_names.insert(def_id, name);
        name
    }

    fn type_symbol(&mut self, module_id: ModuleId, ty: InternedTyId) -> String {
        if let Some(symbol) = self.type_symbols.get(&(module_id, ty)) {
            return symbol.clone();
        }
        let defs_by_module = &self.defs_by_module;
        let def_names = &mut self.def_names;
        let const_by_module = &self.const_by_module;
        let const_expr_summaries_by_module = &self.const_expr_summaries_by_module;
        let missing_array_len_diagnostics = &mut self.missing_array_len_diagnostics;
        let diagnostics = &mut self.diagnostics;
        let source_identities = &self.source_identities;
        let symbol = mangle_type_with(
            self.type_store,
            ty,
            MangleResolvers::new(
                |module_id| module_mangle_id(source_identities, module_id),
                |def_id| cached_def_name(defs_by_module, def_names, def_id),
                |id| {
                    array_len(
                        const_by_module,
                        const_expr_summaries_by_module,
                        missing_array_len_diagnostics,
                        diagnostics,
                        id,
                    )
                },
            ),
        );
        self.type_symbols.insert((module_id, ty), symbol.clone());
        symbol
    }

    fn module_mangle_id(&self, module_id: ModuleId) -> MangleModuleId {
        module_mangle_id(&self.source_identities, module_id)
    }
}

fn module_mangle_id(
    source_identities: &HashMap<ModuleId, SourceIdentity>,
    module_id: ModuleId,
) -> MangleModuleId {
    let source_identity = source_identities.get(&module_id).unwrap_or_else(|| {
        panic!("Nia ICE: missing source identity for monomorphized module {module_id:?}")
    });
    MangleModuleId::from_normalized_source_path(source_identity.normalized_path())
}

fn array_len(
    const_by_module: &HashMap<ModuleId, &ConstCheck>,
    const_expr_summaries_by_module: &HashMap<
        ModuleId,
        &HashMap<GlobalConstExprId, ConstExprSummary>,
    >,
    missing_array_len_diagnostics: &mut HashSet<GlobalConstExprId>,
    diagnostics: &mut Vec<Diagnostic>,
    id: GlobalConstExprId,
) -> Option<u64> {
    let value = const_by_module
        .get(&id.module_id)
        .and_then(|const_eval| const_eval.array_lengths.get(&id).copied());
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

fn def_name(defs_by_module: &HashMap<ModuleId, &DefCollection>, def_id: GlobalDefId) -> SymbolId {
    defs_by_module
        .get(&def_id.module_id)
        .and_then(|defs| defs.defs.get(def_id.def_id))
        .map(|def| def.name)
        .unwrap_or_else(|| SymbolId::from_stable_hash(def_id.def_id.0))
}

fn cached_def_name(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    def_names: &mut HashMap<GlobalDefId, SymbolId>,
    def_id: GlobalDefId,
) -> String {
    if let Some(name) = def_names.get(&def_id) {
        return mangle_symbol_id(*name);
    }
    let name = def_name(defs_by_module, def_id);
    def_names.insert(def_id, name);
    mangle_symbol_id(name)
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
                self_arg: instantiation.self_arg,
                args: instantiation.args.clone(),
                const_args: instantiation.const_args.clone(),
                span: instantiation.span,
            });
        }
    }
    edges
}

fn collect_recorded_generics_by_def(
    inputs: &[MonomorphizeModuleInput<'_>],
) -> HashMap<GlobalDefId, Vec<SymbolId>> {
    let mut generics = HashMap::<GlobalDefId, Vec<SymbolId>>::new();
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
    use nia_ids::{ConstExprId, ModuleIdAllocator};
    use nia_parser::parse_module;
    use nia_sema_ir::GenericInstantiation;
    use nia_span::Span;
    use nia_symbol::stable_hash;
    use nia_ty::{ArrayLenTy, PrimitiveTy};

    include!("tests/monomorphize/test_support.rs");

    #[path = "monomorphize/instance_expansion.rs"]
    mod instance_expansion;

    #[path = "monomorphize/recursive_instances.rs"]
    mod recursive_instances;

    #[path = "monomorphize/array_length_symbols.rs"]
    mod array_length_symbols;

    #[path = "monomorphize/collector_caches.rs"]
    mod collector_caches;
}
