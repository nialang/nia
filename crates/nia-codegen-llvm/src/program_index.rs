// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock, RwLockReadGuard},
};

use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendFunctionInstance, BackendModuleStore,
    BackendStructInstanceKey, BackendTraitObjectVtable, CodegenPartition, CodegenUnitId,
    CodegenUnitKey,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_layout::{StructLayout, TypeLayout};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, ConstGenericValue, TraitId, TyKind, TypeEquivalence, TypeStore,
};

type InstanceArgs = (Vec<InternedTyId>, Vec<ConstGenericArg>);
type AggregateInstanceIndex = HashMap<GlobalDefId, HashMap<InstanceArgs, ItemPosition>>;
type GlobalInstanceIndex = HashMap<(GlobalDefId, ModuleId), HashMap<InstanceArgs, ItemPosition>>;
type FunctionInstanceArgs = (
    Option<InternedTyId>,
    Vec<InternedTyId>,
    Vec<ConstGenericArg>,
);
type FunctionInstanceIndex =
    HashMap<(GlobalDefId, ModuleId), HashMap<FunctionInstanceArgs, ItemPosition>>;
type AggregateLayoutIndex = HashMap<GlobalDefId, HashMap<InstanceArgs, LayoutPosition>>;

#[derive(Clone, Copy)]
struct ItemPosition {
    module: ModuleId,
    item: usize,
}

#[derive(Clone, Copy)]
struct LayoutPosition {
    module: ModuleId,
    layout: usize,
}

#[derive(Clone, Copy)]
struct EnumVariantPosition {
    module: ModuleId,
    owner: usize,
    variant: usize,
}

fn owned_item_position(
    def_id: GlobalDefId,
    position: Option<ItemPosition>,
) -> Option<ItemPosition> {
    position.filter(|position| position.module == def_id.module_id)
}

fn owned_layout_position(
    def_id: GlobalDefId,
    position: Option<LayoutPosition>,
) -> Option<LayoutPosition> {
    position.filter(|position| position.module == def_id.module_id)
}

pub(super) struct ProgramIndex {
    modules: Arc<BackendModuleStore>,
    type_store: Arc<TypeStore>,
    tables: RwLock<ProgramIndexTables>,
}

#[derive(Default)]
struct ProgramIndexTables {
    published_modules: HashSet<ModuleId>,
    structs: HashMap<GlobalDefId, ItemPosition>,
    unions: HashMap<GlobalDefId, ItemPosition>,
    struct_instances: AggregateInstanceIndex,
    union_instances: AggregateInstanceIndex,
    enums: HashMap<GlobalDefId, ItemPosition>,
    enum_variants: HashMap<GlobalDefId, EnumVariantPosition>,
    globals: HashMap<GlobalDefId, ItemPosition>,
    global_instances: GlobalInstanceIndex,
    global_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
    functions: HashMap<GlobalDefId, ItemPosition>,
    function_instances: FunctionInstanceIndex,
    function_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
    closure_entries: HashMap<nia_backend_ir::BackendClosureEntryKey, ItemPosition>,
    trait_object_vtables_by_object_ty: HashMap<InternedTyId, Vec<ItemPosition>>,
    trait_object_vtables_by_trait: HashMap<TraitId, Vec<ItemPosition>>,
    trait_object_vtables: HashMap<nia_backend_ir::BackendTraitObjectVtableKey, ItemPosition>,
    type_layouts: HashMap<InternedTyId, LayoutPosition>,
    struct_layouts: HashMap<GlobalDefId, LayoutPosition>,
    union_layouts: HashMap<GlobalDefId, LayoutPosition>,
    enum_layouts: HashMap<GlobalDefId, LayoutPosition>,
    struct_instance_layouts: AggregateLayoutIndex,
    union_instance_layouts: AggregateLayoutIndex,
    struct_instance_layouts_by_def: HashMap<GlobalDefId, Vec<LayoutPosition>>,
    union_instance_layouts_by_def: HashMap<GlobalDefId, Vec<LayoutPosition>>,
    struct_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
    union_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
}

pub(super) struct ProgramIndexPublisher {
    index: Arc<ProgramIndex>,
}

pub(super) struct BackendLayoutInstance<'a> {
    pub(super) key: &'a BackendStructInstanceKey,
    pub(super) layout: &'a StructLayout,
}

struct ProgramTypeEquivalence<'a> {
    type_store: &'a TypeStore,
}

impl TypeEquivalence for ProgramTypeEquivalence<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_store.get(ty)
    }

    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        match (left, right) {
            (
                ArrayLenTy::Builtin {
                    builtin: left_builtin,
                    ty: left_ty,
                },
                ArrayLenTy::Builtin {
                    builtin: right_builtin,
                    ty: right_ty,
                },
            ) => left_builtin == right_builtin && self.same_type_for_equiv(*left_ty, *right_ty),
            _ => left == right,
        }
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        left == right || self.compute_same_type_for_equiv(left, right)
    }

    fn same_const_generic_args_for_equiv(
        &self,
        left: &[ConstGenericArg],
        right: &[ConstGenericArg],
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.same_type_for_equiv(left.ty, right.ty)
                    && match (&left.value, &right.value) {
                        (ConstGenericValue::Int(left), ConstGenericValue::Int(right)) => {
                            left.bits() == right.bits()
                        }
                        (left, right) => left == right,
                    }
            })
    }
}

pub(super) struct EnumVariantInfo<'a> {
    pub(super) owner: &'a BackendEnum,
    pub(super) variant: &'a BackendEnumVariant,
    pub(super) index: usize,
}

impl ProgramIndex {
    pub(super) fn new(
        modules: Arc<BackendModuleStore>,
        type_store: Arc<TypeStore>,
    ) -> (Arc<Self>, ProgramIndexPublisher) {
        let index = Arc::new(Self {
            modules,
            type_store,
            tables: RwLock::new(ProgramIndexTables::default()),
        });
        let publisher = ProgramIndexPublisher {
            index: Arc::clone(&index),
        };
        (index, publisher)
    }

