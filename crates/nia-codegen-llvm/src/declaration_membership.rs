// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{BTreeSet, HashSet, VecDeque};

use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendGlobal, BackendGlobalInstance,
    BackendGlobalInstanceKey, BackendModuleOwnerDirectory, BackendStructInstanceKey,
    BackendTraitObjectVtable, BackendTraitObjectVtableFunction, BackendTraitObjectVtableKey,
    CodegenPartition, CodegenUnitDependencies, CodegenUnitId, CodegenUnitPendingModules,
};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    DynamicTraitCallRef, FunctionBodyRefs, FunctionInstanceKey, TraitObjectVtableRef,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_mangle::{MangleModuleId, MangleResolvers, mangle_symbol_id, mangle_type_with};
use nia_span::Span;
use nia_ty::{ArrayLenTy, ConstGenericArg, ConstGenericValue, TraitId, TyKind};

use crate::program_index::ProgramIndex;

#[derive(Debug)]
pub(super) struct CodegenDeclarationMembership {
    pub(super) dependencies: CodegenUnitDependencies,
    pub(super) structs: Vec<GlobalDefId>,
    pub(super) struct_instances: Vec<BackendStructInstanceKey>,
    pub(super) unions: Vec<GlobalDefId>,
    pub(super) union_instances: Vec<BackendStructInstanceKey>,
    pub(super) functions: Vec<GlobalDefId>,
    pub(super) function_instances: Vec<FunctionInstanceKey>,
    pub(super) globals: Vec<GlobalDefId>,
    pub(super) global_instances: Vec<BackendGlobalInstanceKey>,
    pub(super) vtables: Vec<BackendTraitObjectVtableKey>,
}

pub(super) enum CodegenDeclarationMembershipBuild {
    Ready(Box<CodegenDeclarationMembership>),
    Pending(CodegenUnitPendingModules),
    Invalid { diagnostics: Vec<Diagnostic> },
}

impl CodegenDeclarationMembership {
    pub(super) fn build(
        partition: &CodegenPartition,
        index: &ProgramIndex,
        owners: &BackendModuleOwnerDirectory,
    ) -> CodegenDeclarationMembershipBuild {
        MembershipBuilder::new(index, owners).build(partition)
    }

    pub(super) fn validate_dependencies(&self, partition: &CodegenPartition, index: &ProgramIndex) {
        assert_eq!(
            self.dependencies.unit(),
            partition.id,
            "Nia ICE: codegen dependency closure belongs to a different unit"
        );
        let owner = index.module_for_partition(partition);
        assert!(
            self.dependencies.contains(owner.id),
            "Nia ICE: codegen dependency closure omits its definition owner"
        );
        for &module_id in self.dependencies.modules() {
            assert!(
                index.module(module_id).is_some(),
                "Nia ICE: codegen dependency module {module_id:?} is not published"
            );
        }
    }
}

