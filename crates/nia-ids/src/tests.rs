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
