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
    for method in BuiltinTraitMethod::DESCRIPTORS
        .iter()
        .map(|(method, _)| *method)
    {
        let expected = !matches!(
            method,
            BuiltinTraitMethod::DerefMut | BuiltinTraitMethod::Ptr | BuiltinTraitMethod::PtrMut
        );
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
                | BuiltinFunction::Splat
                | BuiltinFunction::Extract
                | BuiltinFunction::Insert
                | BuiltinFunction::Bitmask
        );
        assert_eq!(builtin.is_const_capable(), expected, "{}", builtin.name());
    }
}
