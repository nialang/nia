use super::*;

#[test]
fn module_id_allocator_issues_dense_local_indices() {
    let mut allocator = ModuleIdAllocator::new();

    assert_eq!(allocator.allocate().local_index(), 0);
    assert_eq!(allocator.allocate().local_index(), 1);
    assert_eq!(std::mem::size_of::<ModuleId>(), 12);
}

#[test]
fn module_id_allocator_clones_keep_dense_slots_without_aliasing_new_generations() {
    let mut allocator = ModuleIdAllocator::new();
    let first = allocator.allocate();
    let mut cloned = allocator.clone();
    let cloned_second = cloned.allocate();
    let original_second = allocator.allocate();

    assert_eq!(first.local_index(), 0);
    assert_eq!(cloned_second.local_index(), 1);
    assert_eq!(original_second.local_index(), 1);
    assert_ne!(cloned_second, original_second);
}

#[test]
fn independent_module_allocators_do_not_alias_handles() {
    let mut first = ModuleIdAllocator::new();
    let mut second = ModuleIdAllocator::new();

    assert_ne!(first.allocate(), second.allocate());
}

#[test]
fn interned_type_handles_are_qualified_by_store_and_index() {
    let first_store = TypeStoreId::fresh();
    let second_store = TypeStoreId::fresh();
    let index = TypeStoreIndex::from_store_index(7);
    let same = InternedTyId::new(first_store, index);

    assert_eq!(index.index(), 7);
    assert_eq!(same, InternedTyId::new(first_store, index));
    assert_ne!(same, InternedTyId::new(second_store, index));
    assert_ne!(
        same,
        InternedTyId::new(first_store, TypeStoreIndex::from_store_index(8))
    );
}

#[test]
fn builtin_trait_method_const_capabilities_are_explicit() {
    for method in BuiltinTraitMethod::ALL {
        let expected = !matches!(method, BuiltinTraitMethod::DerefMut);
        assert_eq!(method.is_const_capable(), expected, "{}", method.name());
    }
}

#[test]
fn builtin_function_const_capabilities_are_explicit() {
    for builtin in BuiltinFunction::ALL {
        let expected = matches!(
            builtin,
            BuiltinFunction::ConstError
                | BuiltinFunction::Trap
                | BuiltinFunction::SizeOf
                | BuiltinFunction::AlignOf
                | BuiltinFunction::Offset
                | BuiltinFunction::Embed
                | BuiltinFunction::CharFromU32
                | BuiltinFunction::SliceLen
                | BuiltinFunction::CallerLocation
                | BuiltinFunction::Splat
                | BuiltinFunction::Extract
                | BuiltinFunction::Insert
                | BuiltinFunction::Bitmask
        );
        assert_eq!(builtin.is_const_capable(), expected, "{}", builtin.name());
    }
}

#[test]
fn builtin_name_registries_are_exhaustive_unique_and_bidirectional() {
    assert_name_registry(&BuiltinType::ALL, BuiltinType::name, BuiltinType::from_name);
    assert_name_registry(
        &BuiltinTypeAnchor::ALL,
        BuiltinTypeAnchor::name,
        BuiltinTypeAnchor::from_name,
    );
    assert_name_registry(
        &BuiltinFunction::ALL,
        BuiltinFunction::name,
        BuiltinFunction::from_name,
    );
    assert_name_registry(
        &BuiltinConstValue::ALL,
        BuiltinConstValue::name,
        BuiltinConstValue::from_name,
    );
    assert_name_registry(
        &ValueBuiltin::ALL,
        ValueBuiltin::name,
        ValueBuiltin::from_name,
    );
    assert_name_registry(
        &LayoutBuiltin::ALL,
        LayoutBuiltin::name,
        LayoutBuiltin::from_name,
    );
    assert_name_registry(
        &BuiltinAssociatedType::ALL,
        BuiltinAssociatedType::name,
        BuiltinAssociatedType::from_name,
    );
    assert_name_registry(
        &BuiltinAssociatedConst::ALL,
        BuiltinAssociatedConst::name,
        BuiltinAssociatedConst::from_name,
    );
    assert_name_registry(
        &BuiltinTrait::ALL,
        BuiltinTrait::name,
        BuiltinTrait::from_name,
    );
    assert_name_registry(
        &BuiltinTraitMethod::ALL,
        BuiltinTraitMethod::name,
        BuiltinTraitMethod::from_name,
    );
}

