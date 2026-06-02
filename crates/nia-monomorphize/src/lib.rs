// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::Expr;
use nia_body_ir::GenericInstantiation;
use nia_comptime_check::ComptimeCheck;
use nia_defs::{DefCollection, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_mangle::{mangle_base_symbol, mangle_type_with, sanitize_symbol_part};
use nia_span::Span;
use nia_ty::{TyInterner, TyKind};

#[derive(Debug, Clone, PartialEq)]
pub struct Monomorphization {
    pub instances: Vec<MonoInstance>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoInstance {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonomorphizeModuleInput<'a> {
    pub module_id: ModuleId,
    pub defs: &'a DefCollection,
    pub interner: &'a TyInterner,
    pub comptime: &'a ComptimeCheck,
    pub const_exprs: &'a HashMap<GlobalConstExprId, Expr>,
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
        comptime_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.comptime))
            .collect(),
        const_exprs_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.const_exprs))
            .collect(),
        working_interners_by_module: inputs
            .iter()
            .map(|input| (input.module_id, input.interner.clone()))
            .collect(),
        instantiations_by_source: collect_instantiations_by_source(inputs),
        source_instantiation_edges: collect_source_instantiation_edges(inputs),
        recorded_generics_by_def: collect_recorded_generics_by_def(inputs),
        instances: Vec::new(),
        seen: HashSet::new(),
        expanded: HashSet::new(),
        type_symbols: HashMap::new(),
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
    defs_by_module: HashMap<ModuleId, &'a DefCollection>,
    interners_by_module: HashMap<ModuleId, &'a TyInterner>,
    comptime_by_module: HashMap<ModuleId, &'a ComptimeCheck>,
    const_exprs_by_module: HashMap<ModuleId, &'a HashMap<GlobalConstExprId, Expr>>,
    working_interners_by_module: HashMap<ModuleId, TyInterner>,
    instantiations_by_source: HashMap<GlobalDefId, Vec<usize>>,
    source_instantiation_edges: Vec<SourceInstantiationEdge>,
    recorded_generics_by_def: HashMap<GlobalDefId, Vec<Vec<String>>>,
    instances: Vec<MonoInstance>,
    seen: HashSet<MonoInstanceKey>,
    expanded: HashSet<MonoInstanceKey>,
    type_symbols: HashMap<(ModuleId, InternedTyId), String>,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    type_substitutions: Vec<HashMap<String, InternedTyId>>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
    effective_generics: HashMap<GlobalDefId, Vec<String>>,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    diagnostics: Vec<Diagnostic>,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TypeSubstitutionId(usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TypeSubstitutionKey {
    substitutions: Vec<(String, InternedTyId)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceInstantiationEdge {
    module_id: ModuleId,
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    span: Span,
}

impl MonoCollector<'_> {
    fn collect_module(&mut self, input: &MonomorphizeModuleInput<'_>) {
        for instantiation in input.instantiations {
            if !self.is_generic_def(instantiation.def_id) {
                continue;
            }
            if instantiation
                .source_def_id
                .is_some_and(|source_def_id| self.is_generic_def(source_def_id))
            {
                continue;
            }
            let key = MonoInstanceKey {
                def_id: instantiation.def_id,
                arg_module_id: input.module_id,
                args: instantiation.args.clone(),
            };
            self.add_instance(key.clone());
            self.expand_instance(key, instantiation.span, &mut Vec::new());
        }
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
        if let Some(generics) = self.first_non_empty_recorded_generics(def_id) {
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
        self.recorded_generics_by_def
            .get(&def_id)
            .is_some_and(|all| all.iter().any(|generics| !generics.is_empty()))
    }

    fn first_non_empty_recorded_generics(&self, def_id: GlobalDefId) -> Option<&[String]> {
        self.recorded_generics_by_def
            .get(&def_id)
            .and_then(|all| all.iter().find(|generics| !generics.is_empty()))
            .map(Vec::as_slice)
    }

    fn expand_instance(
        &mut self,
        key: MonoInstanceKey,
        span: Span,
        stack: &mut Vec<MonoInstanceKey>,
    ) {
        if let Some(index) = stack.iter().position(|entry| entry == &key) {
            let cycle = stack[index..]
                .iter()
                .chain([&key])
                .map(|entry| self.instance_name(entry))
                .collect::<Vec<_>>()
                .join(" -> ");
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("recursive generic instantiation detected: {cycle}"),
            ));
            return;
        }
        if !self.expanded.insert(key.clone()) {
            return;
        }

        let Some(edge_indices) = self.instantiations_by_source.get(&key.def_id).cloned() else {
            return;
        };
        let substitutions = self.generic_substitutions_for_instance(&key);
        stack.push(key.clone());
        for edge_index in edge_indices {
            let Some(edge) = self.source_instantiation_edges.get(edge_index) else {
                continue;
            };
            let edge_module_id = edge.module_id;
            let edge_def_id = edge.def_id;
            let edge_span = edge.span;
            if !self.is_generic_def(edge_def_id) {
                continue;
            }
            let Some(edge) = self.source_instantiation_edges.get(edge_index) else {
                continue;
            };
            let edge_args = edge.args.clone();
            let args = self.instantiate_args(edge_module_id, &edge_args, &substitutions);
            let edge_key = MonoInstanceKey {
                def_id: edge_def_id,
                arg_module_id: edge_module_id,
                args,
            };
            self.add_instance(edge_key.clone());
            self.expand_instance(edge_key, edge_span, stack);
        }
        stack.pop();
    }

    fn add_instance(&mut self, key: MonoInstanceKey) {
        if self.seen.insert(key.clone()) {
            let symbol = self.instance_symbol(&key);
            self.instances.push(MonoInstance {
                def_id: key.def_id,
                arg_module_id: key.arg_module_id,
                args: key.args.clone(),
                symbol,
            });
        }
    }

    fn generic_substitutions_for_instance(
        &mut self,
        key: &MonoInstanceKey,
    ) -> HashMap<String, InternedTyId> {
        self.effective_generics_for(key.def_id)
            .iter()
            .cloned()
            .zip(key.args.iter().copied())
            .collect()
    }

    fn instantiate_args(
        &mut self,
        module_id: ModuleId,
        args: &[InternedTyId],
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<InternedTyId> {
        let substitutions = self.intern_type_substitutions(substitutions);
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
        let key = TypeInstantiationKey {
            module_id,
            ty,
            substitutions,
        };
        if let Some(cached) = self.type_instantiations.get(&key).copied() {
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
            TyKind::Pointer { is_const, elem } => {
                let elem = self.instantiate_ty(module_id, elem, substitutions);
                self.intern_working_ty(module_id, TyKind::Pointer { is_const, elem })
            }
            TyKind::Slice { is_const, elem } => {
                let elem = self.instantiate_ty(module_id, elem, substitutions);
                self.intern_working_ty(module_id, TyKind::Slice { is_const, elem })
            }
            TyKind::Array { len, elem } => {
                let elem = self.instantiate_ty(module_id, elem, substitutions);
                self.intern_working_ty(module_id, TyKind::Array { len, elem })
            }
            TyKind::Nominal { def_id, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
                    .collect();
                self.intern_working_ty(module_id, TyKind::Nominal { def_id, args })
            }
            TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            } => {
                let self_ty = self.instantiate_ty(module_id, self_ty, substitutions);
                let trait_args = trait_args
                    .iter()
                    .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
                    .collect();
                self.intern_working_ty(
                    module_id,
                    TyKind::Projection {
                        self_ty,
                        trait_id,
                        trait_args,
                        name,
                    },
                )
            }
            TyKind::Range { kind, bound } => {
                let bound = bound.map(|bound| self.instantiate_ty(module_id, bound, substitutions));
                self.intern_working_ty(module_id, TyKind::Range { kind, bound })
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                let params = params
                    .iter()
                    .map(|param| self.instantiate_ty(module_id, *param, substitutions))
                    .collect();
                let return_type = self.instantiate_ty(module_id, return_type, substitutions);
                self.intern_working_ty(
                    module_id,
                    TyKind::FunctionPointer {
                        params,
                        return_type,
                        is_variadic,
                    },
                )
            }
            TyKind::BuiltinTrait { trait_id, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
                    .collect();
                self.intern_working_ty(module_id, TyKind::BuiltinTrait { trait_id, args })
            }
            TyKind::TraitObject {
                trait_id,
                trait_args,
                associated_type_bindings,
                is_const,
            } => {
                let trait_args = trait_args
                    .iter()
                    .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .iter()
                    .map(|(name, ty)| {
                        (
                            name.clone(),
                            self.instantiate_ty(module_id, *ty, substitutions),
                        )
                    })
                    .collect();
                self.intern_working_ty(
                    module_id,
                    TyKind::TraitObject {
                        trait_id,
                        trait_args,
                        associated_type_bindings,
                        is_const,
                    },
                )
            }
            TyKind::Primitive(_) | TyKind::Error => ty,
        };
        self.type_instantiations.insert(key, instantiated);
        instantiated
    }

    fn intern_working_ty(&mut self, module_id: ModuleId, kind: TyKind) -> InternedTyId {
        if let Some(interner) = self.working_interners_by_module.get_mut(&module_id) {
            return interner.intern(kind);
        }
        let Some(interner) = self.interners_by_module.get(&module_id).cloned() else {
            return InternedTyId::new(module_id, nia_ids::TyInternerIndex::from_interner_index(0));
        };
        let mut interner = interner.clone();
        let ty = interner.intern(kind);
        self.working_interners_by_module.insert(module_id, interner);
        ty
    }

    fn intern_type_substitutions(
        &mut self,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> TypeSubstitutionId {
        let mut key = TypeSubstitutionKey {
            substitutions: substitutions
                .iter()
                .map(|(name, ty)| (name.clone(), *ty))
                .collect(),
        };
        key.substitutions.sort_by(|left, right| {
            left.0.cmp(&right.0).then_with(|| {
                (left.1.interner_id, left.1.index.index())
                    .cmp(&(right.1.interner_id, right.1.index.index()))
            })
        });
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
        let name = self.def_name(key.def_id);
        let args = key
            .args
            .iter()
            .map(|arg| self.type_symbol(key.arg_module_id, *arg))
            .collect::<Vec<_>>()
            .join("_");
        if args.is_empty() {
            mangle_base_symbol(key.def_id, &name)
        } else {
            format!("{}__inst__{}", mangle_base_symbol(key.def_id, &name), args)
        }
    }

    fn instance_name(&mut self, key: &MonoInstanceKey) -> String {
        let args = key
            .args
            .iter()
            .map(|arg| self.type_symbol(key.arg_module_id, *arg))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}[{args}]", self.def_name(key.def_id))
    }

    fn def_name(&self, def_id: GlobalDefId) -> String {
        def_name(&self.defs_by_module, def_id)
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
            return format!("m{}_ty{}", ty.interner_id.0, ty.index.index());
        };
        if interner.get(ty).is_none() {
            return format!("m{}_ty{}", ty.interner_id.0, ty.index.index());
        }
        let defs_by_module = &self.defs_by_module;
        let comptime_by_module = &self.comptime_by_module;
        let const_exprs_by_module = &self.const_exprs_by_module;
        let missing_array_len_diagnostics = &mut self.missing_array_len_diagnostics;
        let diagnostics = &mut self.diagnostics;
        let symbol = mangle_type_with(
            interner,
            ty,
            |def_id| def_name(defs_by_module, def_id),
            |id| {
                array_len(
                    comptime_by_module,
                    const_exprs_by_module,
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
    const_exprs_by_module: &HashMap<ModuleId, &HashMap<GlobalConstExprId, Expr>>,
    missing_array_len_diagnostics: &mut HashSet<GlobalConstExprId>,
    diagnostics: &mut Vec<Diagnostic>,
    id: GlobalConstExprId,
) -> Option<u64> {
    let value = comptime_by_module
        .get(&id.module_id)
        .and_then(|comptime| comptime.array_lengths.get(&id).copied());
    if value.is_none() && missing_array_len_diagnostics.insert(id) {
        let span = const_exprs_by_module
            .get(&id.module_id)
            .and_then(|const_exprs| const_exprs.get(&id))
            .map(|expr| expr.span)
            .unwrap_or_default();
        diagnostics.push(Diagnostic::error(
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
            edges.push(SourceInstantiationEdge {
                module_id: input.module_id,
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
) -> HashMap<GlobalDefId, Vec<Vec<String>>> {
    let mut generics = HashMap::<GlobalDefId, Vec<Vec<String>>>::new();
    for input in inputs {
        for instantiation in input.instantiations {
            if !instantiation.generics.is_empty() {
                generics
                    .entry(instantiation.def_id)
                    .or_default()
                    .push(instantiation.generics.clone());
            }
        }
    }
    generics
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_body_ir::GenericInstantiation;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_ids::ConstExprId;
    use nia_parser::parse_module;
    use nia_span::Span;
    use nia_ty::{ArrayLenTy, PrimitiveTy};

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

        let mono = collect_monomorphizations(&[MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &interner,
            comptime: &ComptimeCheck::default(),
            const_exprs: &HashMap::new(),
            instantiations: &instantiations,
        }]);

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

        let mono = collect_monomorphizations(&[MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &interner,
            comptime: &ComptimeCheck::default(),
            const_exprs: &HashMap::new(),
            instantiations: &instantiations,
        }]);

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
fn outer[T](value: &const T) &const T { inner[&const T](value) }
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
            is_const: true,
            elem: generic_t,
        });
        let i32_ptr = interner.intern(TyKind::Pointer {
            is_const: true,
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

        let mono = collect_monomorphizations(&[MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &interner,
            comptime: &ComptimeCheck::default(),
            const_exprs: &HashMap::new(),
            instantiations: &instantiations,
        }]);

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
        let mut const_exprs = HashMap::new();
        const_exprs.insert(
            len_id,
            nia_ast::Expr {
                span: Span::new(10, 12),
                kind: nia_ast::ExprKind::Integer("N".to_string()),
            },
        );

        let mono = collect_monomorphizations(&[MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &interner,
            comptime: &ComptimeCheck::default(),
            const_exprs: &const_exprs,
            instantiations: &instantiations,
        }]);

        assert_eq!(mono.instances.len(), 1);
        assert!(
            mono.instances[0].symbol.contains("len_unresolved__m0__c0"),
            "{}",
            mono.instances[0].symbol
        );
        assert_eq!(mono.diagnostics.len(), 1);
        assert!(
            mono.diagnostics[0]
                .message
                .contains("was not evaluated before monomorphization")
        );
        assert_eq!(mono.diagnostics[0].span, Span::new(10, 12));
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

        let mono = collect_monomorphizations(&[MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &interner,
            comptime: &ComptimeCheck::default(),
            const_exprs: &HashMap::new(),
            instantiations: &instantiations,
        }]);

        assert_eq!(mono.instances.len(), 2);
        assert_eq!(mono.diagnostics.len(), 1);
    }

    #[test]
    fn effective_generics_cache_uses_recorded_generics_by_reference() {
        let def_id = GlobalDefId {
            module_id: ModuleId(0),
            def_id: nia_ids::DefId(0),
        };
        let mut collector = MonoCollector {
            defs_by_module: HashMap::new(),
            interners_by_module: HashMap::new(),
            comptime_by_module: HashMap::new(),
            const_exprs_by_module: HashMap::new(),
            instantiations_by_source: HashMap::new(),
            source_instantiation_edges: Vec::new(),
            recorded_generics_by_def: HashMap::from([(
                def_id,
                vec![Vec::new(), vec!["T".to_string(), "U".to_string()]],
            )]),
            instances: Vec::new(),
            seen: HashSet::new(),
            expanded: HashSet::new(),
            type_symbols: HashMap::new(),
            type_instantiations: HashMap::new(),
            type_substitutions: Vec::new(),
            type_substitution_ids: HashMap::new(),
            effective_generics: HashMap::new(),
            working_interners_by_module: HashMap::new(),
            missing_array_len_diagnostics: HashSet::new(),
            diagnostics: Vec::new(),
        };

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
}
