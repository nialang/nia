// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendLayouts, BackendModule, BackendParam,
    BackendStructInstance, BackendStructInstanceKey, BackendUnionInstance,
};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_layout::ProgramLayoutContext;

use crate::BackendLowerModuleInput;

pub(crate) struct BackendLayoutExtender<'input, 'ctx> {
    input: &'ctx BackendLowerModuleInput<'input>,
    type_store: &'ctx nia_ty::TypeStore,
}

impl<'input, 'ctx> BackendLayoutExtender<'input, 'ctx> {
    pub(crate) fn new(
        input: &'ctx BackendLowerModuleInput<'input>,
        type_store: &'ctx nia_ty::TypeStore,
    ) -> Self {
        Self { input, type_store }
    }

    pub(crate) fn extend_for_finalized_module(
        &mut self,
        layouts: &mut BackendLayouts,
        module: &BackendModule,
    ) {
        let mut normalization_input = self.input.signatures.type_roots();
        normalization_input.extend(finalized_module_type_roots(module, self.type_store));
        normalization_input.sort_unstable();
        normalization_input.dedup();
        let normalization = nia_type_normalize::normalize_module_types(
            nia_type_normalize::TypeNormalizationInput {
                module_id: self.input.module_id,
                type_store: self.type_store,
                input_ids: &normalization_input,
                signatures: self.input.signatures,
            },
        );
        let input = self.input;
        let array_lengths = |id: GlobalConstExprId| {
            if id.module_id == input.module_id {
                return input.const_array_lengths.get(&id).copied();
            }
            input
                .program
                .const_array_lengths(id.module_id)
                .and_then(|array_lengths| array_lengths.get(&id).copied())
        };
        let program = ProgramLayoutContext {
            symbols: Some(self.input.symbols),
            layouts: None,
            array_lengths: Some(&array_lengths),
            structs: Some(self.input.program.structs()),
            unions: Some(self.input.program.unions()),
            enums: Some(self.input.program.enums()),
            type_aliases: Some(self.input.program.type_aliases()),
            ..Default::default()
        };
        let root_types = normalization_input;
        let computed =
            nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
                type_store: self.type_store,
                defs: self.input.defs,
                signatures: self.input.signatures,
                root_types: &root_types,
                normalized: &normalization.normalized,
                array_lengths: &array_lengths,
                target: self.input.layouts.target,
                program,
            });
        append_missing_type_layouts(&mut layouts.types, computed.types);
        append_missing_nominal_layouts(
            &mut layouts.structs,
            computed.structs,
            self.input.module_id,
        );
        append_missing_nominal_layouts(&mut layouts.unions, computed.unions, self.input.module_id);
        append_missing_nominal_layouts(&mut layouts.enums, computed.enums, self.input.module_id);
        let layout_input = nia_layout::LayoutComputationInput {
            type_store: self.type_store,
            defs: self.input.defs,
            signatures: self.input.signatures,
            root_types: &root_types,
            normalized: &normalization.normalized,
            array_lengths: &array_lengths,
            target: self.input.layouts.target,
            program,
        };
        Self::append_instance_layouts(
            layouts,
            &layout_input,
            &module.struct_instances,
            &module.union_instances,
        );
    }

    fn append_instance_layouts(
        layouts: &mut BackendLayouts,
        layout_input: &nia_layout::LayoutComputationInput<'_>,
        struct_instances: &[BackendStructInstance],
        union_instances: &[BackendUnionInstance],
    ) {
        let mut seen_structs = layouts
            .struct_instances
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        for instance in struct_instances {
            let key = BackendStructInstanceKey {
                def_id: instance.def_id,
                args: instance.args.clone(),
                const_args: instance.const_args.clone(),
            };
            if !seen_structs.insert(key.clone()) {
                continue;
            }
            if let Some(layout) = nia_layout::compute_struct_instance_layout_with_program_context(
                layout_input,
                nia_layout::InstanceLayoutRequest {
                    def_id: instance.def_id,
                    args: &instance.args,
                    const_args: &instance.const_args,
                },
            ) {
                layouts.struct_instances.push((key, layout));
            }
        }

        let mut seen_unions = layouts
            .union_instances
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        for instance in union_instances {
            let key = BackendStructInstanceKey {
                def_id: instance.def_id,
                args: instance.args.clone(),
                const_args: instance.const_args.clone(),
            };
            if !seen_unions.insert(key.clone()) {
                continue;
            }
            if let Some(layout) = nia_layout::compute_union_instance_layout_with_program_context(
                layout_input,
                nia_layout::InstanceLayoutRequest {
                    def_id: instance.def_id,
                    args: &instance.args,
                    const_args: &instance.const_args,
                },
            ) {
                layouts.union_instances.push((key, layout));
            }
        }
    }
}

