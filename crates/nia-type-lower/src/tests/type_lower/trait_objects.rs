use super::*;

#[test]
fn lowers_trait_object_pointer_types() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
trait Source[T] {
type Item;
}

fn read(source: &Source[i32, Item = i32]) () {}
fn write(source: &mut Source[i32, Item = i32]) () {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let trait_objects = lowered
        .type_uses
        .values()
        .filter_map(|ty| match type_store.get(*ty) {
            Some(TyKind::TraitObject {
                is_readonly,
                trait_args,
                associated_type_bindings,
                ..
            }) => Some((
                *is_readonly,
                trait_args.len(),
                associated_type_bindings.len(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(trait_objects.contains(&(true, 1, 1)), "{trait_objects:?}");
    assert!(trait_objects.contains(&(false, 1, 1)), "{trait_objects:?}");
}

#[test]
fn validates_trait_object_associated_type_bindings() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(
        r#"
trait Source {
type Item;
}

fn unknown(source: &Source[Missing = i32]) () {}
fn duplicate(source: &Source[Item = i32, Item = bool]) () {}
"#,
        symbols.clone(),
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let program_defs = HashMap::from([(module_id, Arc::new(defs.clone()))]);
    let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
    let type_store = nia_ty::TypeStore::new();
    let lowered = lower_module_types_with_context(
        module_id,
        &module,
        &resolved,
        TypeLoweringContext::from_program_defs(
            &type_store,
            ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        )
        .with_symbols(&symbols),
    );
    assert!(
        lowered.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("trait does not define associated type `Missing`")),
        "{:?}",
        lowered.diagnostics
    );
    assert!(
        lowered.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("duplicate associated type binding `Item`")),
        "{:?}",
        lowered.diagnostics
    );
}

#[test]
fn rejects_bare_trait_as_value_type() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module(
        r#"
trait Show {}

fn bad(value: Show) () {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (_type_store, lowered) = lower_test_module(&module, &defs, &resolved);
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("trait types are not valid")),
        "{:?}",
        lowered.diagnostics
    );
}