struct MembershipBuilder<'a> {
    index: &'a ProgramIndex,
    owners: &'a BackendModuleOwnerDirectory,
    structs: BTreeSet<GlobalDefId>,
    struct_instances: HashSet<BackendStructInstanceKey>,
    unions: BTreeSet<GlobalDefId>,
    union_instances: HashSet<BackendStructInstanceKey>,
    functions: BTreeSet<GlobalDefId>,
    function_instances: HashSet<FunctionInstanceKey>,
    globals: BTreeSet<GlobalDefId>,
    global_instances: HashSet<BackendGlobalInstanceKey>,
    vtables: HashSet<BackendTraitObjectVtableKey>,
    expanded_vtables: HashSet<BackendTraitObjectVtableKey>,
    pending_types: VecDeque<InternedTyId>,
    visited_types: HashSet<InternedTyId>,
    dependency_modules: BTreeSet<ModuleId>,
    pending_modules: BTreeSet<ModuleId>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> MembershipBuilder<'a> {
    fn new(index: &'a ProgramIndex, owners: &'a BackendModuleOwnerDirectory) -> Self {
        Self {
            index,
            owners,
            structs: BTreeSet::new(),
            struct_instances: HashSet::new(),
            unions: BTreeSet::new(),
            union_instances: HashSet::new(),
            functions: BTreeSet::new(),
            function_instances: HashSet::new(),
            globals: BTreeSet::new(),
            global_instances: HashSet::new(),
            vtables: HashSet::new(),
            expanded_vtables: HashSet::new(),
            pending_types: VecDeque::new(),
            visited_types: HashSet::new(),
            dependency_modules: BTreeSet::new(),
            pending_modules: BTreeSet::new(),
            diagnostics: Vec::new(),
        }
    }

    fn build(mut self, partition: &CodegenPartition) -> CodegenDeclarationMembershipBuild {
        let owner = self.index.module_for_partition(partition);
        self.dependency_modules.insert(owner.id);
        for &index in partition.global_definitions() {
            self.add_global_definition(&owner.globals[index]);
        }
        for &index in partition.global_instance_definitions() {
            self.add_global_instance_definition(&owner.global_instances[index]);
        }
        for &index in partition.function_definitions() {
            self.add_function_definition(&owner.functions[index]);
        }
        for &index in partition.function_instance_definitions() {
            self.add_function_instance_definition(&owner.function_instances[index]);
        }
        for &index in partition.closure_entry_definitions() {
            self.add_closure_entry_definition(&owner.closure_entries[index]);
        }
        for &index in partition.vtable_definitions() {
            self.add_vtable_definition(&owner.trait_object_vtables[index]);
        }
        self.close_types();
        self.finish(partition.id)
    }

    fn add_global_definition(&mut self, item: &BackendGlobal) {
        self.add_global(item);
        if let Some(init) = &item.init {
            self.add_refs(init.value_refs(item.def_id.module_id));
        }
    }

    fn add_global_instance_definition(&mut self, item: &BackendGlobalInstance) {
        self.add_global_instance(item);
        if let Some(init) = &item.init {
            self.add_refs(init.value_refs(item.arg_module_id));
        }
    }

    fn add_function_definition(&mut self, item: &BackendFunction) {
        self.add_function(item);
        if let Some(body) = &item.function_body {
            self.add_refs(body.value_refs(self.index.type_store()));
        }
    }

    fn add_function_instance_definition(&mut self, item: &BackendFunctionInstance) {
        self.add_function_instance(item);
        if let Some(body) = &item.function_body {
            self.add_refs(body.value_refs(self.index.type_store()));
        }
    }

    fn add_closure_entry_definition(&mut self, item: &nia_backend_ir::BackendClosureEntry) {
        self.add_type(item.abi.state_type);
        self.add_type(item.abi.state_pointer_type);
        self.add_types(item.abi.params.iter().copied());
        self.add_type(item.abi.return_type);
        self.add_refs(item.function_body.value_refs(self.index.type_store()));
    }

    fn add_global(&mut self, item: &BackendGlobal) {
        if self.globals.insert(item.def_id) {
            self.add_dependency(self.index.global_owner(item.def_id), "global");
            self.add_type(item.ty);
        }
    }

    fn add_global_instance(&mut self, item: &BackendGlobalInstance) {
        let key = BackendGlobalInstanceKey {
            def_id: item.def_id,
            arg_module_id: item.arg_module_id,
            args: item.args.clone(),
            const_args: item.const_args.clone(),
        };
        if self.global_instances.insert(key) {
            self.add_context_dependency(item.arg_module_id);
            self.add_dependency(
                self.index.global_instance_owner(
                    item.def_id,
                    item.arg_module_id,
                    &item.args,
                    &item.const_args,
                ),
                "global instance",
            );
            self.add_type(item.ty);
            self.add_types(item.args.iter().copied());
            self.add_types(item.const_args.iter().map(|arg| arg.ty));
            self.add_const_arg_dependencies(&item.const_args);
        }
    }

    fn add_function(&mut self, item: &BackendFunction) {
        if self.functions.insert(item.def_id) {
            self.add_dependency(self.index.function_owner(item.def_id), "function");
            self.add_function_signature(&item.params, item.return_type);
        }
    }

    fn add_function_instance(&mut self, item: &BackendFunctionInstance) {
        let key = FunctionInstanceKey {
            def_id: item.def_id,
            arg_module_id: item.arg_module_id,
            self_arg: item.self_arg,
            args: item.args.clone(),
            const_args: item.const_args.clone(),
        };
        if self.function_instances.insert(key) {
            self.add_context_dependency(item.arg_module_id);
            self.add_dependency(
                self.index.function_instance_owner(
                    item.def_id,
                    item.arg_module_id,
                    item.self_arg,
                    &item.args,
                    &item.const_args,
                ),
                "function instance",
            );
            self.add_function_signature(&item.params, item.return_type);
            self.add_types(item.self_arg);
            self.add_types(item.args.iter().copied());
            self.add_types(item.const_args.iter().map(|arg| arg.ty));
            self.add_const_arg_dependencies(&item.const_args);
        }
    }

    fn add_function_signature(
        &mut self,
        params: &[nia_backend_ir::BackendParam],
        return_type: InternedTyId,
    ) {
        self.add_types(params.iter().map(|param| param.passing_ty));
        self.add_type(return_type);
    }

    fn add_refs(&mut self, refs: FunctionBodyRefs) {
        if refs.invalid_ir {
            self.invalid("declaration closure encountered invalid function IR".to_string());
        }
        for module_id in refs.modules {
            self.add_context_dependency(module_id);
        }
        for def_id in refs.functions {
            if let Some(item) = self.index.function(def_id) {
                self.add_function(item);
            } else {
                self.wait_for_owner(self.owners.item_owner(def_id), "function");
            }
        }
        for def_id in refs.globals {
            if let Some(item) = self.index.global(def_id) {
                self.add_global(item);
            } else {
                self.wait_for_owner(self.owners.item_owner(def_id), "global");
            }
        }
        for reference in refs.function_instances {
            let key = reference.key();
            self.add_const_arg_dependencies(&reference.const_args);
            if let Some(item) = self.index.function_instance(
                reference.def_id,
                reference.arg_module_id,
                reference.self_arg,
                &reference.args,
                &reference.const_args,
            ) {
                self.add_function_instance(item);
            } else {
                self.wait_for_owner(
                    self.owners.function_instance_owner(&key),
                    "function instance",
                );
            }
        }
        for reference in refs.global_instances {
            let key = BackendGlobalInstanceKey {
                def_id: reference.def_id,
                arg_module_id: reference.arg_module_id,
                args: reference.args.clone(),
                const_args: reference.const_args.clone(),
            };
            self.add_const_arg_dependencies(&reference.const_args);
            if let Some(item) = self.index.global_instance(
                reference.def_id,
                reference.arg_module_id,
                &reference.args,
                &reference.const_args,
            ) {
                self.add_global_instance(item);
            } else {
                self.wait_for_owner(self.owners.global_instance_owner(&key), "global instance");
            }
        }
        self.add_types(refs.types);
        for reference in refs.trait_object_vtables {
            self.add_vtable_reference(reference);
        }
        for call in refs.dynamic_trait_calls {
            self.add_dynamic_trait_object_vtables(call);
        }
    }

    fn add_vtable_reference(&mut self, reference: TraitObjectVtableRef) {
        let key = BackendTraitObjectVtableKey {
            self_ty: reference.self_ty,
            object_ty: reference.object_ty,
        };
        if let Some(item) = self.index.trait_object_vtable(&key) {
            self.add_vtable(item);
        } else {
            self.wait_for_owner(self.owners.vtable_owner(&key), "trait-object vtable");
        }
    }

    fn add_dynamic_trait_object_vtables(&mut self, call: DynamicTraitCallRef) {
        // A dynamic call contains the erased object type but no concrete
        // `self` type. The owner directory is the planning-time authority for
        // every possible table, including tables whose modules have not been
        // published to the incremental index yet.
        let keys = self
            .owners
            .vtable_keys_for_object_ty(call.object_ty)
            .chain(self.owners.vtable_keys_for_trait(call.trait_id))
            .cloned()
            .collect::<HashSet<_>>();
        for key in keys {
            if let Some(item) = self.index.trait_object_vtable(&key) {
                self.add_vtable(item);
            } else {
                self.wait_for_owner(self.owners.vtable_owner(&key), "trait-object vtable");
            }
        }
    }

    fn add_vtable(&mut self, item: &BackendTraitObjectVtable) {
        if self.vtables.insert(item.key.clone()) {
            self.add_dependency(
                self.index.trait_object_vtable_owner(&item.key),
                "trait-object vtable",
            );
            self.add_types([item.key.self_ty, item.key.object_ty]);
            self.add_types(item.trait_args.iter().copied());
            self.add_types(item.trait_const_args.iter().map(|arg| arg.ty));
            self.add_const_arg_dependencies(&item.trait_const_args);
        }
        if !self.expanded_vtables.insert(item.key.clone()) {
            return;
        }
        // A dynamic call needs only the vtable symbol at LLVM runtime, but IR
        // validation also resolves the selected slot's target signature. Keep
        // those declarations and owner modules in the same readiness closure
        // for referenced tables as well as locally defined tables.
        for entry in &item.entries {
            self.add_types(entry.trait_args.iter().copied());
            self.add_types(entry.trait_const_args.iter().map(|arg| arg.ty));
            self.add_const_arg_dependencies(&entry.trait_const_args);
            match &entry.function {
                BackendTraitObjectVtableFunction::Function(def_id) => {
                    if let Some(function) = self.index.function(*def_id) {
                        self.add_function(function);
                    } else {
                        self.wait_for_owner(self.owners.item_owner(*def_id), "vtable function");
                    }
                }
                BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                } => {
                    let key = FunctionInstanceKey {
                        def_id: *def_id,
                        arg_module_id: *arg_module_id,
                        self_arg: *self_arg,
                        args: args.clone(),
                        const_args: const_args.clone(),
                    };
                    self.add_const_arg_dependencies(const_args);
                    if let Some(function) = self.index.function_instance(
                        *def_id,
                        *arg_module_id,
                        *self_arg,
                        args,
                        const_args,
                    ) {
                        self.add_function_instance(function);
                    } else {
                        self.wait_for_owner(
                            self.owners.function_instance_owner(&key),
                            "vtable function instance",
                        );
                    }
                }
            }
        }
    }

    fn add_vtable_definition(&mut self, item: &BackendTraitObjectVtable) {
        self.add_vtable(item);
    }

    fn add_type(&mut self, ty: InternedTyId) {
        if self.visited_types.insert(ty) {
            self.pending_types.push_back(ty);
        }
    }

    fn add_dependency(&mut self, owner: Option<ModuleId>, item: &str) {
        let Some(owner) = owner else {
            self.invalid(format!(
                "declaration closure references missing {item} owner"
            ));
            return;
        };
        self.dependency_modules.insert(owner);
    }

    fn add_context_dependency(&mut self, module_id: ModuleId) {
        self.dependency_modules.insert(module_id);
        if !self.index.is_published(module_id) {
            self.pending_modules.insert(module_id);
        }
    }

    fn wait_for_owner(&mut self, owner: Option<ModuleId>, item: &str) {
        let Some(owner) = owner else {
            self.invalid(format!(
                "declaration closure references missing {item} owner"
            ));
            return;
        };
        self.dependency_modules.insert(owner);
        if self.index.is_published(owner) {
            self.invalid(format!(
                "declaration closure references missing {item} in published module {owner:?}"
            ));
            return;
        }
        self.pending_modules.insert(owner);
    }

    fn add_trait_dependency(&mut self, trait_id: TraitId) {
        if let TraitId::Source(def_id) = trait_id {
            self.dependency_modules.insert(def_id.module_id);
        }
    }

    fn add_const_arg_dependencies(&mut self, args: &[ConstGenericArg]) {
        for arg in args {
            if let ConstGenericValue::ConstExpr(expr_id) = arg.value {
                self.dependency_modules.insert(expr_id.module_id);
            }
        }
    }

    fn add_type_owner_dependencies(&mut self, kind: &TyKind) {
        match kind {
            TyKind::Array {
                len: ArrayLenTy::ConstExpr(expr_id),
                ..
            } => {
                self.dependency_modules.insert(expr_id.module_id);
            }
            TyKind::Nominal {
                def_id, const_args, ..
            } => {
                self.dependency_modules.insert(def_id.module_id);
                self.add_const_arg_dependencies(const_args);
            }
            TyKind::TraitObject {
                trait_id,
                trait_const_args,
                associated_type_bindings,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_id,
                trait_const_args,
                associated_type_bindings,
                ..
            } => {
                self.add_trait_dependency(*trait_id);
                self.add_const_arg_dependencies(trait_const_args);
                for binding in associated_type_bindings {
                    if let Some(trait_id) = binding.trait_id {
                        self.add_trait_dependency(trait_id);
                    }
                    self.add_const_arg_dependencies(&binding.trait_const_args);
                }
            }
            TyKind::Projection {
                trait_id,
                trait_const_args,
                ..
            } => {
                self.add_trait_dependency(*trait_id);
                self.add_const_arg_dependencies(trait_const_args);
            }
            _ => {}
        }
    }

    fn add_types(&mut self, types: impl IntoIterator<Item = InternedTyId>) {
        for ty in types {
            self.add_type(ty);
        }
    }

    fn close_types(&mut self) {
        while let Some(ty) = self.pending_types.pop_front() {
            let Some(kind) = self.index.ty_kind(ty) else {
                self.invalid(format!(
                    "declaration closure references missing type {ty:?}"
                ));
                continue;
            };
            self.add_type_owner_dependencies(kind);
            kind.visit_referenced_types(|referenced| self.add_type(referenced));
            let TyKind::Nominal {
                def_id,
                args,
                const_args,
            } = kind
            else {
                continue;
            };
            let key = BackendStructInstanceKey {
                def_id: *def_id,
                args: args.clone(),
                const_args: const_args.clone(),
            };
            if let Some(item) = self.index.struct_instance(*def_id, args, const_args) {
                self.add_dependency(
                    self.index.struct_instance_owner(*def_id, args, const_args),
                    "struct instance",
                );
                if self.struct_instances.insert(key) {
                    self.add_types(item.fields.iter().map(|field| field.ty));
                }
            } else if let Some(item) = self.index.union_instance(*def_id, args, const_args) {
                self.add_dependency(
                    self.index.union_instance_owner(*def_id, args, const_args),
                    "union instance",
                );
                if self.union_instances.insert(key) {
                    self.add_types(item.fields.iter().map(|field| field.ty));
                }
            } else if let Some(item) = self.index.struct_item(*def_id) {
                self.add_dependency(self.index.struct_owner(*def_id), "struct");
                if self.structs.insert(*def_id) {
                    self.add_types(item.fields.iter().map(|field| field.ty));
                }
            } else if let Some(item) = self.index.union_item(*def_id) {
                self.add_dependency(self.index.union_owner(*def_id), "union");
                if self.unions.insert(*def_id) {
                    self.add_types(item.fields.iter().map(|field| field.ty));
                }
            } else if self.index.enum_item(*def_id).is_none() {
                let owner = self
                    .owners
                    .struct_instance_owner(&key)
                    .or_else(|| self.owners.union_instance_owner(&key))
                    .or_else(|| self.owners.item_owner(*def_id));
                self.wait_for_owner(owner, "nominal type");
            } else {
                self.add_dependency(self.owners.item_owner(*def_id), "enum");
            }
        }
    }

    fn finish(mut self, unit: CodegenUnitId) -> CodegenDeclarationMembershipBuild {
        if !self.diagnostics.is_empty() {
            return CodegenDeclarationMembershipBuild::Invalid {
                diagnostics: self.diagnostics,
            };
        }
        for module_id in &self.dependency_modules {
            if !self.index.is_published(*module_id) {
                self.pending_modules.insert(*module_id);
            }
        }
        if !self.pending_modules.is_empty() {
            return CodegenDeclarationMembershipBuild::Pending(CodegenUnitPendingModules::new(
                unit,
                self.pending_modules,
            ));
        }
        let mut structs = self.structs.into_iter().collect::<Vec<_>>();
        structs.sort_unstable_by(|left, right| {
            stable_def_key(self.index, *left).cmp(&stable_def_key(self.index, *right))
        });
        let mut struct_instances = self.struct_instances.into_iter().collect::<Vec<_>>();
        struct_instances.sort_unstable_by(|left, right| {
            self.index
                .struct_instance(left.def_id, &left.args, &left.const_args)
                .unwrap()
                .symbol
                .cmp(
                    &self
                        .index
                        .struct_instance(right.def_id, &right.args, &right.const_args)
                        .unwrap()
                        .symbol,
                )
        });
        let mut union_instances = self.union_instances.into_iter().collect::<Vec<_>>();
        union_instances.sort_unstable_by(|left, right| {
            self.index
                .union_instance(left.def_id, &left.args, &left.const_args)
                .unwrap()
                .symbol
                .cmp(
                    &self
                        .index
                        .union_instance(right.def_id, &right.args, &right.const_args)
                        .unwrap()
                        .symbol,
                )
        });
        let mut function_instances = self.function_instances.into_iter().collect::<Vec<_>>();
        function_instances.sort_unstable_by(|left, right| {
            function_instance(self.index, left)
                .symbol
                .cmp(&function_instance(self.index, right).symbol)
        });
        let mut global_instances = self.global_instances.into_iter().collect::<Vec<_>>();
        global_instances.sort_unstable_by(|left, right| {
            global_instance(self.index, left)
                .symbol
                .cmp(&global_instance(self.index, right).symbol)
        });
        let mut unions = self.unions.into_iter().collect::<Vec<_>>();
        unions.sort_unstable_by(|left, right| {
            stable_def_key(self.index, *left).cmp(&stable_def_key(self.index, *right))
        });
        let mut functions = self.functions.into_iter().collect::<Vec<_>>();
        functions.sort_unstable_by(|left, right| {
            stable_def_key(self.index, *left).cmp(&stable_def_key(self.index, *right))
        });
        let mut globals = self.globals.into_iter().collect::<Vec<_>>();
        globals.sort_unstable_by(|left, right| {
            stable_def_key(self.index, *left).cmp(&stable_def_key(self.index, *right))
        });
        let mut vtables = self.vtables.into_iter().collect::<Vec<_>>();
        vtables.sort_by_cached_key(|key| {
            (
                stable_type_key(self.index, key.self_ty),
                stable_type_key(self.index, key.object_ty),
            )
        });
        CodegenDeclarationMembershipBuild::Ready(Box::new(CodegenDeclarationMembership {
            dependencies: CodegenUnitDependencies::new(unit, self.dependency_modules),
            structs,
            struct_instances,
            unions,
            union_instances,
            functions,
            function_instances,
            globals,
            global_instances,
            vtables,
        }))
    }

    fn invalid(&mut self, message: String) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            Span::default(),
            message,
        ));
    }
}

