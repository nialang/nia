// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendFunctionInstance, BackendProgram,
    BackendStructInstanceKey, BackendTraitObjectVtable,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_layout::{StructLayout, TypeLayout};
use nia_ty::{ConstGenericArg, TraitId, TyKind};

pub(super) struct ProgramIndex<'a> {
    pub(super) modules: HashMap<ModuleId, &'a nia_backend_ir::BackendModule>,
    pub(super) structs: HashMap<GlobalDefId, &'a nia_backend_ir::BackendStruct>,
    pub(super) unions: HashMap<GlobalDefId, &'a nia_backend_ir::BackendUnion>,
    pub(super) struct_instances: HashMap<
        GlobalDefId,
        HashMap<
            (Vec<InternedTyId>, Vec<ConstGenericArg>),
            &'a nia_backend_ir::BackendStructInstance,
        >,
    >,
    pub(super) union_instances: HashMap<
        GlobalDefId,
        HashMap<
            (Vec<InternedTyId>, Vec<ConstGenericArg>),
            &'a nia_backend_ir::BackendUnionInstance,
        >,
    >,
    pub(super) enums: HashMap<GlobalDefId, &'a nia_backend_ir::BackendEnum>,
    pub(super) enum_variants: HashMap<GlobalDefId, &'a nia_backend_ir::BackendEnumVariant>,
    pub(super) enum_variant_infos: HashMap<GlobalDefId, EnumVariantInfo<'a>>,
    pub(super) globals: HashMap<GlobalDefId, &'a nia_backend_ir::BackendGlobal>,
    pub(super) global_instances: HashMap<
        (GlobalDefId, ModuleId),
        HashMap<
            (Vec<InternedTyId>, Vec<ConstGenericArg>),
            &'a nia_backend_ir::BackendGlobalInstance,
        >,
    >,
    pub(super) global_instances_by_def:
        HashMap<GlobalDefId, Vec<&'a nia_backend_ir::BackendGlobalInstance>>,
    pub(super) functions: HashMap<GlobalDefId, &'a nia_backend_ir::BackendFunction>,
    pub(super) function_instances: HashMap<
        (GlobalDefId, ModuleId),
        HashMap<(Vec<InternedTyId>, Vec<ConstGenericArg>), &'a BackendFunctionInstance>,
    >,
    pub(super) function_instances_by_def: HashMap<GlobalDefId, Vec<&'a BackendFunctionInstance>>,
    trait_object_vtables_by_object_ty: HashMap<InternedTyId, Vec<&'a BackendTraitObjectVtable>>,
    trait_object_vtables_by_trait: HashMap<TraitId, Vec<&'a BackendTraitObjectVtable>>,
    type_layouts: HashMap<InternedTyId, &'a TypeLayout>,
    struct_layouts: HashMap<GlobalDefId, &'a StructLayout>,
    union_layouts: HashMap<GlobalDefId, &'a StructLayout>,
    struct_instance_layouts:
        HashMap<GlobalDefId, HashMap<(Vec<InternedTyId>, Vec<ConstGenericArg>), &'a StructLayout>>,
    union_instance_layouts:
        HashMap<GlobalDefId, HashMap<(Vec<InternedTyId>, Vec<ConstGenericArg>), &'a StructLayout>>,
    struct_instance_layouts_by_def: HashMap<GlobalDefId, Vec<BackendLayoutInstance<'a>>>,
    union_instance_layouts_by_def: HashMap<GlobalDefId, Vec<BackendLayoutInstance<'a>>>,
    pub(super) struct_instances_by_def:
        HashMap<GlobalDefId, Vec<&'a nia_backend_ir::BackendStructInstance>>,
    pub(super) union_instances_by_def:
        HashMap<GlobalDefId, Vec<&'a nia_backend_ir::BackendUnionInstance>>,
}

pub(super) struct BackendLayoutInstance<'a> {
    pub(super) key: &'a BackendStructInstanceKey,
    pub(super) layout: &'a StructLayout,
}

