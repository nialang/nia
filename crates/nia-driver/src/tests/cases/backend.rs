// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::tests::common::codegen_program_from_output;

use super::support::{
    CaseManifest, case_expects_errors, codegen_snapshot, copy_case_tree, diagnostic_snapshot,
    fixture_relative_path,
};

struct CodegenCase {
    source: PathBuf,
    expects_errors: bool,
}

struct LlvmCase {
    source: PathBuf,
    ir_contains: String,
    executions: BackendExecutionExpectations,
}

struct NativeObjectCase {
    source: PathBuf,
    minimum_link_inputs: usize,
    executions: BackendExecutionExpectations,
}

struct ExecutableCase {
    source: PathBuf,
    virtual_source: PathBuf,
    module_map: BTreeMap<String, PathBuf>,
    exit_code: i32,
    executions: BackendExecutionExpectations,
}

struct BackendExecutionExpectations {
    codegen_preparation_executions: usize,
    backend_lowering_executions: usize,
    backend_finalization_executions: usize,
}

pub(super) fn run_compiler(driver: &crate::Driver, root: &Path) {
    run_codegen_suite(driver, &root.join("codegen"));
    run_llvm_suite(driver, &root.join("llvm"));
}

pub(super) fn run_build(driver: &crate::Driver, root: &Path) {
    run_native_object_suite(driver, &root.join("object"));
    run_executable_suite(driver, &root.join("executable"));
}

fn run_codegen_suite(driver: &crate::Driver, root: &Path) {
    let mut cases = directory_cases(root, "codegen");
    cases.sort();
    for case_root in cases {
        let case = load_codegen_case(&case_root);
        let source = case_root.join(&case.source);
        assert!(
            source.is_file(),
            "missing codegen source {}",
            source.display()
        );
        let program = codegen_program_from_output(driver.codegen(crate::CheckRequest::new(
            source.to_string_lossy().into_owned(),
        )));
        assert_eq!(
            nia_compiler_query::has_error_diagnostics(&program.diagnostics),
            case.expects_errors,
            "{} error classification mismatch\n{}",
            source.display(),
            diagnostic_snapshot(&program.diagnostics, root)
        );
        let snapshot_path = source.with_extension("snap");
        let expected = fs::read_to_string(&snapshot_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", snapshot_path.display()));
        assert_eq!(
            codegen_snapshot(&program, root),
            expected,
            "{} codegen snapshot changed",
            source.display()
        );
    }
}

fn load_codegen_case(case_root: &Path) -> CodegenCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path.clone();
    manifest.expect("mode", "codegen");
    manifest.expect("resource", "compiler");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let expects_errors = case_expects_errors(&manifest_path, "expect", manifest.required("expect"));
    manifest.finish();
    CodegenCase {
        source,
        expects_errors,
    }
}

fn run_llvm_suite(driver: &crate::Driver, root: &Path) {
    for case_root in directory_cases(root, "LLVM") {
        let case = load_llvm_case(&case_root);
        let source = case_root.join(&case.source);
        assert!(source.is_file(), "missing LLVM source {}", source.display());
        let artifact = driver
            .emit_llvm_ir(crate::EmitLlvmRequest::new(crate::CheckRequest::new(
                source.to_string_lossy().into_owned(),
            )))
            .result
            .unwrap_or_else(|error| panic!("{} LLVM emission failed: {error:?}", source.display()));
        assert!(
            artifact
                .modules
                .iter()
                .any(|module| module.ir.contains(&case.ir_contains)),
            "{} LLVM output does not contain {:?}: {:?}",
            source.display(),
            case.ir_contains,
            artifact.modules
        );
        case.executions.assert(driver, &source);
    }
}

fn load_llvm_case(case_root: &Path) -> LlvmCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path.clone();
    manifest.expect("mode", "emit-llvm");
    manifest.expect("resource", "compiler");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let ir_contains = manifest.required("ir-contains");
    let executions = BackendExecutionExpectations::load(&mut manifest);
    manifest.finish();
    LlvmCase {
        source,
        ir_contains,
        executions,
    }
}

fn run_native_object_suite(driver: &crate::Driver, root: &Path) {
    for case_root in directory_cases(root, "native-object") {
        let case = load_native_object_case(&case_root);
        let source = case_root.join(&case.source);
        assert!(
            source.is_file(),
            "missing native-object source {}",
            source.display()
        );
        let artifact = driver
            .emit_native_objects(crate::EmitObjectRequest::new(crate::CheckRequest::new(
                source.to_string_lossy().into_owned(),
            )))
            .result
            .unwrap_or_else(|error| {
                panic!(
                    "{} native-object emission failed: {error:?}",
                    source.display()
                )
            });
        assert!(
            artifact.link_inputs.as_slice().len() >= case.minimum_link_inputs,
            "{} emitted {} link inputs, expected at least {}",
            source.display(),
            artifact.link_inputs.as_slice().len(),
            case.minimum_link_inputs
        );
        assert!(
            artifact
                .link_inputs
                .as_slice()
                .iter()
                .all(|input| !input.object.bytes.is_empty()),
            "{} emitted an empty native object",
            source.display()
        );
        case.executions.assert(driver, &source);
    }
}

