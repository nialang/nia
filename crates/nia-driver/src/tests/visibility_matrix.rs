// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{ModuleMap, SourcePath};
use std::fs;

#[test]
fn qualified_callable_visibility_matrix() {
    for (visibility_rank, visibility) in ["", "pub(super)", "pub(pkg)", "pub"].iter().enumerate() {
        for (scope, scope_rank, prefix) in [
            ("same", 0, "self"),
            ("parent", 1, "self::model"),
            ("sibling", 1, "super::model"),
            ("package", 2, "self::api::model"),
            ("external", 3, "dep::api::model"),
        ] {
            let root = temp_dir("qualified_callable_visibility_matrix");
            let external = scope == "external";
            let package = if external {
                root.join("dep")
            } else {
                root.clone()
            };
            fs::create_dir_all(package.join("api")).expect("package modules");
            let entry = root.join("main.nia");
            let package_entry = if external {
                root.join("dep.nia")
            } else {
                entry.clone()
            };
            let api = package.join("api.nia");
            let sibling = package.join("api/sibling.nia");
            let model = package.join("api/model.nia");
            write(&package_entry, "pub module api;\n");
            write(&api, "pub module model; pub module sibling;\n");
            write(&sibling, "");
            if external {
                write(&entry, "");
            }
            let declarations = format!(
                r#"
{visibility} struct Hidden {{}}
{visibility} fn plain(value: i32) i32 {{ value }}
{visibility} fn generic[T](value: T) T {{ value }}
pub struct Point {{}}
extend Point {{
    {visibility} fn plain(value: i32) i32 {{ value }}
    {visibility} fn generic[T](value: T) T {{ value }}
}}
"#
            );
            write(&model, &declarations);
            let mut uses = String::new();
            for target in [prefix.to_string(), format!("{prefix}::Point")] {
                uses.push_str(&format!(
                    r#"
    _ = {target}::plain(1);
    _ = {target}::generic[i32](1);
    _ = &{target}::plain;
    _ = &{target}::generic[i32];
"#
                ));
            }
            let exercise = format!(
                "\nfn exercise() {{ {uses}\nlet hidden: {prefix}::Hidden = .{{}}; _ = hidden; }}\n"
            );
            let consumer = match scope {
                "same" => &model,
                "parent" => &api,
                "sibling" => &sibling,
                _ => &entry,
            };
            let mut source = fs::read_to_string(consumer).expect("consumer");
            source.push_str(&exercise);
            write(consumer, &source);
            let mut map = ModuleMap::new();
            if external {
                map.insert("dep", SourcePath::new(package_entry.to_string_lossy()))
                    .expect("dependency");
            }
            let checked = check_program_with_map(entry.to_string_lossy().into_owned(), map);
            if visibility_rank >= scope_rank {
                assert_no_error_diagnostics(&checked.diagnostics);
                continue;
            }
            assert_eq!(
                checked.diagnostics.len(),
                9,
                "{visibility} / {scope}: {:?}",
                checked.diagnostics
            );
            for item in &checked.diagnostics {
                let diagnostic = &item.diagnostic;
                assert_eq!(
                    diagnostic.code.as_str(),
                    "E0201",
                    "{visibility} / {scope}: {diagnostic:?}"
                );
                assert!(!diagnostic.help.is_empty(), "{diagnostic:?}");
                let declaration = diagnostic
                    .related
                    .iter()
                    .find(|related| {
                        related.source_path.as_ref().is_some_and(|path| {
                            SourcePath::new(path).identity()
                                == SourcePath::new(model.to_string_lossy()).identity()
                        })
                    })
                    .unwrap_or_else(|| {
                        panic!("{visibility} / {scope}: declaration evidence: {diagnostic:?}")
                    });
                assert!(
                    declaration.span.end <= declarations.len(),
                    "{declaration:?}"
                );
                let declared = &declarations[declaration.span.start..declaration.span.end];
                assert!(
                    declared.contains("fn plain")
                        || declared.contains("fn generic")
                        || declared.contains("struct Hidden"),
                    "{declaration:?}"
                );
            }
        }
    }
}