pub(super) struct EnumVariantInfo<'a> {
    pub(super) owner: &'a BackendEnum,
    pub(super) variant: &'a BackendEnumVariant,
    pub(super) index: usize,
}

impl<'a> ProgramIndex<'a> {
    pub(super) fn new(program: &'a BackendProgram) -> Self {
        let mut index = Self {
            modules: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            struct_instances: HashMap::new(),
            union_instances: HashMap::new(),
            enums: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_variant_infos: HashMap::new(),
            globals: HashMap::new(),
            global_instances: HashMap::new(),
            global_instances_by_def: HashMap::new(),
            functions: HashMap::new(),
            function_instances: HashMap::new(),
            function_instances_by_def: HashMap::new(),
            trait_object_vtables_by_object_ty: HashMap::new(),
            trait_object_vtables_by_trait: HashMap::new(),
            type_layouts: HashMap::new(),
            struct_layouts: HashMap::new(),
            union_layouts: HashMap::new(),
            struct_instance_layouts: HashMap::new(),
            union_instance_layouts: HashMap::new(),
            struct_instance_layouts_by_def: HashMap::new(),
            union_instance_layouts_by_def: HashMap::new(),
            struct_instances_by_def: HashMap::new(),
            union_instances_by_def: HashMap::new(),
        };
        for module in &program.modules {
            index.modules.insert(module.id, module);
            for (ty, layout) in &module.layouts.types {
                index.type_layouts.insert(*ty, layout);
            }
            for (def_id, layout) in &module.layouts.structs {
                index.struct_layouts.insert(*def_id, layout);
            }
            for (def_id, layout) in &module.layouts.unions {
                index.union_layouts.insert(*def_id, layout);
            }
            for (key, layout) in &module.layouts.struct_instances {
                index
                    .struct_instance_layouts
                    .entry(key.def_id)
                    .or_default()
                    .insert((key.args.clone(), key.const_args.clone()), layout);
                index
                    .struct_instance_layouts_by_def
                    .entry(key.def_id)
                    .or_default()
                    .push(BackendLayoutInstance { key, layout });
            }
            for (key, layout) in &module.layouts.union_instances {
                index
                    .union_instance_layouts
                    .entry(key.def_id)
                    .or_default()
                    .insert((key.args.clone(), key.const_args.clone()), layout);
                index
                    .union_instance_layouts_by_def
                    .entry(key.def_id)
                    .or_default()
                    .push(BackendLayoutInstance { key, layout });
            }
            for item in &module.structs {
                index.structs.insert(item.def_id, item);
            }
            for item in &module.unions {
                index.unions.insert(item.def_id, item);
            }
            for item in &module.struct_instances {
                index
                    .struct_instances
                    .entry(item.def_id)
                    .or_default()
                    .insert((item.args.clone(), item.const_args.clone()), item);
                index
                    .struct_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(item);
            }
            for item in &module.union_instances {
                index
                    .union_instances
                    .entry(item.def_id)
                    .or_default()
                    .insert((item.args.clone(), item.const_args.clone()), item);
                index
                    .union_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(item);
            }
            for item in &module.enums {
                index.enums.insert(item.def_id, item);
                for (variant_index, variant) in item.variants.iter().enumerate() {
                    index.enum_variants.insert(variant.def_id, variant);
                    index.enum_variant_infos.insert(
                        variant.def_id,
                        EnumVariantInfo {
                            owner: item,
                            variant,
                            index: variant_index,
                        },
                    );
                }
            }
            for item in &module.globals {
                index.globals.insert(item.def_id, item);
            }
            for item in &module.global_instances {
                index
                    .global_instances
                    .entry((item.def_id, item.arg_module_id))
                    .or_default()
                    .insert((item.args.clone(), item.const_args.clone()), item);
                index
                    .global_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(item);
            }
            for item in &module.functions {
                index.functions.insert(item.def_id, item);
            }
            for item in &module.function_instances {
                index
                    .function_instances
                    .entry((item.def_id, item.arg_module_id))
                    .or_default()
                    .insert((item.args.clone(), item.const_args.clone()), item);
                index
                    .function_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(item);
            }
            for vtable in &module.trait_object_vtables {
                index
                    .trait_object_vtables_by_object_ty
                    .entry(vtable.key.object_ty)
                    .or_default()
                    .push(vtable);
                if let Some(TyKind::TraitObject { trait_id, .. }) =
                    module.interner.get(vtable.key.object_ty)
                {
                    index
                        .trait_object_vtables_by_trait
                        .entry(*trait_id)
                        .or_default()
                        .push(vtable);
                }
            }
        }
        index
    }