fn load_native_object_case(case_root: &Path) -> NativeObjectCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path.clone();
    manifest.expect("mode", "emit-object");
    manifest.expect("resource", "build");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let minimum_link_inputs = manifest.required_usize("minimum-link-inputs");
    let executions = BackendExecutionExpectations::load(&mut manifest);
    manifest.finish();
    NativeObjectCase {
        source,
        minimum_link_inputs,
        executions,
    }
}

fn run_executable_suite(driver: &crate::Driver, root: &Path) {
    for case_root in directory_cases(root, "executable") {
        run_executable_case(driver, &case_root);
    }
}

fn run_executable_case(driver: &crate::Driver, case_root: &Path) {
    let case = load_executable_case(case_root);
    let workspace = crate::tests::common::temp_dir("link-execute-case");
    copy_case_tree(case_root, &workspace);

    let source = workspace.join(&case.source);
    let virtual_source = workspace.join(&case.virtual_source);
    let source_text = fs::read_to_string(&source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    driver.set_source(
        virtual_source.to_string_lossy().into_owned(),
        source_text.clone(),
    );

    let mut module_map = crate::ModuleMap::new();
    for (name, relative_path) in case.module_map {
        let path = workspace.join(relative_path);
        assert!(
            path.is_file(),
            "missing mapped module {} for {name}",
            path.display()
        );
        module_map.insert(
            name,
            crate::SourcePath::new(path.to_string_lossy().into_owned()),
        );
    }

    let output = workspace.join("out/runner");
    let artifact = driver
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(virtual_source.to_string_lossy().into_owned())
                .with_module_map(module_map),
            &output,
        ))
        .result
        .unwrap_or_else(|error| {
            panic!(
                "{} executable link failed: {}",
                virtual_source.display(),
                crate::render_driver_error(
                    &error,
                    Some(&virtual_source.to_string_lossy()),
                    Some(&source_text)
                )
            )
        });
    assert_eq!(artifact.path, output, "linked executable output path");
    assert!(
        artifact.path.is_file(),
        "missing linked executable {}",
        artifact.path.display()
    );
    case.executions.assert(driver, &virtual_source);

    let status = Command::new(&artifact.path)
        .status()
        .unwrap_or_else(|error| panic!("run {}: {error}", artifact.path.display()));
    assert_eq!(
        status.code(),
        Some(case.exit_code),
        "{} exit status",
        artifact.path.display()
    );
}

fn load_executable_case(case_root: &Path) -> ExecutableCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path.clone();
    manifest.expect("mode", "link-execute");
    manifest.expect("resource", "build");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let virtual_source = fixture_relative_path(&manifest_path, manifest.required("virtual-source"));
    let module_map = manifest.required_module_map();
    assert!(
        !module_map.is_empty(),
        "{} must declare at least one module.<name> mapping",
        manifest_path.display()
    );
    let exit_code = manifest.required_i32("exit-code");
    let executions = BackendExecutionExpectations::load(&mut manifest);
    manifest.finish();
    ExecutableCase {
        source,
        virtual_source,
        module_map,
        exit_code,
        executions,
    }
}

fn directory_cases(root: &Path, suite: &str) -> Vec<PathBuf> {
    let mut cases = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "{suite} suite must contain cases");
    for case_root in &cases {
        assert!(
            case_root.is_dir(),
            "{suite} case {} must be a directory",
            case_root.display()
        );
    }
    cases
}

impl BackendExecutionExpectations {
    fn load(manifest: &mut CaseManifest) -> Self {
        Self {
            codegen_preparation_executions: manifest
                .required_usize("codegen-preparation-executions"),
            backend_lowering_executions: manifest.required_usize("backend-lowering-executions"),
            backend_finalization_executions: manifest
                .required_usize("backend-finalization-executions"),
        }
    }

    fn assert(&self, driver: &crate::Driver, source: &Path) {
        assert_eq!(
            driver.compiler_query_executions("codegen_preparation"),
            self.codegen_preparation_executions,
            "{} codegen preparation executions",
            source.display()
        );
        assert_eq!(
            driver.compiler_query_executions("backend_lowering"),
            self.backend_lowering_executions,
            "{} backend lowering executions",
            source.display()
        );
        assert_eq!(
            driver.compiler_query_executions("backend_module_finalization"),
            self.backend_finalization_executions,
            "{} backend finalization executions",
            source.display()
        );
    }
}
