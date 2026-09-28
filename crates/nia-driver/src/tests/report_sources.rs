// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{CheckRequest, Driver, DriverConfig, EmitLlvmRequest, EmitObjectRequest};

#[test]
fn diagnostic_reports_retain_dependency_overlays_across_edits_and_cache() {
    let root = temp_dir("diagnostic_report_sources");
    let entry = root.join("main.nia");
    let api = root.join("api.nia");
    let consumer = root.join("consumer.nia");
    write(
        &entry,
        "module api; module consumer; pub fn main() i32 { consumer::run() }\n",
    );
    write(&api, "fn hidden(value: i32) i32 { value }\n");
    write(&consumer, "pub fn run() i32 { entry::api::hidden(1) }\n");
    let api_overlay = "// declaration overlay\n\nfn hidden(value: i32) i32 { value }\n";
    let consumer_overlay = "// consumer overlay\n\npub fn run() i32 { entry::api::hidden(true) }\n";
    let request = || CheckRequest::new(entry.to_string_lossy().into_owned());
    let make_driver = |verify_frontend_cache| {
        let driver = Driver::with_config(DriverConfig {
            artifact_cache_dir: Some(root.join("cache")),
            verify_frontend_cache,
            ..DriverConfig::new(test_toolchain_layout())
        });
        driver
            .set_source(api.to_string_lossy(), api_overlay)
            .expect("api overlay");
        driver
            .set_source(consumer.to_string_lossy(), consumer_overlay)
            .expect("consumer overlay");
        driver
    };
    let driver = make_driver(false);
    let retained = checked_program_from_output(driver.check_all_modules(request()));
    assert!(driver.compiler_query_executions("checked_program") > 0);
    let text = crate::render_program_diagnostics(&retained, None, None);
    let json = crate::render_program_diagnostics_json(&retained);
    assert!(text.contains("entry::api::hidden(true)"), "{text}");
    assert!(text.contains("api.nia:3:1:"), "{text}");
    assert!(text.contains("consumer.nia:3:"), "{text}");
    for (verify, other_driver) in [(false, make_driver(false)), (true, make_driver(true))] {
        let other = checked_program_from_output(other_driver.check_all_modules(request()));
        assert_eq!(
            other_driver.compiler_query_executions("checked_program") > 0,
            verify
        );
        assert_eq!(retained.diagnostics, other.diagnostics);
        assert_eq!(text, crate::render_program_diagnostics(&other, None, None));
        assert_eq!(json, crate::render_program_diagnostics_json(&other));
    }
    assert_eq!(
        text,
        crate::render_program_diagnostics(
            &retained,
            Some(&consumer.to_string_lossy()),
            Some("unrelated current caller text"),
        )
    );
    let codegen_error = driver.codegen(request()).result.expect_err("private call");
    let llvm_error = driver
        .emit_llvm_ir(EmitLlvmRequest::new(request()))
        .result
        .expect_err("private call before LLVM emission");
    let object_error = driver
        .emit_native_objects(EmitObjectRequest::new(request()))
        .result
        .expect_err("private call before object emission");
    let errors = [codegen_error, llvm_error, object_error];
    let reports = errors
        .iter()
        .map(|error| crate::render_driver_error(error, None, None))
        .collect::<Vec<_>>();
    for report in &reports {
        assert!(report.contains("entry::api::hidden(true)"), "{report}");
        assert!(report.contains("api.nia:3:1:"), "{report}");
    }
    driver
        .set_source(
            api.to_string_lossy(),
            "pub fn hidden(value: i32) i32 { value }\n",
        )
        .expect("repair visibility");
    driver
        .set_source(
            consumer.to_string_lossy(),
            "pub fn run() i32 { entry::api::hidden(1) }\n",
        )
        .expect("repair argument");
    driver
        .check_all_modules(request())
        .result
        .expect("repaired program");
    std::fs::remove_file(&api).expect("remove disk dependency");
    std::fs::remove_file(&consumer).expect("remove disk consumer");
    assert_eq!(
        text,
        crate::render_program_diagnostics(&retained, None, None)
    );
    assert_eq!(json, crate::render_program_diagnostics_json(&retained));
    for (error, report) in errors.iter().zip(reports) {
        assert_eq!(report, crate::render_driver_error(error, None, None));
    }
}

#[test]
fn unavailable_report_sources_do_not_read_later_files() {
    let root = temp_dir("unavailable_report_sources");
    let entry = root.join("main.nia");
    let absent = root.join("absent.nia");
    write(&entry, "module absent; pub fn main() i32 { 0 }\n");
    let program = checked_program_from_output(
        test_driver().check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
    );
    assert!(program.diagnostics.iter().any(|item| item.is_error()));
    let before = crate::render_program_diagnostics(&program, None, None);
    let related = root.join("related.nia");
    let absent_path = crate::SourcePath::new(absent.to_string_lossy());
    let related_path = crate::SourcePath::new(related.to_string_lossy());
    let unavailable = crate::DriverError::CodegenPreparationDiagnostics {
        diagnostic_sources: [
            (absent_path.as_str().to_owned(), None),
            (related_path.as_str().to_owned(), None),
        ]
        .into_iter()
        .collect(),
        diagnostics: vec![crate::ProgramDiagnostic {
            path: absent_path.clone(),
            diagnostic: nia_diagnostic::Diagnostic::user_error(
                nia_diagnostic::codes::NAME_RESOLUTION,
                "unavailable source",
            )
            .primary(nia_span::Span::new(2, 5), "primary location")
            .related_at(
                related_path.as_str(),
                nia_span::Span::new(2, 5),
                "related location",
            )
            .finish(),
        }],
        suppressed_downstream: 0,
    };
    let unavailable_before = crate::render_driver_error(&unavailable, None, None);
    write(&entry, "unrelated replacement for entry\n");
    write(&absent, "unrelated replacement for missing dependency\n");
    write(&related, "unrelated replacement for related dependency\n");
    assert_eq!(
        before,
        crate::render_program_diagnostics(&program, None, None)
    );
    assert_eq!(
        unavailable_before,
        crate::render_driver_error(
            &unavailable,
            Some(absent_path.as_str()),
            Some("unrelated caller text"),
        )
    );
}
