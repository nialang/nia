use super::*;

#[test]
fn std_builtin_source_declarations_match_rust_descriptors() {
    let declarations = std_builtin_source_declarations();

    let expected_functions = BuiltinFunction::ALL
        .iter()
        .map(|builtin| builtin.name().to_string())
        .collect::<BTreeSet<_>>();
    let actual_functions = declarations
        .functions
        .iter()
        .map(|symbol| symbol_debug_text(*symbol))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_functions, expected_functions);

    let expected_traits = BuiltinTrait::ALL
        .iter()
        .map(|builtin| builtin.name())
        .collect::<BTreeSet<_>>();
    let actual_traits = declarations
        .traits
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_traits, expected_traits);

    let expected_types = BuiltinType::ALL
        .iter()
        .map(|builtin| builtin.name().to_string())
        .collect::<BTreeSet<_>>();
    let actual_types = declarations
        .types
        .iter()
        .map(|symbol| symbol_debug_text(*symbol))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_types, expected_types);

    let expected_type_anchors = BuiltinTypeAnchor::ALL
        .iter()
        .map(|builtin| builtin.name().to_string())
        .collect::<BTreeSet<_>>();
    let actual_type_anchors = declarations
        .type_anchors
        .iter()
        .map(|symbol| symbol_debug_text(*symbol))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_type_anchors, expected_type_anchors);

    let expected_consts = BuiltinConstValue::ALL
        .iter()
        .map(|builtin| builtin.name().to_string())
        .collect::<BTreeSet<_>>();
    let actual_consts = declarations
        .consts
        .iter()
        .map(|symbol| symbol_debug_text(*symbol))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_consts, expected_consts);

    let expected_extends = [
        "i8",
        "i16",
        "i32",
        "i64",
        "i128",
        "isize",
        "slice.Ptr",
        "slice.PtrMut",
        "u8",
        "u16",
        "u32",
        "u64",
        "u128",
        "usize",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let actual_extends = declarations
        .extends
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_extends, expected_extends);
    for primitive in [
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
    ] {
        assert_eq!(
            declarations.extends[primitive],
            vec![known::MIN, known::MAX],
            "primitive builtin extend associated values drift for {primitive}"
        );
    }

    for builtin in BuiltinTrait::ALL {
        let descriptor = builtin.descriptor();
        let source = declarations
            .traits
            .get(descriptor.name)
            .unwrap_or_else(|| panic!("missing source declaration for {}", descriptor.name));
        assert_eq!(
            source.item_name,
            sym(descriptor.name),
            "builtin trait source item name must match `@[builtin]` name"
        );
        assert_eq!(
            source.generic_count, descriptor.generic_count,
            "generic count drift for {}",
            descriptor.name
        );
        assert_eq!(
            source
                .associated_types
                .iter()
                .map(|name| symbol_debug_text(*name))
                .collect::<Vec<_>>(),
            descriptor
                .associated_types
                .iter()
                .map(|associated_type| associated_type.name().to_string())
                .collect::<Vec<_>>(),
            "associated type drift for {}",
            descriptor.name
        );
        assert_eq!(
            source
                .associated_values
                .iter()
                .map(|name| symbol_debug_text(*name))
                .collect::<Vec<_>>(),
            builtin
                .associated_consts()
                .iter()
                .map(|associated_value| associated_value.name().to_string())
                .collect::<Vec<_>>(),
            "associated const drift for {}",
            descriptor.name
        );
        assert_eq!(
            source
                .methods
                .iter()
                .map(|method| symbol_debug_text(method.name))
                .collect::<Vec<_>>(),
            descriptor
                .required_methods
                .iter()
                .map(|method| method.name())
                .collect::<Vec<_>>(),
            "required method drift for {}",
            descriptor.name
        );
        for method in descriptor.required_methods {
            let source_method = source
                .methods
                .iter()
                .find(|candidate| symbol_debug_text(candidate.name) == method.name())
                .unwrap_or_else(|| {
                    panic!(
                        "missing source method {}::{}",
                        descriptor.name,
                        method.name()
                    )
                });
            assert_eq!(
                source_method.param_count,
                method.param_count(),
                "parameter count drift for {}::{}",
                descriptor.name,
                method.name()
            );
            assert_eq!(
                source_method.receiver,
                Some(
                    method
                        .place_receiver_kind()
                        .unwrap_or(method.receiver_kind())
                ),
                "receiver drift for {}::{}",
                descriptor.name,
                method.name()
            );
        }
        assert_eq!(
            source.supertraits,
            descriptor
                .supertraits
                .iter()
                .map(|supertrait| SourceBuiltinSupertrait {
                    name: sym(supertrait.trait_id.name()),
                    preserves_trait_args: supertrait.preserves_trait_args,
                })
                .collect::<Vec<_>>(),
            "supertrait drift for {}",
            descriptor.name
        );
    }
}

