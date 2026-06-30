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
    interner: &'ctx nia_ty::TyInterner,
}

impl<'input, 'ctx> BackendLayoutExtender<'input, 'ctx> {
    pub(crate) fn new(
        input: &'ctx BackendLowerModuleInput<'input>,
        interner: &'ctx nia_ty::TyInterner,
    ) -> Self {
        Self { input, interner }
    }

    pub(crate) fn extend_for_instances(
        &self,
        layouts: &mut BackendLayouts,
        struct_instances: &[BackendStructInstance],
        union_instances: &[BackendUnionInstance],
    ) {
        let array_lengths = |id| self.program_array_len(id);
        let program = ProgramLayoutContext {
            layouts: None,
            array_lengths: Some(&array_lengths),
            structs: Some(self.input.program_structs),
            unions: Some(self.input.program_unions),
            ..Default::default()
        };
        let normalization = nia_type_normalize::normalize_module_types(
            self.input.module_id,
            self.interner,
            self.input.signatures,
        );
        let layout_input = nia_layout::LayoutComputationInput {
            defs: self.input.defs,
            interner: self.interner,
            signatures: self.input.signatures,
            normalized: &normalization.normalized,
            array_lengths: &array_lengths,
            target: self.input.layouts.target,
            program,
        };
        let computed = nia_layout::compute_layouts_with_program_context(
            self.input.defs,
            self.interner,
            self.input.signatures,
            &normalization.normalized,
            &array_lengths,
            self.input.layouts.target,
            program,
        );
        append_missing_type_layouts(&mut layouts.types, computed.types);
        append_missing_nominal_layouts(
            &mut layouts.structs,
            computed.structs,
            self.input.module_id,
        );
        append_missing_nominal_layouts(&mut layouts.unions, computed.unions, self.input.module_id);
        self.append_local_instance_layouts(
            layouts,
            layout_input,
            struct_instances,
            union_instances,
        );
        self.append_foreign_instance_layouts(layouts, struct_instances, union_instances);
    }

    fn append_local_instance_layouts(
        &self,
        layouts: &mut BackendLayouts,
        layout_input: nia_layout::LayoutComputationInput<'_>,
        struct_instances: &[BackendStructInstance],
        union_instances: &[BackendUnionInstance],
    ) {
        let mut seen_structs = layouts
            .struct_instances
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        for instance in struct_instances {
            if instance.def_id.module_id != self.input.module_id {
                continue;
            }
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
            if instance.def_id.module_id != self.input.module_id {
                continue;
            }
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

    fn append_foreign_instance_layouts(
        &self,
        layouts: &mut BackendLayouts,
        struct_instances: &[BackendStructInstance],
        union_instances: &[BackendUnionInstance],
    ) {
        let array_lengths = |id| self.program_array_len(id);
        let program = ProgramLayoutContext {
            layouts: None,
            array_lengths: Some(&array_lengths),
            structs: Some(self.input.program_structs),
            unions: Some(self.input.program_unions),
            ..Default::default()
        };
        let layout_input = nia_layout::LayoutComputationInput {
            defs: self.input.defs,
            interner: self.interner,
            signatures: self.input.signatures,
            normalized: &self.input.type_normalization.normalized,
            array_lengths: &array_lengths,
            target: self.input.layouts.target,
            program,
        };

        let mut seen_structs = layouts
            .struct_instances
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        for instance in struct_instances {
            if instance.def_id.module_id == self.input.module_id {
                continue;
            }
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
            if instance.def_id.module_id == self.input.module_id {
                continue;
            }
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

    fn program_array_len(&self, id: GlobalConstExprId) -> Option<u64> {
        if id.module_id == self.input.module_id {
            return self.input.comptime_array_lengths.values.get(&id).copied();
        }
        self.input
            .program_comptime
            .get(&id.module_id)
            .and_then(|array_lengths| array_lengths.values.get(&id).copied())
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
