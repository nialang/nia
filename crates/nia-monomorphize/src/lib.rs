// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_body_check::GenericInstantiation;
use nia_defs::{DefCollection, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, ModuleId, TyId};
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
    pub args: Vec<TyId>,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonomorphizeModuleInput<'a> {
    pub module_id: ModuleId,
    pub defs: &'a DefCollection,
    pub interner: &'a TyInterner,
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
        instantiations_by_source: collect_instantiations_by_source(inputs),
        recorded_generics: collect_recorded_generics(inputs),
        instances: Vec::new(),
        seen: HashSet::new(),
        expanded: HashSet::new(),
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
    instantiations_by_source: HashMap<GlobalDefId, Vec<(ModuleId, GenericInstantiation)>>,
    recorded_generics: HashMap<GlobalDefId, Vec<String>>,
    instances: Vec<MonoInstance>,
    seen: HashSet<MonoInstanceKey>,
    expanded: HashSet<MonoInstanceKey>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MonoInstanceKey {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    args: Vec<TyId>,
}

impl MonoCollector<'_> {
    fn collect_module(&mut self, input: &MonomorphizeModuleInput<'_>) {
        for instantiation in input.instantiations {
            if !self.is_generic_def(instantiation.def_id) {
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

    fn is_generic_def(&self, def_id: GlobalDefId) -> bool {
        if self
            .all_recorded_generics(def_id)
            .iter()
            .any(|generics| !generics.is_empty())
        {
            return true;
        }
        let Some(defs) = self.defs_by_module.get(&def_id.module_id) else {
            return false;
        };
        defs.defs.get(def_id.def_id).is_some_and(|def| {
            if !matches!(def.kind, DefKind::Function | DefKind::Method) {
                return false;
            }
            !self.effective_generics(defs, def_id).is_empty()
        })
    }

    fn effective_generics(&self, defs: &DefCollection, def_id: GlobalDefId) -> Vec<String> {
        let Some(def) = defs.defs.get(def_id.def_id) else {
            return Vec::new();
        };
        let mut generics = def
            .parent
            .and_then(|parent| defs.defs.get(parent))
            .map(|parent| parent.generics.clone())
            .unwrap_or_default();
        generics.extend(def.generics.clone());
        generics
    }

    fn effective_generics_for(&self, def_id: GlobalDefId) -> Vec<String> {
        if let Some(generics) = self
            .all_recorded_generics(def_id)
            .into_iter()
            .find(|generics| !generics.is_empty())
        {
            return generics;
        }
        self.defs_by_module
            .get(&def_id.module_id)
            .map(|defs| self.effective_generics(defs, def_id))
            .unwrap_or_default()
    }

    fn all_recorded_generics(&self, def_id: GlobalDefId) -> Vec<Vec<String>> {
        self.recorded_generics
            .get(&def_id)
            .cloned()
            .into_iter()
            .chain(self.instantiations_by_source.values().flatten().filter_map(
                |(_, instantiation)| {
                    (instantiation.def_id == def_id).then_some(instantiation.generics.clone())
                },
            ))
            .collect()
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
            self.instances.push(MonoInstance {
                def_id: key.def_id,
                arg_module_id: key.arg_module_id,
                args: key.args.clone(),
                symbol: self.instance_symbol(&key),
            });
        }
    }

    fn generic_substitutions_for_instance(&self, key: &MonoInstanceKey) -> HashMap<String, TyId> {
        self.effective_generics_for(key.def_id)
            .iter()
            .cloned()
            .zip(key.args.iter().copied())
            .collect()
    }

    fn instantiate_args(
        &self,
        module_id: ModuleId,
        args: &[TyId],
        substitutions: &HashMap<String, TyId>,
    ) -> Vec<TyId> {
        args.iter()
            .map(|arg| self.instantiate_ty(module_id, *arg, substitutions))
            .collect()
    }

    fn instantiate_ty(
        &self,
        module_id: ModuleId,
        ty: TyId,
        substitutions: &HashMap<String, TyId>,
    ) -> TyId {
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
            _ => ty,
        }
    }

    fn instance_symbol(&self, key: &MonoInstanceKey) -> String {
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

    fn instance_name(&self, key: &MonoInstanceKey) -> String {
        let args = key
            .args
            .iter()
            .map(|arg| self.type_symbol(key.arg_module_id, *arg))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}[{args}]", self.def_name(key.def_id))
    }

    fn def_name(&self, def_id: GlobalDefId) -> String {
        self.defs_by_module
            .get(&def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .map(|def| sanitize_symbol_part(&def.name))
            .unwrap_or_else(|| format!("def{}", def_id.def_id.0))
    }

    fn type_symbol(&self, module_id: ModuleId, ty: TyId) -> String {
        let Some(interner) = self.interners_by_module.get(&module_id) else {
            return format!("ty{}", ty.0);
        };
        match interner.get(ty) {
            Some(TyKind::Primitive(_)) => {
                mangle_type_with(interner, ty, |def_id| self.def_name(def_id))
            }
            Some(TyKind::Pointer { is_const, elem }) => {
                let qualifier = if *is_const { "const_ptr" } else { "ptr" };
                format!("{qualifier}_{}", self.type_symbol(module_id, *elem))
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let qualifier = if *is_const { "const_slice" } else { "slice" };
                format!("{qualifier}_{}", self.type_symbol(module_id, *elem))
            }
            Some(TyKind::Array { elem, .. }) => {
                format!("array_{}", self.type_symbol(module_id, *elem))
            }
            Some(TyKind::FunctionPointer { .. }) => "fnptr".to_string(),
            Some(TyKind::Nominal { def_id, args }) => {
                let name = self.def_name(*def_id);
                if args.is_empty() {
                    name
                } else {
                    let args = args
                        .iter()
                        .map(|arg| self.type_symbol(module_id, *arg))
                        .collect::<Vec<_>>()
                        .join("_");
                    format!("{name}_{args}")
                }
            }
            Some(TyKind::GenericParam(name)) => sanitize_symbol_part(name),
            Some(TyKind::Error) | None => format!("ty{}", ty.0),
        }
    }
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

fn collect_recorded_generics(
    inputs: &[MonomorphizeModuleInput<'_>],
) -> HashMap<GlobalDefId, Vec<String>> {
    let mut generics = HashMap::new();
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
    use nia_body_check::GenericInstantiation;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_parser::parse_module;
    use nia_span::Span;
    use nia_ty::PrimitiveTy;

    #[test]
    fn deduplicates_generic_instances() {
        let (module, errors) = parse_module("fn id[T](value: T) T { value }");
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let def_id = defs.module_scope.values.get("id").expect("id def");
        let interner = TyInterner::new();
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
            instantiations: &instantiations,
        }]);

        assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
        assert_eq!(mono.instances.len(), 1);
    }
}