#[derive(Debug, Default)]
struct SourceBuiltinDeclarations {
    functions: Vec<SymbolId>,
    types: Vec<SymbolId>,
    type_anchors: Vec<SymbolId>,
    consts: Vec<SymbolId>,
    traits: BTreeMap<String, SourceBuiltinTrait>,
    extends: BTreeMap<String, Vec<SymbolId>>,
}

#[derive(Debug)]
struct SourceBuiltinTrait {
    item_name: SymbolId,
    generic_count: usize,
    associated_types: Vec<SymbolId>,
    associated_values: Vec<SymbolId>,
    methods: Vec<SourceBuiltinMethod>,
    supertraits: Vec<SourceBuiltinSupertrait>,
}

#[derive(Debug)]
struct SourceBuiltinMethod {
    name: SymbolId,
    param_count: usize,
    receiver: Option<ReceiverKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceBuiltinSupertrait {
    name: SymbolId,
    preserves_trait_args: bool,
}

fn std_builtin_source_declarations() -> SourceBuiltinDeclarations {
    let mut out = SourceBuiltinDeclarations::default();
    for path in std_builtin_source_files() {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let (module, errors) = parse_module(&source);
        assert!(
            errors.is_empty(),
            "failed to parse {}: {errors:?}",
            path.display()
        );
        for item in module.items {
            match item.kind {
                nia_ast::ItemKind::Function(function) => {
                    if let Some(name) = builtin_attribute(&item.attributes) {
                        let name_symbol = sym(&name);
                        assert!(
                            BuiltinFunction::from_name(&name).is_some(),
                            "unknown builtin function `{name}` in {}",
                            path.display()
                        );
                        assert_eq!(
                            function.name, name_symbol,
                            "builtin function source item name must match `@[builtin]` name"
                        );
                        assert!(
                            !out.functions.contains(&name_symbol),
                            "duplicate builtin function declaration `{name}` in {}",
                            path.display()
                        );
                        out.functions.push(name_symbol);
                    }
                }
                nia_ast::ItemKind::TypeAlias(alias) => {
                    if let Some(name) = builtin_attribute(&item.attributes) {
                        let name_symbol = sym(&name);
                        let is_opaque = BuiltinType::from_name(&name).is_some();
                        let is_anchor = BuiltinTypeAnchor::from_name(&name).is_some();
                        assert!(
                            is_opaque || is_anchor,
                            "unknown builtin type `{name}` in {}",
                            path.display()
                        );
                        if is_opaque {
                            assert_eq!(
                                alias.name, name_symbol,
                                "builtin type source item name must match `@[builtin]` name"
                            );
                        } else {
                            assert_eq!(
                                alias.name,
                                BuiltinTypeAnchor::from_name(&name).unwrap().symbol_id(),
                                "builtin type anchor source item name must match descriptor item name"
                            );
                        }
                        assert!(
                            alias.ty.is_none(),
                            "builtin type declaration `{name}` in {} must be bodyless",
                            path.display()
                        );
                        let declarations = if is_opaque {
                            &mut out.types
                        } else {
                            &mut out.type_anchors
                        };
                        assert!(
                            !declarations.contains(&name_symbol),
                            "duplicate builtin type declaration `{name}` in {}",
                            path.display()
                        );
                        declarations.push(name_symbol);
                    }
                }
                nia_ast::ItemKind::Trait(item_trait) => {
                    if let Some(name) = builtin_attribute(&item.attributes) {
                        assert!(
                            BuiltinTrait::from_name(&name).is_some(),
                            "unknown builtin trait `{name}` in {}",
                            path.display()
                        );
                        let previous = out
                            .traits
                            .insert(name.clone(), source_builtin_trait(name, item_trait));
                        assert!(
                            previous.is_none(),
                            "duplicate builtin trait declaration in {}",
                            path.display()
                        );
                    }
                }
                nia_ast::ItemKind::Binding(binding) => {
                    if let Some(name) = builtin_attribute(&item.attributes) {
                        let name_symbol = sym(&name);
                        assert!(
                            BuiltinConstValue::from_name(&name).is_some(),
                            "unknown builtin const `{name}` in {}",
                            path.display()
                        );
                        assert_eq!(
                            binding.name,
                            sym(BuiltinConstValue::from_name(&name).unwrap().item_name()),
                            "builtin const source item name must match descriptor item name"
                        );
                        assert!(
                            binding.is_const() && binding.value.is_none() && binding.ty.is_some(),
                            "builtin const declaration `{name}` in {} must be bodyless with an explicit type",
                            path.display()
                        );
                        assert!(
                            !out.consts.contains(&name_symbol),
                            "duplicate builtin const declaration `{name}` in {}",
                            path.display()
                        );
                        out.consts.push(name_symbol);
                    }
                }
                nia_ast::ItemKind::Extend(extend) => {
                    if let Some(name) = builtin_attribute(&item.attributes) {
                        assert!(
                            !out.extends.contains_key(&name),
                            "duplicate builtin extend declaration `{name}` in {}",
                            path.display()
                        );
                        out.extends.insert(
                            name,
                            extend
                                .associated_values
                                .into_iter()
                                .map(|value| value.binding.name)
                                .collect(),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn std_builtin_source_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("lib/std/builtin");
    let mut files = fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", root.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("failed to read builtin entry: {error}"))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "nia"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn source_builtin_trait(
    builtin_name: String,
    item_trait: nia_ast::TraitItem,
) -> SourceBuiltinTrait {
    let generic_names = item_trait
        .generics
        .iter()
        .map(|generic| generic.name)
        .collect::<Vec<_>>();
    let out = SourceBuiltinTrait {
        item_name: item_trait.name,
        generic_count: item_trait.generics.len(),
        associated_types: item_trait
            .associated_types
            .into_iter()
            .map(|associated_type| associated_type.name)
            .collect(),
        associated_values: item_trait
            .associated_values
            .into_iter()
            .map(|associated_value| associated_value.name)
            .collect(),
        methods: item_trait
            .methods
            .into_iter()
            .map(|method| SourceBuiltinMethod {
                name: method.function.name,
                param_count: method.function.params.len(),
                receiver: method
                    .function
                    .params
                    .first()
                    .and_then(|param| param.receiver),
            })
            .collect(),
        supertraits: item_trait
            .supertraits
            .iter()
            .map(|supertrait| source_builtin_supertrait(supertrait, &generic_names))
            .collect(),
    };
    assert_eq!(
        out.item_name,
        sym(&builtin_name),
        "builtin trait source item name must match `@[builtin]` name"
    );
    out
}

fn source_builtin_supertrait(
    ty: &nia_ast::TypeRef,
    generic_names: &[SymbolId],
) -> SourceBuiltinSupertrait {
    let nia_ast::TypeKind::Path { segments } = &ty.kind else {
        panic!(
            "builtin supertrait must be a direct trait path: {}",
            ty.text
        );
    };
    assert_eq!(
        segments.len(),
        1,
        "builtin supertrait must be unqualified: {}",
        ty.text
    );
    let segment = &segments[0];
    let nia_ast::PathSegmentKind::Name(name) = segment.kind else {
        panic!("builtin supertrait must be a named trait path: {}", ty.text);
    };
    SourceBuiltinSupertrait {
        name,
        preserves_trait_args: source_supertrait_preserves_trait_args(segment, generic_names),
    }
}

fn source_supertrait_preserves_trait_args(
    segment: &nia_ast::TypePathSegment,
    generic_names: &[SymbolId],
) -> bool {
    if generic_names.is_empty() {
        return false;
    }
    if segment.args.len() != generic_names.len() {
        return false;
    }
    segment
        .args
        .iter()
        .zip(generic_names)
        .all(|(arg, generic_name)| match arg {
            nia_ast::TypeArg::Type(ty) | nia_ast::TypeArg::TypeOrConst { ty, .. } => {
                let nia_ast::TypeKind::Path { segments } = &ty.kind else {
                    return false;
                };
                matches!(
                    segments.as_slice(),
                    [segment]
                        if matches!(segment.kind, nia_ast::PathSegmentKind::Name(name) if name == *generic_name)
                            && segment.args.is_empty()
                )
            }
            _ => false,
        })
}

fn builtin_attribute(attributes: &[Attribute]) -> Option<String> {
    let mut out = None;
    for attribute in attributes {
        let AttributeKind::Meta(meta) = &attribute.kind else {
            continue;
        };
        if meta.path != [known::BUILTIN] {
            continue;
        }
        let [arg] = meta.args.as_slice() else {
            panic!("`@[builtin]` source declaration must have one argument");
        };
        let nia_ast::ExprKind::String(text) = &arg.kind else {
            panic!("`@[builtin]` source declaration must use a string literal");
        };
        let name = nia_literals::eval_string_literal_parts(text.parts.iter().map(String::as_str))
            .unwrap_or_else(|| panic!("invalid builtin attribute string {:?}", text.parts));
        assert!(
            out.replace(name).is_none(),
            "duplicate `@[builtin]` source declaration"
        );
    }
    out
}
