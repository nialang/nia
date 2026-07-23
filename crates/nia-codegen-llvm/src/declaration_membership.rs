// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{BTreeSet, HashSet, VecDeque};

use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendGlobal, BackendGlobalInstance,
    BackendGlobalInstanceKey, BackendStructInstanceKey, BackendTraitObjectVtable,
    BackendTraitObjectVtableFunction, BackendTraitObjectVtableKey, CodegenPartition,
};
use nia_function_ir::{FunctionBodyRefs, FunctionInstanceKey, TraitObjectVtableRef};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_mangle::{mangle_symbol_id, mangle_type_with};
use nia_ty::TyKind;

use crate::program_index::ProgramIndex;

#[derive(Debug)]
pub(super) struct CodegenDeclarationMembership {
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

impl CodegenDeclarationMembership {
    pub(super) fn build(partition: &CodegenPartition, index: &ProgramIndex) -> Self {
        MembershipBuilder::new(index).build(partition)
    }
}

struct MembershipBuilder<'a> {
    index: &'a ProgramIndex,
    structs: BTreeSet<GlobalDefId>,
    struct_instances: HashSet<BackendStructInstanceKey>,
    unions: BTreeSet<GlobalDefId>,
    union_instances: HashSet<BackendStructInstanceKey>,
    functions: BTreeSet<GlobalDefId>,
    function_instances: HashSet<FunctionInstanceKey>,
    globals: BTreeSet<GlobalDefId>,
    global_instances: HashSet<BackendGlobalInstanceKey>,
    vtables: HashSet<BackendTraitObjectVtableKey>,
    expanded_vtable_definitions: HashSet<BackendTraitObjectVtableKey>,
    pending_types: VecDeque<InternedTyId>,
    visited_types: HashSet<InternedTyId>,
}

impl<'a> MembershipBuilder<'a> {
    fn new(index: &'a ProgramIndex) -> Self {
        Self {
            index,
            structs: BTreeSet::new(),
            struct_instances: HashSet::new(),
            unions: BTreeSet::new(),
            union_instances: HashSet::new(),
            functions: BTreeSet::new(),
            function_instances: HashSet::new(),
            globals: BTreeSet::new(),
            global_instances: HashSet::new(),
            vtables: HashSet::new(),
            expanded_vtable_definitions: HashSet::new(),
            pending_types: VecDeque::new(),
            visited_types: HashSet::new(),
        }
    }