#[test]
fn builtin_trait_descriptors_cover_each_trait_and_method_once() {
    assert_eq!(BuiltinTrait::DESCRIPTORS.len(), BuiltinTrait::ALL.len());
    assert_eq!(
        BuiltinTraitMethod::DESCRIPTORS.len(),
        BuiltinTraitMethod::ALL.len()
    );

    for method in BuiltinTraitMethod::ALL {
        let descriptor = method.descriptor();
        assert!(
            descriptor.trait_id.required_methods().contains(&method),
            "{} is absent from its owning {} descriptor",
            method.name(),
            descriptor.trait_id.name()
        );
    }
}

#[test]
fn builtin_trait_schema_owns_every_declared_member_once() {
    let mut method_owners = std::collections::HashMap::new();
    let mut associated_types = std::collections::HashSet::new();
    let mut associated_consts = std::collections::HashSet::new();

    for trait_id in BuiltinTrait::ALL {
        for method in trait_id.required_methods() {
            assert_eq!(method.trait_id(), trait_id, "{}", method.name());
            assert_eq!(
                method_owners.insert(*method, trait_id),
                None,
                "{} is declared by more than one builtin trait",
                method.name()
            );
        }
        associated_types.extend(trait_id.associated_types().iter().copied());
        associated_consts.extend(trait_id.associated_consts().iter().copied());

        let mut direct_supertraits = std::collections::HashSet::new();
        for supertrait in trait_id.supertraits() {
            assert_ne!(supertrait.trait_id, trait_id, "{}", trait_id.name());
            assert!(
                direct_supertraits.insert(supertrait.trait_id),
                "{} repeats supertrait {}",
                trait_id.name(),
                supertrait.trait_id.name()
            );
            if supertrait.preserves_trait_args {
                assert_eq!(
                    trait_id.generic_count(),
                    supertrait.trait_id.generic_count(),
                    "{} cannot preserve its arguments for {}",
                    trait_id.name(),
                    supertrait.trait_id.name()
                );
            }
        }
    }

    assert_eq!(method_owners.len(), BuiltinTraitMethod::ALL.len());
    assert!(
        BuiltinTraitMethod::ALL
            .iter()
            .all(|method| method_owners.contains_key(method))
    );
    assert_eq!(
        associated_types,
        BuiltinAssociatedType::ALL.into_iter().collect()
    );
    assert_eq!(
        associated_consts,
        BuiltinAssociatedConst::ALL.into_iter().collect()
    );
}

#[test]
fn builtin_method_descriptor_flags_match_receiver_contracts() {
    for method in BuiltinTraitMethod::ALL {
        assert!(method.param_count() >= 1, "{}", method.name());
        assert!(!(method.is_value_operator() && method.is_place_method()));
        if method.is_value_operator() {
            assert_eq!(method.receiver_kind(), ReceiverKind::Value);
            assert_eq!(method.place_receiver_kind(), None);
        }
        if method.place_receiver_kind().is_some() {
            assert!(method.is_place_method(), "{}", method.name());
        }
    }
}

#[test]
fn target_const_item_names_are_canonical_suffixes() {
    for value in BuiltinConstValue::ALL {
        assert_eq!(
            value.name().strip_prefix("target."),
            Some(value.item_name()),
            "{value:?}"
        );
    }
}

fn assert_name_registry<T>(
    values: &[T],
    name: impl Fn(T) -> &'static str,
    from_name: impl Fn(&str) -> Option<T>,
) where
    T: Copy + Eq + std::fmt::Debug,
{
    let mut names = std::collections::HashSet::new();
    for &value in values {
        let value_name = name(value);
        assert!(
            names.insert(value_name),
            "duplicate builtin name {value_name}"
        );
        assert_eq!(from_name(value_name), Some(value), "{value:?}");
    }
}
