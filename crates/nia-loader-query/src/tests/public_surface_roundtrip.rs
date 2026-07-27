use super::*;
use nia_ast::PathSegmentKind;
use nia_defs::{
    DefId, DefKind, ModuleUsing, PublicSurfaceDefFact, PublicSurfaceEnumScopeFact,
    PublicSurfaceModuleFacts, PublicSurfaceModuleScopeFacts, UsingGroupItem, UsingName,
    UsingPathSegment, UsingSelector,
};
use nia_span::Span;

#[test]
fn public_surface_facts_cache_round_trips_all_stable_fields() {
    let root = temp_dir("public_surface_facts_round_trip_all_stable_fields");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main, " ".repeat(512));
    let identity = public_surface_facts_cache_identity(&file);
    let cache = crate::frontend_cache::PersistentFrontendCache::new(root.join("cache"));
    let names = [
        "module",
        "function",
        "global",
        "const",
        "struct",
        "struct_field",
        "union",
        "union_field",
        "trait",
        "associated_type",
        "trait_method",
        "method",
        "enum",
        "variant",
        "type_alias",
    ];
    let symbols = symbols_for(&[
        "module",
        "function",
        "global",
        "const",
        "struct",
        "struct_field",
        "union",
        "union_field",
        "trait",
        "associated_type",
        "trait_method",
        "method",
        "enum",
        "variant",
        "type_alias",
        "host",
        "selected",
        "renamed",
        "nested",
        "plain",
        "final",
    ]);
    let kinds = [
        DefKind::Module,
        DefKind::Function,
        DefKind::Global,
        DefKind::Const,
        DefKind::Struct,
        DefKind::StructField,
        DefKind::Union,
        DefKind::UnionField,
        DefKind::Trait,
        DefKind::TraitAssociatedType,
        DefKind::TraitMethod,
        DefKind::Method,
        DefKind::Enum,
        DefKind::EnumVariant,
        DefKind::TypeAlias,
    ];
    let parents = [
        None,
        None,
        None,
        None,
        None,
        Some(DefId(5)),
        None,
        Some(DefId(7)),
        None,
        Some(DefId(9)),
        Some(DefId(9)),
        Some(DefId(5)),
        None,
        Some(DefId(13)),
        None,
    ];
    let visibilities = [
        Visibility::Private,
        Visibility::PublicSuper,
        Visibility::PublicPkg,
        Visibility::Public,
    ];
    let defs = names
        .into_iter()
        .zip(kinds)
        .zip(parents)
        .enumerate()
        .map(|(index, ((name, kind), parent))| PublicSurfaceDefFact {
            id: DefId((index + 1) as u64),
            name: sym(name),
            kind,
            parent,
            visibility: visibilities[index % visibilities.len()],
            span: Span::new(index + 1, index + 2),
        })
        .collect::<Vec<_>>();
    let mut modules = vec![(sym("module"), DefId(1))];
    let mut types = vec![
        (sym("struct"), DefId(5)),
        (sym("union"), DefId(7)),
        (sym("trait"), DefId(9)),
        (sym("enum"), DefId(13)),
        (sym("type_alias"), DefId(15)),
    ];
    let mut values = vec![
        (sym("function"), DefId(2)),
        (sym("global"), DefId(3)),
        (sym("const"), DefId(4)),
    ];
    modules.sort_by_key(|entry| entry.0);
    types.sort_by_key(|entry| entry.0);
    values.sort_by_key(|entry| entry.0);
    let facts = PublicSurfaceModuleFacts {
        defs,
        module_scope: PublicSurfaceModuleScopeFacts {
            modules,
            types,
            values,
        },
        enum_scopes: vec![PublicSurfaceEnumScopeFact {
            owner: DefId(13),
            variants: vec![(sym("variant"), DefId(14))],
        }],
        module_usings: vec![ModuleUsing {
            visibility: Visibility::PublicPkg,
            span: Span::new(40, 90),
            host: vec![
                UsingPathSegment {
                    kind: PathSegmentKind::Name(sym("host")),
                    span: Span::new(41, 45),
                },
                UsingPathSegment {
                    kind: PathSegmentKind::Package,
                    span: Span::new(46, 49),
                },
                UsingPathSegment {
                    kind: PathSegmentKind::Super,
                    span: Span::new(50, 53),
                },
                UsingPathSegment {
                    kind: PathSegmentKind::SelfValue,
                    span: Span::new(54, 57),
                },
            ],
            selector: UsingSelector::Group(vec![
                UsingGroupItem::Name(UsingName {
                    name: sym("selected"),
                    name_span: Span::new(58, 62),
                    alias: Some(sym("renamed")),
                    alias_span: Some(Span::new(63, 67)),
                }),
                UsingGroupItem::Nested {
                    host: vec![UsingPathSegment {
                        kind: PathSegmentKind::Name(sym("nested")),
                        span: Span::new(68, 72),
                    }],
                    selector: Box::new(UsingSelector::Group(vec![
                        UsingGroupItem::Name(UsingName {
                            name: sym("plain"),
                            name_span: Span::new(73, 75),
                            alias: None,
                            alias_span: None,
                        }),
                        UsingGroupItem::Nested {
                            host: vec![UsingPathSegment {
                                kind: PathSegmentKind::Super,
                                span: Span::new(76, 77),
                            }],
                            selector: Box::new(UsingSelector::Wildcard {
                                span: Span::new(78, 79),
                            }),
                        },
                        UsingGroupItem::Nested {
                            host: vec![UsingPathSegment {
                                kind: PathSegmentKind::Package,
                                span: Span::new(80, 81),
                            }],
                            selector: Box::new(UsingSelector::SelfName),
                        },
                        UsingGroupItem::Nested {
                            host: vec![UsingPathSegment {
                                kind: PathSegmentKind::SelfValue,
                                span: Span::new(82, 83),
                            }],
                            selector: Box::new(UsingSelector::Single(UsingName {
                                name: sym("final"),
                                name_span: Span::new(84, 85),
                                alias: None,
                                alias_span: None,
                            })),
                        },
                    ])),
                },
            ]),
        }],
    };
    let source =
        crate::frontend_cache::PublicSurfaceFactsSource::new(identity.source, identity.source_len);
    cache
        .publish_public_surface_facts(
            identity.namespace,
            &identity.module,
            source,
            &facts,
            &symbols,
        )
        .expect("publish complete public surface facts");
    let loaded_symbols = SymbolTable::new();

    assert!(matches!(
        cache
            .load_public_surface_facts(
                identity.key,
                identity.namespace,
                &identity.module,
                source,
                &loaded_symbols,
            )
            .expect("load complete public surface facts"),
        crate::frontend_cache::PublicSurfaceFactsCacheLookup::Hit(cached) if cached == facts
    ));
    assert_eq!(
        loaded_symbols.resolve(sym("renamed")).as_deref(),
        Some("renamed")
    );
    assert_eq!(
        loaded_symbols.resolve(sym("final")).as_deref(),
        Some("final")
    );

    let short_sources = SourceDatabase::new();
    let short_file = short_sources.set_source(SourcePath::new("short.nia"), " ".repeat(32));
    let short_identity = public_surface_facts_cache_identity(&short_file);
    assert!(
        cache
            .publish_public_surface_facts(
                short_identity.namespace,
                &short_identity.module,
                crate::frontend_cache::PublicSurfaceFactsSource::new(
                    short_identity.source,
                    short_identity.source_len,
                ),
                &facts,
                &symbols,
            )
            .is_err()
    );
}