    fn build(mut self, partition: &CodegenPartition) -> CodegenDeclarationMembership {
        let owner = self.index.program().module_for_partition(partition);
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
        for &index in partition.vtable_definitions() {
            self.add_vtable_definition(&owner.trait_object_vtables[index]);
        }
        self.close_types();
        self.finish()
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

    fn add_global(&mut self, item: &BackendGlobal) {
        if self.globals.insert(item.def_id) {
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
            self.add_type(item.ty);
            self.add_types(item.args.iter().copied());
            self.add_types(item.const_args.iter().map(|arg| arg.ty));
        }
    }

    fn add_function(&mut self, item: &BackendFunction) {
        if self.functions.insert(item.def_id) {
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
            self.add_function_signature(&item.params, item.return_type);
            self.add_types(item.self_arg);
            self.add_types(item.args.iter().copied());
            self.add_types(item.const_args.iter().map(|arg| arg.ty));
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
        for def_id in refs.functions {
            let item = self.index.function(def_id).unwrap_or_else(|| {
                panic!("Nia ICE: declaration closure references missing function {def_id:?}")
            });
            self.add_function(item);
        }
        for def_id in refs.globals {
            let item = self.index.global(def_id).unwrap_or_else(|| {
                panic!("Nia ICE: declaration closure references missing global {def_id:?}")
            });
            self.add_global(item);
        }
        for reference in refs.function_instances {
            let item = self
                .index
                .function_instance(
                    reference.def_id,
                    reference.arg_module_id,
                    reference.self_arg,
                    &reference.args,
                    &reference.const_args,
                )
                .unwrap_or_else(|| {
                    panic!(
                        "Nia ICE: declaration closure references missing function instance {:?}",
                        reference.key()
                    )
                });
            self.add_function_instance(item);
        }
        for reference in refs.global_instances {
            let item = self
                .index
                .global_instance(
                    reference.def_id,
                    reference.arg_module_id,
                    &reference.args,
                    &reference.const_args,
                )
                .unwrap_or_else(|| {
                    panic!(
                        "Nia ICE: declaration closure references missing global instance {:?}",
                        reference.key()
                    )
                });
            self.add_global_instance(item);
        }
        self.add_types(refs.types);
        for reference in refs.trait_object_vtables {
            self.add_vtable_reference(reference);
        }
    }

    fn add_vtable_reference(&mut self, reference: TraitObjectVtableRef) {
        let key = BackendTraitObjectVtableKey {
            self_ty: reference.self_ty,
            object_ty: reference.object_ty,
        };
        let item = self.index.trait_object_vtable(&key).unwrap_or_else(|| {
            panic!("Nia ICE: declaration closure references missing vtable {key:?}")
        });
        self.add_vtable(item);
    }

    fn add_vtable(&mut self, item: &BackendTraitObjectVtable) {
        if self.vtables.insert(item.key.clone()) {
            self.add_types([item.key.self_ty, item.key.object_ty]);
            self.add_types(item.trait_args.iter().copied());
        }
    }

    fn add_vtable_definition(&mut self, item: &BackendTraitObjectVtable) {
        self.add_vtable(item);
        if !self.expanded_vtable_definitions.insert(item.key.clone()) {
            return;
        }
        for entry in &item.entries {
            match &entry.function {
                BackendTraitObjectVtableFunction::Function(def_id) => {
                    let function = self.index.function(*def_id).unwrap_or_else(|| {
                        panic!("Nia ICE: codegen vtable references missing function {def_id:?}")
                    });
                    self.add_function(function);
                }
                BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                } => {
                    let function = self
                        .index
                        .function_instance(*def_id, *arg_module_id, *self_arg, args, const_args)
                        .unwrap_or_else(|| {
                            panic!("Nia ICE: codegen vtable references missing function instance")
                        });
                    self.add_function_instance(function);
                }
            }
        }
    }

    fn add_type(&mut self, ty: InternedTyId) {
        if self.visited_types.insert(ty) {
            self.pending_types.push_back(ty);
        }
    }

    fn add_types(&mut self, types: impl IntoIterator<Item = InternedTyId>) {
        for ty in types {
            self.add_type(ty);
        }
    }

    fn close_types(&mut self) {
        while let Some(ty) = self.pending_types.pop_front() {
            let kind = self.index.ty_kind(ty).unwrap_or_else(|| {
                panic!("Nia ICE: declaration closure references missing type {ty:?}")
            });
            kind.visit_referenced_types(|referenced| self.add_type(referenced));
            let TyKind::Nominal {
                def_id,
                args,
                const_args,
            } = kind
            else {
                continue;
            };
            if let Some(item) = self.index.struct_instance(*def_id, args, const_args) {
                let key = BackendStructInstanceKey {
                    def_id: *def_id,
                    args: args.clone(),
                    const_args: const_args.clone(),
                };
                if self.struct_instances.insert(key) {
                    self.add_types(item.fields.iter().map(|field| field.ty));
                }
            } else if let Some(item) = self.index.union_instance(*def_id, args, const_args) {
                let key = BackendStructInstanceKey {
                    def_id: *def_id,
                    args: args.clone(),
                    const_args: const_args.clone(),
                };
                if self.union_instances.insert(key) {
                    self.add_types(item.fields.iter().map(|field| field.ty));
                }
            } else if let Some(item) = self.index.struct_item(*def_id) {
                if self.structs.insert(*def_id) {
                    self.add_types(item.fields.iter().map(|field| field.ty));
                }
            } else if let Some(item) = self.index.union_item(*def_id)
                && self.unions.insert(*def_id)
            {
                self.add_types(item.fields.iter().map(|field| field.ty));
            }
        }
    }

    fn finish(self) -> CodegenDeclarationMembership {
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
        CodegenDeclarationMembership {
            structs,
            struct_instances,
            unions,
            union_instances,
            functions,
            function_instances,
            globals,
            global_instances,
            vtables,
        }
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
        |_| Some(0),
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
