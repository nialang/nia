// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_backend_ir::{
    BackendLayouts, BackendStructInstance, BackendStructInstanceKey, BackendUnionInstance,
};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_layout::{ProgramLayoutContext, StructLayout};

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

    pub(crate) fn extend_for_instances(
        &mut self,
        layouts: &mut BackendLayouts,
        struct_instances: &[BackendStructInstance],
        union_instances: &[BackendUnionInstance],
    ) {
        let mut normalization_input = self.input.signatures.type_roots();
        for instance in struct_instances {
            normalization_input.extend(instance.args.iter().copied());
            normalization_input.extend(instance.const_args.iter().map(|arg| arg.ty));
        }
        for instance in union_instances {
            normalization_input.extend(instance.args.iter().copied());
            normalization_input.extend(instance.const_args.iter().map(|arg| arg.ty));
        }
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
                return input.const_array_lengths.values.get(&id).copied();
            }
            input
                .program_const
                .get(&id.module_id)
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        let program = ProgramLayoutContext {
            symbols: Some(self.input.symbols),
            layouts: None,
            array_lengths: Some(&array_lengths),
            structs: Some(self.input.program_structs),
            unions: Some(self.input.program_unions),
            ..Default::default()
        };
        let root_types = self.input.signatures.type_roots();
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
        Self::append_instance_layouts(layouts, &layout_input, struct_instances, union_instances);
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

fn append_missing_nominal_layouts(
    output: &mut Vec<(GlobalDefId, StructLayout)>,
    computed: HashMap<nia_ids::DefId, StructLayout>,
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