fn stable_def_key(index: &ProgramIndex, def_id: GlobalDefId) -> (&str, u64) {
    let module = index.module(def_id.module_id).unwrap_or_else(|| {
        panic!("Nia ICE: declaration membership references missing module {def_id:?}")
    });
    (module.source_identity.normalized_path(), def_id.def_id.0)
}

fn stable_type_key(index: &ProgramIndex, ty: InternedTyId) -> String {
    mangle_type_with(
        index.type_store(),
        ty,
        MangleResolvers::new(
            |module_id| {
                let module = index.module(module_id).unwrap_or_else(|| {
                    panic!("Nia ICE: stable type key references missing module {module_id:?}")
                });
                MangleModuleId::from_normalized_source_path(
                    module.source_identity.normalized_path(),
                )
            },
            |def_id| {
                index
                    .struct_item(def_id)
                    .map(|item| mangle_symbol_id(item.name))
                    .or_else(|| {
                        index
                            .union_item(def_id)
                            .map(|item| mangle_symbol_id(item.name))
                    })
                    .or_else(|| {
                        index
                            .enum_item(def_id)
                            .map(|item| mangle_symbol_id(item.name))
                    })
                    .or_else(|| {
                        index
                            .function(def_id)
                            .map(|item| mangle_symbol_id(item.name))
                    })
                    .unwrap_or_else(|| format!("def{}", def_id.def_id.0))
            },
            |const_expr: nia_ids::GlobalConstExprId| {
                index
                    .module(const_expr.module_id)
                    .and_then(|module| module.const_eval.array_lengths.get(&const_expr).copied())
            },
        ),
    )
}

