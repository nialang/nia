// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn persistent_signature_semantics_reuse_lowering_with_fresh_trait_facts() {
    let root = temp_dir("persistent_signature_semantics_with_fresh_trait_facts");
    let source = r#"
pub struct Box[T] { value: T }
fn unwrap[T](value: Box[T]) T { value.value }
"#;

    let first_sources = SourceDatabase::new();
    first_sources.set_source(SourcePath::new("main.nia"), source);
    let first_loader = LoaderDatabase::new(
        LoadRequest::new("main.nia")
            .with_sources(first_sources)
            .with_frontend_cache_dir(Some(root.clone())),
    );
    let first_compiler = CompilerDatabase::new(
        CompileRequest::new(first_loader).with_frontend_cache_dir(Some(root.clone())),
    );
    let first = first_compiler.analyze_program().expect("cold analysis");
    assert!(!has_error_diagnostics(&first.diagnostics));
    let first_trace = first_compiler.query_trace();
    assert!(first_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "signature_type_resolution" && dependency.to.name == "module_defs"
    }));
    assert!(first_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "signature_type_lowering"
            && dependency.to.name == "signature_type_resolution"
    }));
    assert!(first_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "signature_item_signatures"
            && dependency.to.name == "signature_type_lowering"
    }));
    assert!(first_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_trait_solving_module_facts"
            && dependency.to.name == "extension_signature_module_input"
    }));

    let second_sources = SourceDatabase::new();
    second_sources.set_source(SourcePath::new("main.nia"), source);
    let second_loader = LoaderDatabase::new(
        LoadRequest::new("main.nia")
            .with_sources(second_sources)
            .with_frontend_cache_dir(Some(root.clone())),
    );
    let second_compiler = CompilerDatabase::new(
        CompileRequest::new(second_loader).with_frontend_cache_dir(Some(root.clone())),
    );
    let second = second_compiler.analyze_program().expect("warm analysis");
    assert!(!has_error_diagnostics(&second.diagnostics));
    assert_eq!(second.diagnostics, first.diagnostics);
    let second_trace = second_compiler.query_trace();
    assert_eq!(
        query_executions(&second_trace, "signature_type_resolution"),
        0
    );
    assert_eq!(
        query_executions(&second_trace, "signature_type_lowering"),
        1
    );
    assert!(!second_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "signature_type_lowering"
            && dependency.to.name == "signature_type_resolution"
    }));
    assert!(second_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "signature_item_signatures"
            && dependency.to.name == "frontend_program_sources"
    }));
    assert!(!second_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "signature_item_signatures"
            && dependency.to.name == "signature_type_lowering"
    }));
    assert!(query_executions(&second_trace, "extension_trait_solving_module_facts") > 0);
    assert!(query_executions(&second_trace, "extension_signature_module_input") > 0);
    assert!(second_trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_trait_solving_module_facts"
            && dependency.to.name == "extension_signature_module_input"
    }));

    let verified_sources = SourceDatabase::new();
    verified_sources.set_source(SourcePath::new("main.nia"), source);
    let verified_loader = LoaderDatabase::new(
        LoadRequest::new("main.nia")
            .with_sources(verified_sources)
            .with_frontend_cache_dir(Some(root.clone()))
            .with_frontend_cache_verification(true),
    );
    let verified_compiler = CompilerDatabase::new(
        CompileRequest::new(verified_loader)
            .with_frontend_cache_dir(Some(root))
            .with_frontend_cache_verification(true),
    );
    let verified = verified_compiler
        .analyze_program()
        .expect("verified analysis");
    assert!(!has_error_diagnostics(&verified.diagnostics));
    assert_eq!(verified.diagnostics, first.diagnostics);
    assert!(
        verified_compiler
            .query_trace()
            .dependencies
            .iter()
            .any(|dependency| {
                dependency.from.name == "signature_type_resolution"
                    && dependency.to.name == "module_defs"
            })
    );
    assert!(
        verified_compiler
            .query_trace()
            .dependencies
            .iter()
            .any(|dependency| {
                dependency.from.name == "signature_type_lowering"
                    && dependency.to.name == "signature_type_resolution"
            })
    );
    assert!(
        verified_compiler
            .query_trace()
            .dependencies
            .iter()
            .any(|dependency| {
                dependency.from.name == "signature_item_signatures"
                    && dependency.to.name == "signature_type_lowering"
            })
    );
    assert!(
        verified_compiler
            .query_trace()
            .dependencies
            .iter()
            .any(|dependency| {
                dependency.from.name == "extension_trait_solving_module_facts"
                    && dependency.to.name == "extension_signature_module_input"
            })
    );
}

#[test]
fn persistent_extension_validation_skips_raw_global_dependencies_across_sessions() {
    let root = temp_dir("persistent_extension_validation_dependencies");
    let source = r#"
pub struct Box[T] { value: T }
extend Box[i32] { fn get(self) i32 { self.value } }
fn main() i32 { Box[i32] { value: 1 }.get() }
"#;
    let compile = |verify| {
        let sources = SourceDatabase::new();
        sources.set_source(SourcePath::new("main.nia"), source);
        let loader = LoaderDatabase::new(
            LoadRequest::new("main.nia")
                .with_sources(sources)
                .with_frontend_cache_dir(Some(root.clone()))
                .with_frontend_cache_verification(verify),
        );
        let compiler = CompilerDatabase::new(
            CompileRequest::new(loader)
                .with_frontend_cache_dir(Some(root.clone()))
                .with_frontend_cache_verification(verify),
        );
        let checked = compiler.analyze_program().expect("cached analysis");
        assert!(!has_error_diagnostics(&checked.diagnostics));
        compiler.query_trace()
    };

    let cold = compile(false);
    assert!(cold.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_validation_facts"
            && dependency.to.name == "extension_signature_module_input"
    }));
    assert!(cold.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_validation_facts"
            && dependency.to.name == "extension_trait_signature_index"
    }));

    let warm = compile(false);
    assert!(warm.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_validation_facts"
            && dependency.to.name == "frontend_program_sources"
    }));
    assert!(!warm.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_validation_facts"
            && matches!(
                dependency.to.name,
                "extension_provider_module_eligibility"
                    | "extension_signature_module_input"
                    | "extension_trait_signature_index"
            )
    }));

    let verified = compile(true);
    assert!(verified.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_validation_facts"
            && dependency.to.name == "extension_signature_module_input"
    }));
}