    fn tables(&self) -> RwLockReadGuard<'_, ProgramIndexTables> {
        self.tables.read().expect("program index lock poisoned")
    }

    fn item_owner(position: ItemPosition) -> ModuleId {
        position.module
    }

    pub(super) fn is_published(&self, module_id: ModuleId) -> bool {
        self.tables().published_modules.contains(&module_id)
    }

    pub(super) fn module(&self, module_id: ModuleId) -> Option<&nia_backend_ir::BackendModule> {
        if !self.is_published(module_id) {
            return None;
        }
        self.modules.get(module_id)
    }

    pub(super) fn module_ids(&self) -> &[ModuleId] {
        self.modules.module_ids()
    }

    /// Reports whether this owner belongs to the backend program at all.
    ///
    /// Use this for pure identity checks that reject stale or foreign owners.
    /// It stays true throughout lowering, so a validator running inside the
    /// readiness window cannot mistake an unwritten module for a missing one.
    pub(super) fn is_registered_module(&self, module_id: ModuleId) -> bool {
        self.modules.is_registered(module_id)
    }

    /// Returns a module whose payload is written, published or not.
    ///
    /// Use this when a check reads module facts such as layouts or const-eval
    /// results. A written payload is complete; publication only adds the index
    /// tables. Returns `None` while the slot is still unwritten, which callers
    /// must treat as "defer" rather than "malformed".
    pub(super) fn written_module(
        &self,
        module_id: ModuleId,
    ) -> Option<&nia_backend_ir::BackendModule> {
        self.modules.get(module_id)
    }

    /// Target layouts of every backend module whose payload is written.
    ///
    /// This deliberately spans modules that are written but not yet published,
    /// because artifact target agreement is a whole-store property rather than
    /// an index-visibility one. Registration is a third, earlier state: the
    /// store lists an id before lowering writes its payload, and partition
    /// validation runs inside that window. Such ids are skipped rather than
    /// indexed, since a registered-but-unwritten module has no layout to
    /// compare and `module_at` may not index it.
    pub(super) fn module_target_layouts(
        &self,
    ) -> impl Iterator<Item = nia_layout::TargetDataLayout> + '_ {
        self.module_ids()
            .iter()
            .filter_map(|module_id| self.modules.get(*module_id))
            .map(|module| module.layouts.target)
    }

    pub(super) fn module_for_partition(
        &self,
        partition: &CodegenPartition,
    ) -> &nia_backend_ir::BackendModule {
        let (module_id, ordinal) = match partition.id {
            CodegenUnitId::SourceModule { module_id, ordinal } => (module_id, ordinal),
            CodegenUnitId::CompilerBuiltins => {
                panic!("Nia ICE: compiler builtins partition has no backend module")
            }
        };
        assert!(
            self.is_published(module_id),
            "Nia ICE: codegen partition references an unindexed backend module"
        );
        let module = self.module_at(module_id);
        assert_eq!(
            partition.key,
            CodegenUnitKey::SourceModule {
                source_identity: module.source_identity.clone(),
                ordinal,
            },
            "Nia ICE: codegen partition stable key does not match its backend module"
        );
        module
    }

    fn module_at(&self, module_id: ModuleId) -> &nia_backend_ir::BackendModule {
        self.modules
            .get(module_id)
            .expect("program index position references published module")
    }
}

