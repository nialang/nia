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
        instantiations_by_source: collect_instantiations_by_source(inputs),
        recorded_generics_by_def: collect_recorded_generics_by_def(inputs),
        instances: Vec::new(),
        seen: HashSet::new(),
        expanded: HashSet::new(),
        type_symbols: HashMap::new(),
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
    instantiations_by_source: HashMap<GlobalDefId, Vec<(ModuleId, GenericInstantiation)>>,
    recorded_generics_by_def: HashMap<GlobalDefId, Vec<Vec<String>>>,
    instances: Vec<MonoInstance>,
    seen: HashSet<MonoInstanceKey>,
    expanded: HashSet<MonoInstanceKey>,
    type_symbols: HashMap<(ModuleId, InternedTyId), String>,
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

        let Some(edges) = self.instantiations_by_source.get(&key.def_id).cloned() else {
            return;
        };
        let substitutions = self.generic_substitutions_for_instance(&key);
        stack.push(key.clone());
        for (edge_module_id, edge) in edges {
            if !self.is_generic_def(edge.def_id) {
                continue;
            }
            let args = self.instantiate_args(edge_module_id, &edge.args, &substitutions);
            let edge_key = MonoInstanceKey {
                def_id: edge.def_id,
                arg_module_id: edge_module_id,
                args,
            };
            self.add_instance(edge_key.clone());
            self.expand_instance(edge_key, edge.span, stack);
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
        &self,
        module_id: ModuleId,
        args: &[InternedTyId],
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<InternedTyId> {
        args.iter()
            .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
            .collect()
    }

    fn instantiate_ty(
        &self,
        module_id: ModuleId,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> InternedTyId {
        let Some(interner) = self.interners_by_module.get(&module_id) else {
            return ty;
        };
        match interner.get(ty) {
            Some(TyKind::GenericParam(name)) => substitutions.get(name).copied().unwrap_or(ty),
            Some(TyKind::Pointer { is_const, elem }) => {
                let elem = self.instantiate_ty(module_id, *elem, substitutions);
                let mut interner = (*interner).clone();
                interner.intern(TyKind::Pointer {
                    is_const: *is_const,
                    elem,
                })
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let elem = self.instantiate_ty(module_id, *elem, substitutions);
                let mut interner = (*interner).clone();
                interner.intern(TyKind::Slice {
                    is_const: *is_const,
                    elem,
                })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.instantiate_ty(module_id, *elem, substitutions);
                let mut interner = (*interner).clone();
                interner.intern(TyKind::Array {
                    len: len.clone(),
                    elem,
                })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .iter()
                    .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
                    .collect();
                let mut interner = (*interner).clone();
                interner.intern(TyKind::Nominal {
                    def_id: *def_id,
                    args,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.instantiate_ty(module_id, *self_ty, substitutions);
                let trait_args = trait_args
                    .iter()
                    .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
                    .collect();
                let mut interner = (*interner).clone();
                interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id: *trait_id,
                    trait_args,
                    name: name.clone(),
                })
            }
            _ => ty,
        }
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
        let Some(interner) = self.interners_by_module.get(&module_id) else {
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
) -> HashMap<GlobalDefId, Vec<(ModuleId, GenericInstantiation)>> {
    let mut by_source: HashMap<GlobalDefId, Vec<(ModuleId, GenericInstantiation)>> = HashMap::new();
    for input in inputs {
        for instantiation in input.instantiations {
            let Some(source_def_id) = instantiation.source_def_id else {
                continue;
            };
            by_source
                .entry(source_def_id)
                .or_default()
                .push((input.module_id, instantiation.clone()));
        }
    }
    by_source
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
            recorded_generics_by_def: HashMap::from([(
                def_id,
                vec![Vec::new(), vec!["T".to_string(), "U".to_string()]],
            )]),
            instances: Vec::new(),
            seen: HashSet::new(),
            expanded: HashSet::new(),
            type_symbols: HashMap::new(),
            effective_generics: HashMap::new(),
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