fn function_instance<'a>(
    index: &'a ProgramIndex,
    key: &FunctionInstanceKey,
) -> &'a BackendFunctionInstance {
    index
        .function_instance(
            key.def_id,
            key.arg_module_id,
            key.self_arg,
            &key.args,
            &key.const_args,
        )
        .unwrap()
}

fn global_instance<'a>(
    index: &'a ProgramIndex,
    key: &BackendGlobalInstanceKey,
) -> &'a BackendGlobalInstance {
    index
        .global_instance(key.def_id, key.arg_module_id, &key.args, &key.const_args)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nia_backend_ir::{
        BackendConstFacts, BackendFunctionInstance, BackendGlobal, BackendLayouts, BackendModule,
        BackendModuleOwnerDirectory, BackendProgram, BackendStruct,
    };
    use nia_ids::{ConstExprId, DefId, GlobalConstExprId, ModuleIdAllocator};
    use nia_layout::TargetDataLayout;
    use nia_source::SourceIdentity;
    use nia_span::Span;
    use nia_static_ir::StaticInit;
    use nia_symbol::SymbolId;
    use nia_ty::{ConstGenericArg, ConstGenericValue, PrimitiveTy, TypeStore};

    use super::*;
    use crate::program_index::ProgramIndex;

    fn empty_module(module_id: ModuleId, name: &str) -> BackendModule {
        BackendModule {
            id: module_id,
            source_identity: SourceIdentity::new(name),
            name: name.to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: TargetDataLayout::LP64,
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }
    }

    fn caller_module(
        module_id: ModuleId,
        ty: InternedTyId,
        referenced_function: GlobalDefId,
    ) -> BackendModule {
        let mut module = empty_module(module_id, "caller.nia");
        module.globals.push(BackendGlobal {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(0),
            },
            name: SymbolId::EMPTY,
            link_name: None,
            ty,
            is_let: false,
            is_extern: false,
            init: Some(StaticInit::AddrOfFunction {
                function: referenced_function,
                args: vec![ty],
                const_args: Vec::new(),
            }),
            span: Span::default(),
        });
        module
    }

    fn instance_owner_module(
        module_id: ModuleId,
        semantic_def: GlobalDefId,
        caller: ModuleId,
        ty: InternedTyId,
    ) -> BackendModule {
        let mut module = empty_module(module_id, "instance-owner.nia");
        module.function_instances.push(BackendFunctionInstance {
            def_id: semantic_def,
            name: SymbolId::EMPTY,
            arg_module_id: caller,
            self_arg: None,
            args: vec![ty],
            const_args: Vec::new(),
            symbol: "instance".to_string(),
            params: Vec::new(),
            return_type: ty,
            is_extern: true,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: None,
            span: Span::default(),
        });
        module
    }

    fn instance_owner_module_with_const(
        module_id: ModuleId,
        semantic_def: GlobalDefId,
        caller: ModuleId,
        ty: InternedTyId,
        const_arg: ConstGenericArg,
    ) -> BackendModule {
        let mut module = empty_module(module_id, "instance-owner.nia");
        module.function_instances.push(BackendFunctionInstance {
            def_id: semantic_def,
            name: SymbolId::EMPTY,
            arg_module_id: caller,
            self_arg: None,
            args: vec![ty],
            const_args: vec![const_arg],
            symbol: "instance".to_string(),
            params: Vec::new(),
            return_type: ty,
            is_extern: true,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: None,
            span: Span::default(),
        });
        module
    }

    fn function_owner_module(
        module_id: ModuleId,
        def_id: GlobalDefId,
        ty: InternedTyId,
    ) -> BackendModule {
        let mut module = empty_module(module_id, "function-owner.nia");
        module.functions.push(BackendFunction {
            def_id,
            name: SymbolId::EMPTY,
            link_name: None,
            generics: Vec::new(),
            params: Vec::new(),
            return_type: ty,
            is_extern: true,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: None,
            span: Span::default(),
        });
        module
    }

    fn caller_partition(program: &BackendProgram, caller: ModuleId) -> CodegenPartition {
        program
            .codegen_partition_plan()
            .partitions()
            .iter()
            .find(|partition| {
                matches!(
                    partition.id,
                    CodegenUnitId::SourceModule { module_id, .. } if module_id == caller
                )
            })
            .expect("caller partition")
            .clone()
    }

    #[test]
    fn membership_waits_for_exact_actual_instance_owner() {
        let mut module_ids = ModuleIdAllocator::new();
        let caller = module_ids.allocate();
        let semantic_owner = module_ids.allocate();
        let actual_owner = module_ids.allocate();
        let unrelated = module_ids.allocate();
        let types = TypeStore::new();
        let ty = types.append_for_module(caller).primitive(PrimitiveTy::I32);
        let semantic_def = GlobalDefId {
            module_id: semantic_owner,
            def_id: DefId(7),
        };
        let modules = vec![
            caller_module(caller, ty, semantic_def),
            instance_owner_module(actual_owner, semantic_def, caller, ty),
            empty_module(unrelated, "unrelated.nia"),
        ];
        let owners = BackendModuleOwnerDirectory::from_modules(&modules);
        let program = BackendProgram::new(modules);
        let partition = caller_partition(&program, caller);
        let (index, mut publisher) = ProgramIndex::new(program.module_store(), Arc::new(types));
        publisher.publish(caller);

        let pending = match CodegenDeclarationMembership::build(&partition, &index, &owners) {
            CodegenDeclarationMembershipBuild::Pending(pending) => pending,
            CodegenDeclarationMembershipBuild::Ready(_) => {
                panic!("membership became ready before its actual instance owner")
            }
            CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                panic!("membership became invalid: {diagnostics:?}")
            }
        };
        assert_eq!(pending.unit(), partition.id);
        assert_eq!(pending.modules(), &[actual_owner]);
        assert!(!pending.modules().contains(&semantic_owner));
        assert!(!pending.modules().contains(&unrelated));

        publisher.publish(actual_owner);
        let ready = match CodegenDeclarationMembership::build(&partition, &index, &owners) {
            CodegenDeclarationMembershipBuild::Ready(ready) => ready,
            CodegenDeclarationMembershipBuild::Pending(pending) => {
                panic!("membership remained pending for {:?}", pending.modules())
            }
            CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                panic!("membership became invalid: {diagnostics:?}")
            }
        };
        assert_eq!(ready.dependencies.modules(), &[caller, actual_owner]);
    }

    #[test]
    fn membership_waits_for_const_expression_owner_in_nominal_type() {
        let mut module_ids = ModuleIdAllocator::new();
        let caller = module_ids.allocate();
        let const_owner = module_ids.allocate();
        let types = TypeStore::new();
        let append = types.append_for_module(caller);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let nominal_def = GlobalDefId {
            module_id: caller,
            def_id: DefId(9),
        };
        let nominal_ty = append.intern(TyKind::Nominal {
            def_id: nominal_def,
            args: Vec::new(),
            const_args: vec![ConstGenericArg {
                ty: i32_ty,
                value: ConstGenericValue::ConstExpr(GlobalConstExprId {
                    module_id: const_owner,
                    const_expr_id: ConstExprId(0),
                }),
            }],
        });
        let mut caller_backend = empty_module(caller, "caller.nia");
        caller_backend.globals.push(BackendGlobal {
            def_id: GlobalDefId {
                module_id: caller,
                def_id: DefId(0),
            },
            name: SymbolId::EMPTY,
            link_name: None,
            ty: nominal_ty,
            is_let: false,
            is_extern: true,
            init: None,
            span: Span::default(),
        });
        caller_backend.structs.push(BackendStruct {
            def_id: nominal_def,
            name: SymbolId::EMPTY,
            generics: Vec::new(),
            fields: Vec::new(),
            is_extern: false,
            span: Span::default(),
        });
        let modules = vec![caller_backend, empty_module(const_owner, "const-owner.nia")];
        let owners = BackendModuleOwnerDirectory::from_modules(&modules);
        let program = BackendProgram::new(modules);
        let unit = CodegenUnitId::SourceModule {
            module_id: caller,
            ordinal: 0,
        };
        let (index, mut publisher) = ProgramIndex::new(program.module_store(), Arc::new(types));
        publisher.publish(caller);

        let result = {
            let mut builder = MembershipBuilder::new(&index, &owners);
            builder.add_type(nominal_ty);
            builder.close_types();
            builder.finish(unit)
        };
        let pending = match result {
            CodegenDeclarationMembershipBuild::Pending(pending) => pending,
            CodegenDeclarationMembershipBuild::Ready(_) => {
                panic!("membership became ready before the const expression owner")
            }
            CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                panic!("membership became invalid: {diagnostics:?}")
            }
        };
        assert_eq!(pending.modules(), &[const_owner]);
    }

    #[test]
    fn membership_waits_for_const_expression_owner_in_instance_metadata() {
        let mut module_ids = ModuleIdAllocator::new();
        let caller = module_ids.allocate();
        let instance_owner = module_ids.allocate();
        let const_owner = module_ids.allocate();
        let types = TypeStore::new();
        let ty = types.append_for_module(caller).primitive(PrimitiveTy::I32);
        let semantic_def = GlobalDefId {
            module_id: caller,
            def_id: DefId(9),
        };
        let const_arg = ConstGenericArg {
            ty,
            value: ConstGenericValue::ConstExpr(GlobalConstExprId {
                module_id: const_owner,
                const_expr_id: ConstExprId(0),
            }),
        };
        let mut caller_backend = empty_module(caller, "caller.nia");
        caller_backend.globals.push(BackendGlobal {
            def_id: GlobalDefId {
                module_id: caller,
                def_id: DefId(0),
            },
            name: SymbolId::EMPTY,
            link_name: None,
            ty,
            is_let: false,
            is_extern: false,
            init: Some(StaticInit::AddrOfFunction {
                function: semantic_def,
                args: vec![ty],
                const_args: vec![const_arg.clone()],
            }),
            span: Span::default(),
        });
        let modules = vec![
            caller_backend,
            instance_owner_module_with_const(instance_owner, semantic_def, caller, ty, const_arg),
            empty_module(const_owner, "const-owner.nia"),
        ];
        let owners = BackendModuleOwnerDirectory::from_modules(&modules);
        let program = BackendProgram::new(modules);
        let partition = caller_partition(&program, caller);
        let (index, mut publisher) = ProgramIndex::new(program.module_store(), Arc::new(types));
        publisher.publish(caller);
        publisher.publish(instance_owner);

        let pending = match CodegenDeclarationMembership::build(&partition, &index, &owners) {
            CodegenDeclarationMembershipBuild::Pending(pending) => pending,
            CodegenDeclarationMembershipBuild::Ready(_) => {
                panic!("membership became ready before the const expression owner")
            }
            CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                panic!("membership became invalid: {diagnostics:?}")
            }
        };
        assert_eq!(pending.modules(), &[const_owner]);
    }

    #[test]
    fn stable_type_key_preserves_evaluated_array_lengths() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let left = append.intern(TyKind::Array {
            len: nia_ty::ArrayLenTy::ConstExpr(GlobalConstExprId {
                module_id,
                const_expr_id: ConstExprId(0),
            }),
            elem: i32_ty,
        });
        let right = append.intern(TyKind::Array {
            len: nia_ty::ArrayLenTy::ConstExpr(GlobalConstExprId {
                module_id,
                const_expr_id: ConstExprId(1),
            }),
            elem: i32_ty,
        });
        let mut module = empty_module(module_id, "arrays.nia");
        module.const_eval.array_lengths.insert(
            GlobalConstExprId {
                module_id,
                const_expr_id: ConstExprId(0),
            },
            4,
        );
        module.const_eval.array_lengths.insert(
            GlobalConstExprId {
                module_id,
                const_expr_id: ConstExprId(1),
            },
            8,
        );
        let program = BackendProgram::new(vec![module]);
        let store = program.module_store();
        let (index, mut publisher) = ProgramIndex::new(store, Arc::new(types));
        publisher.publish(module_id);

        assert_ne!(
            stable_type_key(&index, left),
            stable_type_key(&index, right)
        );
    }

    #[test]
    fn referenced_vtable_waits_for_its_function_owner() {
        let mut module_ids = ModuleIdAllocator::new();
        let caller = module_ids.allocate();
        let vtable_owner = module_ids.allocate();
        let function_owner = module_ids.allocate();
        let types = TypeStore::new();
        let append = types.append_for_module(caller);
        let ty = append.primitive(PrimitiveTy::I32);
        let upcast_object_ty = append.primitive(PrimitiveTy::Bool);
        let trait_def = GlobalDefId {
            module_id: vtable_owner,
            def_id: DefId(3),
        };
        let function_def = GlobalDefId {
            module_id: function_owner,
            def_id: DefId(7),
        };
        let key = BackendTraitObjectVtableKey {
            self_ty: ty,
            object_ty: ty,
        };
        let mut vtable_module = empty_module(vtable_owner, "vtable-owner.nia");
        vtable_module
            .trait_object_vtables
            .push(BackendTraitObjectVtable {
                key: key.clone(),
                trait_id: TraitId::Source(trait_def),
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
                entries: vec![nia_backend_ir::BackendTraitObjectVtableEntry {
                    trait_id: TraitId::Source(trait_def),
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    method_id: trait_def,
                    method_name: SymbolId::EMPTY,
                    slot: 0,
                    function: BackendTraitObjectVtableFunction::Function(function_def),
                }],
                span: Span::default(),
            });
        let modules = vec![
            empty_module(caller, "caller.nia"),
            vtable_module,
            function_owner_module(function_owner, function_def, ty),
        ];
        let owners = BackendModuleOwnerDirectory::from_modules(&modules);
        let program = BackendProgram::new(modules);
        let partition = caller_partition(&program, vtable_owner);
        let (index, mut publisher) = ProgramIndex::new(program.module_store(), Arc::new(types));
        publisher.publish(caller);

        let build = |index: &ProgramIndex| {
            let mut builder = MembershipBuilder::new(index, &owners);
            let mut refs = FunctionBodyRefs::default();
            refs.dynamic_trait_calls.insert(DynamicTraitCallRef {
                // An upcast view names a different object type while retaining
                // the source vtable that contains this trait's slot segment.
                object_ty: upcast_object_ty,
                trait_id: TraitId::Source(trait_def),
            });
            builder.add_refs(refs);
            builder.close_types();
            builder.finish(partition.id)
        };
        let pending = match build(&index) {
            CodegenDeclarationMembershipBuild::Pending(pending) => pending,
            CodegenDeclarationMembershipBuild::Ready(_) => {
                panic!("membership became ready before the vtable function owner")
            }
            CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                panic!("membership became invalid: {diagnostics:?}")
            }
        };
        assert_eq!(pending.modules(), &[vtable_owner]);

        publisher.publish(vtable_owner);
        let pending = match build(&index) {
            CodegenDeclarationMembershipBuild::Pending(pending) => pending,
            CodegenDeclarationMembershipBuild::Ready(_) => {
                panic!("membership became ready before the vtable function owner")
            }
            CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                panic!("membership became invalid: {diagnostics:?}")
            }
        };
        assert_eq!(pending.modules(), &[function_owner]);

        publisher.publish(function_owner);
        let ready = match build(&index) {
            CodegenDeclarationMembershipBuild::Ready(ready) => ready,
            CodegenDeclarationMembershipBuild::Pending(pending) => {
                panic!("membership remained pending for {:?}", pending.modules())
            }
            CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                panic!("membership became invalid: {diagnostics:?}")
            }
        };
        assert_eq!(ready.functions, vec![function_def]);
        assert!(ready.dependencies.contains(vtable_owner));
        assert!(ready.dependencies.contains(function_owner));
    }

    #[test]
    fn published_owner_without_payload_is_a_structural_error() {
        let mut module_ids = ModuleIdAllocator::new();
        let caller = module_ids.allocate();
        let semantic_owner = module_ids.allocate();
        let actual_owner = module_ids.allocate();
        let types = TypeStore::new();
        let ty = types.append_for_module(caller).primitive(PrimitiveTy::I32);
        let semantic_def = GlobalDefId {
            module_id: semantic_owner,
            def_id: DefId(7),
        };
        let caller_module = caller_module(caller, ty, semantic_def);
        let directory_owner = instance_owner_module(actual_owner, semantic_def, caller, ty);
        let owners = BackendModuleOwnerDirectory::from_modules([&caller_module, &directory_owner]);
        let program = BackendProgram::new(vec![
            caller_module,
            empty_module(actual_owner, "empty-owner.nia"),
        ]);
        let partition = caller_partition(&program, caller);
        let (index, mut publisher) = ProgramIndex::new(program.module_store(), Arc::new(types));
        publisher.publish(caller);
        publisher.publish(actual_owner);

        let result = CodegenDeclarationMembership::build(&partition, &index, &owners);
        let CodegenDeclarationMembershipBuild::Invalid { diagnostics } = result else {
            panic!("expected invalid membership result")
        };
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].summary.contains("missing function instance"));
    }

    #[test]
    fn invalid_function_refs_are_a_structural_error() {
        let module_id = ModuleIdAllocator::new().allocate();
        let module = empty_module(module_id, "main.nia");
        let owners = BackendModuleOwnerDirectory::from_modules([&module]);
        let program = BackendProgram::new(vec![module]);
        let (index, mut publisher) =
            ProgramIndex::new(program.module_store(), Arc::new(TypeStore::new()));
        publisher.publish(module_id);
        let mut builder = MembershipBuilder::new(&index, &owners);
        let refs = FunctionBodyRefs {
            invalid_ir: true,
            ..FunctionBodyRefs::default()
        };

        builder.add_refs(refs);
        let result = builder.finish(CodegenUnitId::SourceModule {
            module_id,
            ordinal: 0,
        });

        let CodegenDeclarationMembershipBuild::Invalid { diagnostics } = result else {
            panic!("expected invalid membership result")
        };
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].summary.contains("invalid function IR"));
    }
}