impl ProgramIndexPublisher {
    pub(super) fn publish(&mut self, module_id: ModuleId) {
        let module = self
            .index
            .modules
            .get(module_id)
            .expect("program index publisher requires a ready backend module");
        let mut index = self
            .index
            .tables
            .write()
            .expect("program index lock poisoned");
        assert!(
            index.published_modules.insert(module_id),
            "Nia ICE: backend module was published to the program index twice"
        );
        for (layout_index, (ty, _)) in module.layouts.types.iter().enumerate() {
            index.type_layouts.insert(
                *ty,
                LayoutPosition {
                    module: module.id,
                    layout: layout_index,
                },
            );
        }
        for (layout_index, (def_id, _)) in module.layouts.structs.iter().enumerate() {
            index.struct_layouts.insert(
                *def_id,
                LayoutPosition {
                    module: module.id,
                    layout: layout_index,
                },
            );
        }
        for (layout_index, (def_id, _)) in module.layouts.unions.iter().enumerate() {
            index.union_layouts.insert(
                *def_id,
                LayoutPosition {
                    module: module.id,
                    layout: layout_index,
                },
            );
        }
        for (layout_index, (def_id, _)) in module.layouts.enums.iter().enumerate() {
            index.enum_layouts.insert(
                *def_id,
                LayoutPosition {
                    module: module.id,
                    layout: layout_index,
                },
            );
        }
        for (layout_index, (key, _)) in module.layouts.struct_instances.iter().enumerate() {
            let position = LayoutPosition {
                module: module.id,
                layout: layout_index,
            };
            index
                .struct_instance_layouts
                .entry(key.def_id)
                .or_default()
                .insert((key.args.clone(), key.const_args.clone()), position);
            index
                .struct_instance_layouts_by_def
                .entry(key.def_id)
                .or_default()
                .push(position);
        }
        for (layout_index, (key, _)) in module.layouts.union_instances.iter().enumerate() {
            let position = LayoutPosition {
                module: module.id,
                layout: layout_index,
            };
            index
                .union_instance_layouts
                .entry(key.def_id)
                .or_default()
                .insert((key.args.clone(), key.const_args.clone()), position);
            index
                .union_instance_layouts_by_def
                .entry(key.def_id)
                .or_default()
                .push(position);
        }
        for (item_index, item) in module.structs.iter().enumerate() {
            index.structs.insert(
                item.def_id,
                ItemPosition {
                    module: module.id,
                    item: item_index,
                },
            );
        }
        for (item_index, item) in module.unions.iter().enumerate() {
            index.unions.insert(
                item.def_id,
                ItemPosition {
                    module: module.id,
                    item: item_index,
                },
            );
        }
        for (item_index, item) in module.struct_instances.iter().enumerate() {
            let position = ItemPosition {
                module: module.id,
                item: item_index,
            };
            index
                .struct_instances
                .entry(item.def_id)
                .or_default()
                .insert((item.args.clone(), item.const_args.clone()), position);
            index
                .struct_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push(position);
        }
        for (item_index, item) in module.union_instances.iter().enumerate() {
            let position = ItemPosition {
                module: module.id,
                item: item_index,
            };
            index
                .union_instances
                .entry(item.def_id)
                .or_default()
                .insert((item.args.clone(), item.const_args.clone()), position);
            index
                .union_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push(position);
        }
        for (item_index, item) in module.enums.iter().enumerate() {
            index.enums.insert(
                item.def_id,
                ItemPosition {
                    module: module.id,
                    item: item_index,
                },
            );
            for (variant_index, variant) in item.variants.iter().enumerate() {
                index.enum_variants.insert(
                    variant.def_id,
                    EnumVariantPosition {
                        module: module.id,
                        owner: item_index,
                        variant: variant_index,
                    },
                );
            }
        }
        for (item_index, item) in module.globals.iter().enumerate() {
            index.globals.insert(
                item.def_id,
                ItemPosition {
                    module: module.id,
                    item: item_index,
                },
            );
        }
        for (item_index, item) in module.global_instances.iter().enumerate() {
            let position = ItemPosition {
                module: module.id,
                item: item_index,
            };
            index
                .global_instances
                .entry((item.def_id, item.arg_module_id))
                .or_default()
                .insert((item.args.clone(), item.const_args.clone()), position);
            index
                .global_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push(position);
        }
        for (item_index, item) in module.functions.iter().enumerate() {
            index.functions.insert(
                item.def_id,
                ItemPosition {
                    module: module.id,
                    item: item_index,
                },
            );
        }
        for (item_index, item) in module.function_instances.iter().enumerate() {
            let position = ItemPosition {
                module: module.id,
                item: item_index,
            };
            index
                .function_instances
                .entry((item.def_id, item.arg_module_id))
                .or_default()
                .insert(
                    (item.self_arg, item.args.clone(), item.const_args.clone()),
                    position,
                );
            index
                .function_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push(position);
        }
        for (item_index, entry) in module.closure_entries.iter().enumerate() {
            index.closure_entries.insert(
                entry.key.clone(),
                ItemPosition {
                    module: module.id,
                    item: item_index,
                },
            );
        }
        for (item_index, vtable) in module.trait_object_vtables.iter().enumerate() {
            let position = ItemPosition {
                module: module.id,
                item: item_index,
            };
            index
                .trait_object_vtables
                .insert(vtable.key.clone(), position);
            index
                .trait_object_vtables_by_object_ty
                .entry(vtable.key.object_ty)
                .or_default()
                .push(position);
            if let Some(TyKind::TraitObject { trait_id, .. }) =
                self.index.ty_kind(vtable.key.object_ty)
            {
                index
                    .trait_object_vtables_by_trait
                    .entry(*trait_id)
                    .or_default()
                    .push(position);
            }
        }
    }
}

impl ProgramIndex {
    pub(super) fn struct_owner(&self, def_id: GlobalDefId) -> Option<ModuleId> {
        owned_item_position(def_id, self.tables().structs.get(&def_id).copied())
            .map(Self::item_owner)
    }

    pub(super) fn union_owner(&self, def_id: GlobalDefId) -> Option<ModuleId> {
        owned_item_position(def_id, self.tables().unions.get(&def_id).copied())
            .map(Self::item_owner)
    }