fn append_missing_type_layouts(
    output: &mut Vec<(InternedTyId, nia_layout::TypeLayout)>,
    computed: HashMap<InternedTyId, nia_layout::TypeLayout>,
) {
    for (ty, layout) in computed {
        if let Some((_, existing)) = output
            .iter_mut()
            .find(|(existing_ty, _)| *existing_ty == ty)
        {
            *existing = layout;
        } else {
            output.push((ty, layout));
        }
    }
}

fn append_missing_nominal_layouts<Layout>(
    output: &mut Vec<(GlobalDefId, Layout)>,
    computed: HashMap<nia_ids::DefId, Layout>,
    default_module_id: ModuleId,
) {
    let mut existing = output
        .iter()
        .map(|(def_id, _)| *def_id)
        .collect::<HashSet<_>>();
    for (def_id, layout) in computed {
        let def_id = GlobalDefId {
            module_id: default_module_id,
            def_id,
        };
        if existing.insert(def_id) {
            output.push((def_id, layout));
        }
    }
}

fn finalized_module_type_roots(
    module: &BackendModule,
    type_store: &nia_ty::TypeStore,
) -> Vec<InternedTyId> {
    let mut roots = Vec::new();
    for item in &module.structs {
        roots.extend(item.fields.iter().map(|field| field.ty));
    }
    for item in &module.unions {
        roots.extend(item.fields.iter().map(|field| field.ty));
    }
    for item in &module.struct_instances {
        extend_instance_types(&mut roots, &item.args, &item.const_args);
        roots.extend(item.fields.iter().map(|field| field.ty));
    }
    for item in &module.union_instances {
        extend_instance_types(&mut roots, &item.args, &item.const_args);
        roots.extend(item.fields.iter().map(|field| field.ty));
    }
    for item in &module.enums {
        roots.push(item.backing_type);
        for variant in &item.variants {
            match &variant.payload {
                nia_backend_ir::BackendEnumVariantPayload::Unit => {}
                nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => {
                    roots.extend(fields.iter().copied());
                }
                nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                    roots.extend(fields.iter().map(|field| field.ty));
                }
            }
        }
    }
    for item in &module.globals {
        roots.push(item.ty);
        if let Some(init) = &item.init {
            roots.extend(init.value_refs(module.id).types);
        }
    }
    for item in &module.global_instances {
        roots.push(item.ty);
        extend_instance_types(&mut roots, &item.args, &item.const_args);
        if let Some(init) = &item.init {
            roots.extend(init.value_refs(item.arg_module_id).types);
        }
    }
    for item in &module.functions {
        extend_function_types(&mut roots, item, type_store);
    }
    for item in &module.function_instances {
        extend_function_instance_types(&mut roots, item, type_store);
    }
    for vtable in &module.trait_object_vtables {
        roots.extend([vtable.key.self_ty, vtable.key.object_ty]);
        roots.extend(vtable.trait_args.iter().copied());
        for entry in &vtable.entries {
            if let nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                self_arg,
                args,
                const_args,
                ..
            } = &entry.function
            {
                roots.extend(self_arg.iter().copied());
                extend_instance_types(&mut roots, args, const_args);
            }
        }
    }
    for instantiation in &module.generic_instantiations {
        roots.extend(instantiation.self_arg.iter().copied());
        extend_instance_types(&mut roots, &instantiation.args, &instantiation.const_args);
    }
    roots
}

fn extend_function_types(
    roots: &mut Vec<InternedTyId>,
    function: &BackendFunction,
    type_store: &nia_ty::TypeStore,
) {
    extend_params(roots, &function.params);
    roots.push(function.return_type);
    if let Some(body) = &function.function_body {
        roots.extend(body.value_refs(type_store).types);
    }
}

fn extend_function_instance_types(
    roots: &mut Vec<InternedTyId>,
    function: &BackendFunctionInstance,
    type_store: &nia_ty::TypeStore,
) {
    roots.extend(function.self_arg.iter().copied());
    extend_instance_types(roots, &function.args, &function.const_args);
    extend_params(roots, &function.params);
    roots.push(function.return_type);
    if let Some(body) = &function.function_body {
        roots.extend(body.value_refs(type_store).types);
    }
}

fn extend_params(roots: &mut Vec<InternedTyId>, params: &[BackendParam]) {
    for param in params {
        roots.extend([param.passing_ty, param.local_ty]);
    }
}

fn extend_instance_types(
    roots: &mut Vec<InternedTyId>,
    args: &[InternedTyId],
    const_args: &[nia_ty::ConstGenericArg],
) {
    roots.extend(args.iter().copied());
    roots.extend(const_args.iter().map(|arg| arg.ty));
}