    pub(super) fn module(&self, module_id: ModuleId) -> Option<&'a nia_backend_ir::BackendModule> {
        self.modules.get(&module_id).copied()
    }

    pub(super) fn type_layout(&self, ty: InternedTyId) -> Option<&'a TypeLayout> {
        self.type_layouts.get(&ty).copied()
    }

    pub(super) fn struct_layout(&self, def_id: GlobalDefId) -> Option<&'a StructLayout> {
        self.struct_layouts.get(&def_id).copied()
    }

    pub(super) fn union_layout(&self, def_id: GlobalDefId) -> Option<&'a StructLayout> {
        self.union_layouts.get(&def_id).copied()
    }

    pub(super) fn struct_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&'a StructLayout> {
        self.struct_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
    }

    pub(super) fn union_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&'a StructLayout> {
        self.union_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
    }

    pub(super) fn struct_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&'a nia_backend_ir::BackendStructInstance> {
        self.struct_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
    }

    pub(super) fn union_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&'a nia_backend_ir::BackendUnionInstance> {
        self.union_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
    }

    pub(super) fn function_instance(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&'a BackendFunctionInstance> {
        self.function_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
    }

    pub(super) fn struct_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &BackendLayoutInstance<'a>> {
        self.struct_instance_layouts_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
    }

    pub(super) fn union_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &BackendLayoutInstance<'a>> {
        self.union_instance_layouts_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
    }

    pub(super) fn trait_object_vtables_for_object_ty(
        &self,
        object_ty: InternedTyId,
    ) -> impl Iterator<Item = &'a BackendTraitObjectVtable> {
        self.trait_object_vtables_by_object_ty
            .get(&object_ty)
            .into_iter()
            .flatten()
            .copied()
    }

    pub(super) fn trait_object_vtables_for_trait(
        &self,
        trait_id: TraitId,
    ) -> impl Iterator<Item = &'a BackendTraitObjectVtable> {
        self.trait_object_vtables_by_trait
            .get(&trait_id)
            .into_iter()
            .flatten()
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_backend_ir::{
        BackendComptimeFacts, BackendFunctionInstance, BackendGenericInstantiation, BackendLayouts,
        BackendModule, BackendParam, BackendProgram, BackendStructInstance,
        BackendTraitObjectVtable, BackendTraitObjectVtableKey,
    };
    use nia_ids::{DefId, ModuleId};
    use nia_layout::{StructLayout, TypeLayout};
    use nia_span::Span;
    use nia_ty::{PrimitiveTy, TyInterner, TyKind};

    #[test]
    fn indexes_codegen_lookup_tables_by_exact_keys_and_fallback_groups() {
        let module_id = ModuleId(0);
        let mut interner = TyInterner::new(module_id);
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let function_def = global(module_id, 1);
        let struct_def = global(module_id, 2);
        let enum_def = global(module_id, 3);
        let first_variant = global(module_id, 4);
        let second_variant = global(module_id, 5);
        let trait_def = global(module_id, 6);
        let object_ty = interner.intern(TyKind::TraitObject {
            is_readonly: true,
            trait_id: TraitId::Source(trait_def),
            trait_args: vec![i32_ty],
            associated_type_bindings: Vec::new(),
        });
        let struct_key = BackendStructInstanceKey {
            def_id: struct_def,
            args: vec![i32_ty],
            const_args: Vec::new(),
        };
        let struct_layout = StructLayout {
            layout: TypeLayout { size: 4, align: 4 },
            fields: Vec::new(),
        };
        let function_instance = BackendFunctionInstance {
            def_id: function_def,
            name: "id".to_string(),
            arg_module_id: module_id,
            args: vec![i32_ty],
            const_args: Vec::new(),
            symbol: "id_i32".to_string(),
            params: vec![BackendParam {
                local_id: None,
                name: Some("value".to_string()),
                receiver: None,
                passing_ty: i32_ty,
                local_ty: i32_ty,
                span: Span::default(),
            }],
            return_type: i32_ty,
            is_extern: false,
            is_variadic: false,
            attributes: Vec::new(),
            function_body: None,
            span: Span::default(),
        };
        let vtable = BackendTraitObjectVtable {
            key: BackendTraitObjectVtableKey {
                self_ty: i32_ty,
                object_ty,
            },
            trait_id: TraitId::Source(trait_def),
            trait_args: vec![i32_ty],
            entries: Vec::new(),
            span: Span::default(),
        };
        let program = BackendProgram {
            modules: vec![BackendModule {
                id: module_id,
                name: "main".to_string(),
                interner,
                comptime: BackendComptimeFacts::default(),
                layouts: BackendLayouts {
                    target: nia_layout::TargetDataLayout::LP64,
                    types: vec![
                        (i32_ty, TypeLayout { size: 4, align: 4 }),
                        (object_ty, TypeLayout { size: 16, align: 8 }),
                    ],
                    structs: Vec::new(),
                    unions: Vec::new(),
                    struct_instances: vec![(struct_key.clone(), struct_layout.clone())],
                    union_instances: Vec::new(),
                },
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: vec![BackendStructInstance {
                    def_id: struct_def,
                    name: "Box".to_string(),
                    args: vec![i32_ty],
                    const_args: Vec::new(),
                    symbol: "Box_i32".to_string(),
                    fields: Vec::new(),
                    is_extern: false,
                    span: Span::default(),
                }],
                union_instances: Vec::new(),
                enums: vec![BackendEnum {
                    def_id: enum_def,
                    name: "Choice".to_string(),
                    backing_type: i32_ty,
                    variants: vec![
                        BackendEnumVariant {
                            def_id: first_variant,
                            name: "First".to_string(),
                            value: None,
                            span: Span::default(),
                        },
                        BackendEnumVariant {
                            def_id: second_variant,
                            name: "Second".to_string(),
                            value: Some(7),
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                }],
                globals: Vec::new(),
                global_instances: Vec::new(),
                functions: Vec::new(),
                function_instances: vec![function_instance],
                trait_object_vtables: vec![vtable],
                generic_instantiations: Vec::<BackendGenericInstantiation>::new(),
            }],
        };

        let index = ProgramIndex::new(&program);

        assert!(index.module(module_id).is_some());
        assert_eq!(
            index.type_layout(i32_ty),
            Some(&TypeLayout { size: 4, align: 4 })
        );
        assert_eq!(
            index.struct_instance_layout(struct_def, &[i32_ty], &[]),
            Some(&struct_layout)
        );
        assert!(index.struct_instance(struct_def, &[i32_ty], &[]).is_some());
        assert!(
            index
                .function_instance(function_def, module_id, &[i32_ty], &[])
                .is_some()
        );
        let variant_info = index
            .enum_variant_infos
            .get(&second_variant)
            .expect("second enum variant info");
        assert_eq!(variant_info.owner.def_id, enum_def);
        assert_eq!(variant_info.variant.def_id, second_variant);
        assert_eq!(variant_info.index, 1);
        assert_eq!(
            index
                .trait_object_vtables_for_object_ty(object_ty)
                .map(|vtable| vtable.key.self_ty)
                .collect::<Vec<_>>(),
            vec![i32_ty]
        );
        assert_eq!(
            index
                .trait_object_vtables_for_trait(TraitId::Source(trait_def))
                .map(|vtable| vtable.key.object_ty)
                .collect::<Vec<_>>(),
            vec![object_ty]
        );
    }

    fn global(module_id: ModuleId, def_id: u64) -> GlobalDefId {
        GlobalDefId {
            module_id,
            def_id: DefId(def_id),
        }
    }
}