    pub(super) fn struct_instance_owner(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<ModuleId> {
        self.tables()
            .struct_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
            .or_else(|| {
                self.tables()
                    .struct_instances_by_def
                    .get(&def_id)
                    .into_iter()
                    .flatten()
                    .find(|position| {
                        let item = &self.module_at(position.module).struct_instances[position.item];
                        self.instance_args_match(args, const_args, &item.args, &item.const_args)
                    })
                    .copied()
            })
            .map(Self::item_owner)
    }

    pub(super) fn union_instance_owner(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<ModuleId> {
        self.tables()
            .union_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
            .or_else(|| {
                self.tables()
                    .union_instances_by_def
                    .get(&def_id)
                    .into_iter()
                    .flatten()
                    .find(|position| {
                        let item = &self.module_at(position.module).union_instances[position.item];
                        self.instance_args_match(args, const_args, &item.args, &item.const_args)
                    })
                    .copied()
            })
            .map(Self::item_owner)
    }

    pub(super) fn global_owner(&self, def_id: GlobalDefId) -> Option<ModuleId> {
        owned_item_position(def_id, self.tables().globals.get(&def_id).copied())
            .map(Self::item_owner)
    }

    pub(super) fn global_instance_owner(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<ModuleId> {
        self.tables()
            .global_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
            .or_else(|| {
                self.tables()
                    .global_instances_by_def
                    .get(&def_id)
                    .into_iter()
                    .flatten()
                    .find(|position| {
                        let item = &self.module_at(position.module).global_instances[position.item];
                        item.arg_module_id == arg_module_id
                            && self.instance_args_match(
                                args,
                                const_args,
                                &item.args,
                                &item.const_args,
                            )
                    })
                    .copied()
            })
            .map(Self::item_owner)
    }

    pub(super) fn function_owner(&self, def_id: GlobalDefId) -> Option<ModuleId> {
        owned_item_position(def_id, self.tables().functions.get(&def_id).copied())
            .map(Self::item_owner)
    }

    pub(super) fn function_instance_owner(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<ModuleId> {
        self.tables()
            .function_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(self_arg, args.to_vec(), const_args.to_vec())))
            .copied()
            .or_else(|| {
                self.tables()
                    .function_instances_by_def
                    .get(&def_id)
                    .into_iter()
                    .flatten()
                    .find(|position| {
                        let item =
                            &self.module_at(position.module).function_instances[position.item];
                        item.arg_module_id == arg_module_id
                            && self.function_instance_args_match(
                                self_arg,
                                args,
                                const_args,
                                item.self_arg,
                                &item.args,
                                &item.const_args,
                            )
                    })
                    .copied()
            })
            .map(Self::item_owner)
    }

    pub(super) fn trait_object_vtable_owner(
        &self,
        key: &nia_backend_ir::BackendTraitObjectVtableKey,
    ) -> Option<ModuleId> {
        self.tables()
            .trait_object_vtables
            .get(key)
            .copied()
            .map(Self::item_owner)
    }

    pub(super) fn type_store(&self) -> &TypeStore {
        &self.type_store
    }

    fn instance_args_match(
        &self,
        left_args: &[InternedTyId],
        left_const_args: &[ConstGenericArg],
        right_args: &[InternedTyId],
        right_const_args: &[ConstGenericArg],
    ) -> bool {
        let equivalence = ProgramTypeEquivalence {
            type_store: &self.type_store,
        };
        equivalence.same_type_args_for_equiv(left_args, right_args)
            && equivalence.same_const_generic_args_for_equiv(left_const_args, right_const_args)
    }

    fn function_instance_args_match(
        &self,
        left_self_arg: Option<InternedTyId>,
        left_args: &[InternedTyId],
        left_const_args: &[ConstGenericArg],
        right_self_arg: Option<InternedTyId>,
        right_args: &[InternedTyId],
        right_const_args: &[ConstGenericArg],
    ) -> bool {
        match (left_self_arg, right_self_arg) {
            (Some(left), Some(right)) if !self.instance_args_match(&[left], &[], &[right], &[]) => {
                return false;
            }
            (None, None) => {}
            _ => return false,
        }
        self.instance_args_match(left_args, left_const_args, right_args, right_const_args)
    }

    pub(super) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_store.get(ty)
    }

    pub(super) fn type_layout(&self, ty: InternedTyId) -> Option<&TypeLayout> {
        let position = self.tables().type_layouts.get(&ty).copied()?;
        Some(&self.module_at(position.module).layouts.types[position.layout].1)
    }

    pub(super) fn struct_layout(&self, def_id: GlobalDefId) -> Option<&StructLayout> {
        let position =
            owned_layout_position(def_id, self.tables().struct_layouts.get(&def_id).copied())?;
        Some(&self.module_at(position.module).layouts.structs[position.layout].1)
    }

    pub(super) fn union_layout(&self, def_id: GlobalDefId) -> Option<&StructLayout> {
        let position =
            owned_layout_position(def_id, self.tables().union_layouts.get(&def_id).copied())?;
        Some(&self.module_at(position.module).layouts.unions[position.layout].1)
    }

    pub(super) fn enum_layout(&self, def_id: GlobalDefId) -> Option<&nia_layout::EnumLayout> {
        let position =
            owned_layout_position(def_id, self.tables().enum_layouts.get(&def_id).copied())?;
        Some(&self.module_at(position.module).layouts.enums[position.layout].1)
    }

    pub(super) fn struct_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&StructLayout> {
        let position = self
            .tables()
            .struct_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(&(args.to_vec(), const_args.to_vec())))
            .copied()?;
        Some(&self.module_at(position.module).layouts.struct_instances[position.layout].1)
    }

    pub(super) fn union_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&StructLayout> {
        let position = self
            .tables()
            .union_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(&(args.to_vec(), const_args.to_vec())))
            .copied()?;
        Some(&self.module_at(position.module).layouts.union_instances[position.layout].1)
    }

    pub(super) fn struct_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&nia_backend_ir::BackendStructInstance> {
        let position = self
            .tables()
            .struct_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()?;
        Some(&self.module_at(position.module).struct_instances[position.item])
    }

    pub(super) fn union_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&nia_backend_ir::BackendUnionInstance> {
        let position = self
            .tables()
            .union_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()?;
        Some(&self.module_at(position.module).union_instances[position.item])
    }

    pub(super) fn function_instance(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&BackendFunctionInstance> {
        let position = self
            .tables()
            .function_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(self_arg, args.to_vec(), const_args.to_vec())))
            .copied()
            .or_else(|| {
                self.tables()
                    .function_instances_by_def
                    .get(&def_id)
                    .into_iter()
                    .flatten()
                    .find(|position| {
                        let item =
                            &self.module_at(position.module).function_instances[position.item];
                        item.arg_module_id == arg_module_id
                            && self.function_instance_args_match(
                                self_arg,
                                args,
                                const_args,
                                item.self_arg,
                                &item.args,
                                &item.const_args,
                            )
                    })
                    .copied()
            })?;
        Some(&self.module_at(position.module).function_instances[position.item])
    }

    pub(super) fn struct_item(
        &self,
        def_id: GlobalDefId,
    ) -> Option<&nia_backend_ir::BackendStruct> {
        let position = owned_item_position(def_id, self.tables().structs.get(&def_id).copied())?;
        Some(&self.module_at(position.module).structs[position.item])
    }

    pub(super) fn has_struct(&self, def_id: GlobalDefId) -> bool {
        self.struct_item(def_id).is_some()
    }

    pub(super) fn union_item(&self, def_id: GlobalDefId) -> Option<&nia_backend_ir::BackendUnion> {
        let position = owned_item_position(def_id, self.tables().unions.get(&def_id).copied())?;
        Some(&self.module_at(position.module).unions[position.item])
    }

    pub(super) fn has_union(&self, def_id: GlobalDefId) -> bool {
        self.union_item(def_id).is_some()
    }

    pub(super) fn has_struct_instances(&self, def_id: GlobalDefId) -> bool {
        self.tables().struct_instances_by_def.contains_key(&def_id)
    }

    pub(super) fn struct_instances_for(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &nia_backend_ir::BackendStructInstance> {
        let positions = self
            .tables()
            .struct_instances_by_def
            .get(&def_id)
            .cloned()
            .unwrap_or_default();
        positions
            .into_iter()
            .map(|position| &self.module_at(position.module).struct_instances[position.item])
    }

    pub(super) fn has_union_instances(&self, def_id: GlobalDefId) -> bool {
        self.tables().union_instances_by_def.contains_key(&def_id)
    }

    pub(super) fn union_instances_for(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &nia_backend_ir::BackendUnionInstance> {
        let positions = self
            .tables()
            .union_instances_by_def
            .get(&def_id)
            .cloned()
            .unwrap_or_default();
        positions
            .into_iter()
            .map(|position| &self.module_at(position.module).union_instances[position.item])
    }

    pub(super) fn has_enum(&self, def_id: GlobalDefId) -> bool {
        self.enum_item(def_id).is_some()
    }

    pub(super) fn enum_item(&self, def_id: GlobalDefId) -> Option<&BackendEnum> {
        let position = owned_item_position(def_id, self.tables().enums.get(&def_id).copied())?;
        Some(&self.module_at(position.module).enums[position.item])
    }

    pub(super) fn enum_variant_info(&self, def_id: GlobalDefId) -> Option<EnumVariantInfo<'_>> {
        let position = self.tables().enum_variants.get(&def_id).copied()?;
        let owner = &self.module_at(position.module).enums[position.owner];
        if owner.def_id.module_id != def_id.module_id {
            return None;
        }
        Some(EnumVariantInfo {
            owner,
            variant: &owner.variants[position.variant],
            index: position.variant,
        })
    }

    pub(super) fn has_enum_variant(&self, def_id: GlobalDefId) -> bool {
        self.enum_variant_info(def_id).is_some()
    }

    pub(super) fn has_enum_variant_owner_mismatch(&self, def_id: GlobalDefId) -> bool {
        let Some(position) = self.tables().enum_variants.get(&def_id).copied() else {
            return false;
        };
        self.module_at(position.module).enums[position.owner]
            .def_id
            .module_id
            != def_id.module_id
    }

    pub(super) fn global(&self, def_id: GlobalDefId) -> Option<&nia_backend_ir::BackendGlobal> {
        let position = owned_item_position(def_id, self.tables().globals.get(&def_id).copied())?;
        Some(&self.module_at(position.module).globals[position.item])
    }

    pub(super) fn has_global(&self, def_id: GlobalDefId) -> bool {
        self.global(def_id).is_some()
    }

    pub(super) fn global_instance(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&nia_backend_ir::BackendGlobalInstance> {
        let position = self
            .tables()
            .global_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
            .or_else(|| {
                self.tables()
                    .global_instances_by_def
                    .get(&def_id)
                    .into_iter()
                    .flatten()
                    .find(|position| {
                        let item = &self.module_at(position.module).global_instances[position.item];
                        item.arg_module_id == arg_module_id
                            && self.instance_args_match(
                                args,
                                const_args,
                                &item.args,
                                &item.const_args,
                            )
                    })
                    .copied()
            })?;
        Some(&self.module_at(position.module).global_instances[position.item])
    }

    pub(super) fn function(&self, def_id: GlobalDefId) -> Option<&nia_backend_ir::BackendFunction> {
        let position = owned_item_position(def_id, self.tables().functions.get(&def_id).copied())?;
        Some(&self.module_at(position.module).functions[position.item])
    }

    pub(super) fn closure_entry(
        &self,
        key: &nia_backend_ir::BackendClosureEntryKey,
    ) -> Option<&nia_backend_ir::BackendClosureEntry> {
        let position = self.tables().closure_entries.get(key).copied()?;
        Some(&self.module_at(position.module).closure_entries[position.item])
    }

    pub(super) fn has_function(&self, def_id: GlobalDefId) -> bool {
        self.function(def_id).is_some()
    }

    pub(super) fn function_instances_for(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &BackendFunctionInstance> {
        let positions = self
            .tables()
            .function_instances_by_def
            .get(&def_id)
            .cloned()
            .unwrap_or_default();
        positions
            .into_iter()
            .map(|position| &self.module_at(position.module).function_instances[position.item])
    }

    pub(super) fn function_instance_count(&self, def_id: GlobalDefId) -> usize {
        self.tables()
            .function_instances_by_def
            .get(&def_id)
            .map_or(0, Vec::len)
    }

    pub(super) fn struct_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = BackendLayoutInstance<'_>> {
        let positions = self
            .tables()
            .struct_instance_layouts_by_def
            .get(&def_id)
            .cloned()
            .unwrap_or_default();
        positions.into_iter().map(|position| {
            let (key, layout) =
                &self.module_at(position.module).layouts.struct_instances[position.layout];
            BackendLayoutInstance { key, layout }
        })
    }

    pub(super) fn union_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = BackendLayoutInstance<'_>> {
        let positions = self
            .tables()
            .union_instance_layouts_by_def
            .get(&def_id)
            .cloned()
            .unwrap_or_default();
        positions.into_iter().map(|position| {
            let (key, layout) =
                &self.module_at(position.module).layouts.union_instances[position.layout];
            BackendLayoutInstance { key, layout }
        })
    }

    pub(super) fn trait_object_vtables_for_object_ty(
        &self,
        object_ty: InternedTyId,
    ) -> impl Iterator<Item = &BackendTraitObjectVtable> {
        let positions = self
            .tables()
            .trait_object_vtables_by_object_ty
            .get(&object_ty)
            .cloned()
            .unwrap_or_default();
        positions
            .into_iter()
            .map(|position| &self.module_at(position.module).trait_object_vtables[position.item])
    }

    pub(super) fn trait_object_vtable(
        &self,
        key: &nia_backend_ir::BackendTraitObjectVtableKey,
    ) -> Option<&BackendTraitObjectVtable> {
        let position = self.tables().trait_object_vtables.get(key).copied()?;
        Some(&self.module_at(position.module).trait_object_vtables[position.item])
    }

    pub(super) fn trait_object_vtables_for_trait(
        &self,
        trait_id: TraitId,
    ) -> impl Iterator<Item = &BackendTraitObjectVtable> {
        let positions = self
            .tables()
            .trait_object_vtables_by_trait
            .get(&trait_id)
            .cloned()
            .unwrap_or_default();
        positions
            .into_iter()
            .map(|position| &self.module_at(position.module).trait_object_vtables[position.item])
    }

    /// Iterates every emitted trait-object vtable.
    ///
    /// A dynamic call on a supertrait view can use metadata produced for the
    /// original (more-derived) object. In that case the call's `object_ty`
    /// names the upcast target, while no standalone vtable is indexed under
    /// that target type. Validators must still be able to inspect the source
    /// vtable entries rather than treating the upcast as an absent table.
    pub(super) fn trait_object_vtables(&self) -> impl Iterator<Item = &BackendTraitObjectVtable> {
        self.module_ids()
            .iter()
            .copied()
            .filter(|module_id| self.is_published(*module_id))
            .flat_map(move |module_id| self.module_at(module_id).trait_object_vtables.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_backend_ir::{
        BackendConstFacts, BackendFunctionInstance, BackendGenericInstantiation,
        BackendGlobalInstance, BackendLayouts, BackendModule, BackendParam, BackendProgram,
        BackendStructInstance, BackendTraitObjectVtable, BackendTraitObjectVtableKey,
    };
    use nia_ids::{DefId, ModuleId, ModuleIdAllocator};
    use nia_layout::{StructLayout, TypeLayout};
    use nia_span::Span;
    use nia_symbol::{SymbolId, stable_hash};
    use nia_ty::{PrimitiveTy, TyKind, TypeStore};

    fn sym(text: &str) -> SymbolId {
        SymbolId::from_stable_hash(stable_hash(text))
    }

    fn enum_module(
        module_id: ModuleId,
        ty: InternedTyId,
        def_id: GlobalDefId,
        name: &str,
    ) -> BackendModule {
        BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new(name),
            name: name.to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(ty, TypeLayout { size: 4, align: 4 })],
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
            enums: vec![BackendEnum {
                def_id,
                name: sym(name),
                backing_type: ty,
                variants: Vec::new(),
                span: Span::default(),
            }],
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }
    }

    #[test]
    fn publishes_ready_modules_out_of_order_for_concurrent_readers() {
        let mut module_ids = ModuleIdAllocator::new();
        let first = module_ids.allocate();
        let second = module_ids.allocate();
        let first_def = global(first, 1);
        let second_def = global(second, 1);
        let type_store = TypeStore::new();
        let interner = type_store.append_for_module(first);
        let first_ty = interner.primitive(PrimitiveTy::I32);
        let second_ty = interner.primitive(PrimitiveTy::U32);
        let program = BackendProgram::new(vec![
            enum_module(first, first_ty, first_def, "first"),
            enum_module(second, second_ty, second_def, "second"),
        ]);
        let (index, mut publisher) =
            ProgramIndex::new(program.module_store(), Arc::new(type_store));

        publisher.publish(second);
        assert!(!index.has_enum(first_def));
        assert!(index.module(first).is_none());
        assert!(index.has_enum(second_def));
        assert_eq!(index.module(second).map(|module| module.id), Some(second));
        assert_eq!(
            index.enum_item(second_def).map(|item| item.def_id),
            Some(second_def)
        );

        let reader = Arc::clone(&index);
        let read = std::thread::spawn(move || {
            for _ in 0..1_000 {
                assert_eq!(
                    reader.enum_item(second_def).map(|item| item.def_id),
                    Some(second_def)
                );
            }
        });
        publisher.publish(first);
        read.join().expect("concurrent program index reader");

        assert!(index.has_enum(first_def));
        assert_eq!(
            index
                .tables()
                .enums
                .get(&first_def)
                .map(|position| position.module),
            Some(first)
        );
        assert_eq!(
            index
                .tables()
                .enums
                .get(&second_def)
                .map(|position| position.module),
            Some(second)
        );
    }

    #[test]
    fn target_layout_iteration_includes_unpublished_modules() {
        let mut module_ids = ModuleIdAllocator::new();
        let first = module_ids.allocate();
        let second = module_ids.allocate();
        let type_store = TypeStore::new();
        let interner = type_store.append_for_module(first);
        let first_ty = interner.primitive(PrimitiveTy::I32);
        let second_ty = interner.primitive(PrimitiveTy::U32);
        let first_module = enum_module(first, first_ty, global(first, 1), "first");
        let mut second_module = enum_module(second, second_ty, global(second, 1), "second");
        second_module.layouts.target = nia_layout::TargetDataLayout {
            pointer_size: 4,
            pointer_align: 4,
        };
        let program = BackendProgram::new(vec![first_module, second_module]);
        let (index, _publisher) = ProgramIndex::new(program.module_store(), Arc::new(type_store));

        assert!(index.module(first).is_none());
        assert!(index.module(second).is_none());
        assert_eq!(
            index.module_target_layouts().collect::<Vec<_>>(),
            vec![
                nia_layout::TargetDataLayout::LP64,
                nia_layout::TargetDataLayout {
                    pointer_size: 4,
                    pointer_align: 4,
                },
            ]
        );
    }

    /// The three readiness states must stay independently observable, so
    /// validation can reject a foreign owner without rejecting one that is
    /// merely mid-lowering.
    #[test]
    fn separates_registered_written_and_published_module_states() {
        let mut module_ids = ModuleIdAllocator::new();
        let written = module_ids.allocate();
        let unwritten = module_ids.allocate();
        let foreign = module_ids.allocate();
        let type_store = TypeStore::new();
        let interner = type_store.append_for_module(written);
        let ty = interner.primitive(PrimitiveTy::I32);
        drop(interner);
        let store = Arc::new(nia_backend_ir::BackendModuleStore::new([
            written, unwritten,
        ]));
        store.publish(enum_module(written, ty, global(written, 1), "written"));
        let (index, mut publisher) = ProgramIndex::new(Arc::clone(&store), Arc::new(type_store));

        // Written but not yet published.
        assert!(index.is_registered_module(written));
        assert!(index.written_module(written).is_some());
        assert!(index.module(written).is_none());

        // Registered, payload still absent.
        assert!(index.is_registered_module(unwritten));
        assert!(index.written_module(unwritten).is_none());
        assert!(index.module(unwritten).is_none());

        // Never part of this program.
        assert!(!index.is_registered_module(foreign));
        assert!(index.written_module(foreign).is_none());
        assert!(index.module(foreign).is_none());

        publisher.publish(written);
        assert!(index.module(written).is_some());
    }

    /// Registration precedes payload writing, and partition validation runs in
    /// that window. Target-layout iteration must skip an unwritten slot instead
    /// of indexing it, which previously aborted codegen with an ICE whenever a
    /// partition became ready before the last module finished lowering.
    #[test]
    fn target_layout_iteration_skips_registered_but_unwritten_modules() {
        let mut module_ids = ModuleIdAllocator::new();
        let first = module_ids.allocate();
        let second = module_ids.allocate();
        let type_store = TypeStore::new();
        let interner = type_store.append_for_module(first);
        let first_ty = interner.primitive(PrimitiveTy::I32);
        drop(interner);
        let store = Arc::new(nia_backend_ir::BackendModuleStore::new([first, second]));
        store.publish(enum_module(first, first_ty, global(first, 1), "first"));
        let (index, _publisher) = ProgramIndex::new(Arc::clone(&store), Arc::new(type_store));

        assert!(store.get(second).is_none());
        assert_eq!(
            index.module_target_layouts().collect::<Vec<_>>(),
            vec![nia_layout::TargetDataLayout::LP64]
        );
    }

    #[test]
    fn indexes_codegen_lookup_tables_by_exact_keys_and_fallback_groups() {
        let mut module_ids = ModuleIdAllocator::new();
        let semantic_module_id = module_ids.allocate();
        let module_id = module_ids.allocate();
        let type_store = TypeStore::new();
        let interner = type_store.append_for_module(module_id);
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let function_def = global(semantic_module_id, 1);
        let struct_def = global(semantic_module_id, 2);
        let enum_def = global(semantic_module_id, 3);
        let first_variant = global(semantic_module_id, 4);
        let second_variant = global(semantic_module_id, 5);
        let trait_def = global(semantic_module_id, 6);
        let object_ty = interner.intern(TyKind::TraitObject {
            is_readonly: true,
            trait_id: TraitId::Source(trait_def),
            trait_args: vec![i32_ty],
            trait_const_args: Vec::new(),
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
            name: sym("id"),
            arg_module_id: module_id,
            self_arg: None,
            args: vec![i32_ty],
            const_args: Vec::new(),
            symbol: "id_i32".to_string(),
            params: vec![BackendParam {
                local_id: None,
                name: Some(sym("value")),
                receiver: None,
                passing_ty: i32_ty,
                local_ty: i32_ty,
                span: Span::default(),
            }],
            return_type: i32_ty,
            is_extern: false,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
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
            trait_const_args: Vec::new(),
            entries: Vec::new(),
            span: Span::default(),
        };
        let program = BackendProgram {
            modules: vec![BackendModule {
                id: module_id,
                source_identity: nia_source::SourceIdentity::new("main"),
                name: "main".to_string(),
                const_eval: BackendConstFacts::default(),
                layouts: BackendLayouts {
                    target: nia_layout::TargetDataLayout::LP64,
                    types: vec![
                        (i32_ty, TypeLayout { size: 4, align: 4 }),
                        (object_ty, TypeLayout { size: 16, align: 8 }),
                    ],
                    structs: Vec::new(),
                    unions: Vec::new(),
                    enums: Vec::new(),
                    struct_instances: vec![(struct_key.clone(), struct_layout.clone())],
                    union_instances: Vec::new(),
                },
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: vec![BackendStructInstance {
                    def_id: struct_def,
                    name: sym("Box"),
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
                    name: sym("Choice"),
                    backing_type: i32_ty,
                    variants: vec![
                        BackendEnumVariant {
                            def_id: first_variant,
                            name: sym("First"),
                            value: None,
                            payload: nia_backend_ir::BackendEnumVariantPayload::Unit,
                            span: Span::default(),
                        },
                        BackendEnumVariant {
                            def_id: second_variant,
                            name: sym("Second"),
                            value: Some(7),
                            payload: nia_backend_ir::BackendEnumVariantPayload::Unit,
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                }],
                globals: Vec::new(),
                global_instances: Vec::new(),
                functions: Vec::new(),
                function_instances: vec![function_instance],
                closure_entries: Vec::new(),
                trait_object_vtables: vec![vtable],
                generic_instantiations: Vec::<BackendGenericInstantiation>::new(),
            }]
            .into(),
        };

        let (index, mut publisher) =
            ProgramIndex::new(program.module_store(), Arc::new(type_store));
        assert!(!index.is_published(module_id));
        publisher.publish(module_id);
        assert!(index.is_published(module_id));

        assert_eq!(
            index
                .tables()
                .struct_instance_layouts
                .get(&struct_def)
                .and_then(|layouts| layouts.get(&(vec![i32_ty], Vec::new())))
                .map(|position| position.module),
            Some(module_id)
        );
        assert_eq!(
            index
                .tables()
                .function_instances
                .get(&(function_def, module_id))
                .and_then(|instances| { instances.get(&(None, vec![i32_ty], Vec::new())) })
                .map(|position| position.module),
            Some(module_id)
        );
        assert_eq!(
            index
                .tables()
                .enum_variants
                .get(&second_variant)
                .map(|position| position.module),
            Some(module_id)
        );
        assert!(std::ptr::eq(
            index.module(module_id).expect("indexed module"),
            &program.modules[0]
        ));
        assert_eq!(
            index.type_layout(i32_ty),
            Some(&TypeLayout { size: 4, align: 4 })
        );
        assert_eq!(
            index.struct_instance_layout(struct_def, &[i32_ty], &[]),
            Some(&struct_layout)
        );
        assert!(index.struct_instance(struct_def, &[i32_ty], &[]).is_some());
        assert_eq!(
            index.struct_instance_owner(struct_def, &[i32_ty], &[]),
            Some(module_id)
        );
        assert!(std::ptr::eq(
            index
                .function_instance(function_def, module_id, None, &[i32_ty], &[])
                .expect("indexed function instance"),
            &program.modules[0].function_instances[0]
        ));
        assert_eq!(
            index.function_instance_owner(function_def, module_id, None, &[i32_ty], &[]),
            Some(module_id)
        );
        let variant_info = index
            .enum_variant_info(second_variant)
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
        assert_eq!(
            index.trait_object_vtable_owner(&program.modules[0].trait_object_vtables[0].key),
            Some(module_id)
        );
    }

    #[test]
    fn instance_lookup_matches_semantically_equal_rebuilt_arguments() {
        let mut module_ids = ModuleIdAllocator::new();
        let owner_module = module_ids.allocate();
        let argument_module = module_ids.allocate();
        let type_store = TypeStore::new();
        let owner_append = type_store.append_for_module(owner_module);
        let argument_append = type_store.append_for_module(argument_module);
        let owner_i32 = owner_append.primitive(PrimitiveTy::I32);
        let argument_i32 = argument_append.primitive(PrimitiveTy::I32);
        let owner_usize = owner_append.primitive(PrimitiveTy::Usize);
        let argument_usize = argument_append.primitive(PrimitiveTy::Usize);
        let def_id = global(owner_module, 77);
        let function_def = global(owner_module, 78);
        let stored_const = ConstGenericArg {
            ty: owner_usize,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed(13)),
        };
        let query_const = ConstGenericArg {
            ty: argument_usize,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(13)),
        };
        let module = BackendModule {
            id: owner_module,
            source_identity: nia_source::SourceIdentity::new("owner"),
            name: "owner".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
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
            global_instances: vec![BackendGlobalInstance {
                def_id,
                name: sym("VALUE"),
                arg_module_id: argument_module,
                args: vec![owner_i32],
                const_args: vec![stored_const.clone()],
                symbol: "VALUE_i32_13".to_string(),
                ty: owner_i32,
                is_let: true,
                init: None,
                span: Span::default(),
            }],
            functions: Vec::new(),
            function_instances: vec![BackendFunctionInstance {
                def_id: function_def,
                name: sym("compute"),
                arg_module_id: argument_module,
                self_arg: None,
                args: vec![owner_i32],
                const_args: vec![stored_const],
                symbol: "compute_i32_13".to_string(),
                params: Vec::new(),
                return_type: owner_i32,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: None,
                span: Span::default(),
            }],
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        };
        let program = BackendProgram::new(vec![module]);
        let (index, mut publisher) =
            ProgramIndex::new(program.module_store(), Arc::new(type_store));
        publisher.publish(owner_module);

        let item = index
            .global_instance(
                def_id,
                argument_module,
                &[argument_i32],
                std::slice::from_ref(&query_const),
            )
            .expect("semantic fallback should find the rebuilt global instance");
        assert_eq!(item.def_id, def_id);
        assert_eq!(
            index.global_instance_owner(
                def_id,
                argument_module,
                &[argument_i32],
                std::slice::from_ref(&query_const),
            ),
            Some(owner_module)
        );
        let function_item = index
            .function_instance(
                function_def,
                argument_module,
                None,
                &[argument_i32],
                std::slice::from_ref(&query_const),
            )
            .expect("semantic fallback should find the rebuilt function instance");
        assert_eq!(function_item.def_id, function_def);
        assert_eq!(
            index.function_instance_owner(
                function_def,
                argument_module,
                None,
                &[argument_i32],
                &[query_const],
            ),
            Some(owner_module)
        );
    }

    #[test]
    fn position_index_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<ProgramIndex>();
    }

    #[test]
    fn indexed_nominal_positions_require_the_definition_module_owner() {
        let mut module_ids = ModuleIdAllocator::new();
        let owner = module_ids.allocate();
        let foreign = module_ids.allocate();
        let position = ItemPosition {
            module: owner,
            item: 0,
        };
        let layout = LayoutPosition {
            module: owner,
            layout: 0,
        };

        assert!(super::owned_item_position(global(owner, 1), Some(position)).is_some());
        assert!(super::owned_item_position(global(foreign, 1), Some(position)).is_none());
        assert!(super::owned_layout_position(global(owner, 1), Some(layout)).is_some());
        assert!(super::owned_layout_position(global(foreign, 1), Some(layout)).is_none());
    }

    #[test]
    fn resolves_types_without_a_program_module_view() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = TypeStore::new();
        let ty = {
            let interner = type_store.append_for_module(module_id);
            let elem = interner.primitive(PrimitiveTy::U32);
            interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem,
            })
        };
        let program = BackendProgram::new(Vec::new());
        let (index, _publisher) = ProgramIndex::new(program.module_store(), Arc::new(type_store));

        assert!(matches!(
            index.ty_kind(ty),
            Some(TyKind::Pointer {
                is_readonly: true,
                ..
            })
        ));
    }

    fn global(module_id: ModuleId, def_id: u64) -> GlobalDefId {
        GlobalDefId {
            module_id,
            def_id: DefId(def_id),
        }
    }
}
