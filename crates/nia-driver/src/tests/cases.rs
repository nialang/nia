// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use super::common::{checked_program_from_output, codegen_program_from_output};

struct CheckSuite {
    name: &'static str,
    expects_errors: bool,
}

struct IncrementalCheckCase {
    source: PathBuf,
    edited_source: PathBuf,
    initial_expects_errors: bool,
    edited_expects_errors: bool,
}

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

struct CaseManifest {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

#[test]
fn compiler_cases_match_snapshots() {
    let _permit = nia_test_support::compiler_permit();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/check");
    let driver = crate::Driver::new();
    for suite in [
        CheckSuite {
            name: "pass",
            expects_errors: false,
        },
        CheckSuite {
            name: "fail",
            expects_errors: true,
        },
    ] {
        run_check_suite(&driver, &root, suite);
    }
    run_incremental_check_suite(&driver, &root);
    run_codegen_suite(&driver, &root);
    run_llvm_suite(&driver, &root);
}

#[test]
fn build_cases_match_expectations() {
    let _permit = nia_test_support::build_permit();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let driver = crate::Driver::new();
    run_native_object_suite(&driver, &root.join("object"));
    run_executable_suite(&driver, &root.join("executable"));
}

fn run_check_suite(driver: &crate::Driver, root: &Path, suite: CheckSuite) {
    let mut cases = fs::read_dir(root.join(suite.name))
        .unwrap_or_else(|error| panic!("read {} cases: {error}", suite.name))
        .filter_map(|entry| {
            let path = entry.expect("read case entry").path();
            if path.extension().is_some_and(|extension| extension == "nia") {
                Some(path)
            } else if path.is_dir() {
                let entry = path.join("main.nia");
                assert!(
                    entry.is_file(),
                    "{} case must contain main.nia",
                    path.display()
                );
                Some(entry)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "{} suite must contain cases", suite.name);
    for source in cases {
        let snapshot_path = source.with_extension("snap");
        assert_check_case(driver, root, &source, suite.expects_errors, &snapshot_path);
    }
}

fn run_incremental_check_suite(driver: &crate::Driver, root: &Path) {
    let suite_root = root.join("incremental");
    let mut cases = fs::read_dir(&suite_root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", suite_root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(
        !cases.is_empty(),
        "incremental suite must contain directory cases"
    );

    for case_root in cases {
        assert!(
            case_root.is_dir(),
            "incremental case {} must be a directory",
            case_root.display()
        );
        run_incremental_check_case(driver, &case_root);
    }
}

fn run_incremental_check_case(driver: &crate::Driver, case_root: &Path) {
    let case = load_incremental_check_case(case_root);
    let workspace = super::common::temp_dir("check-incremental-case");
    copy_case_tree(case_root, &workspace);

    let source = workspace.join(&case.source);
    let edited_source = workspace.join(&case.edited_source);
    assert!(
        source.is_file(),
        "missing incremental source {}",
        source.display()
    );
    assert!(
        edited_source.is_file(),
        "missing incremental edit {}",
        edited_source.display()
    );
    let initial_snapshot = case_root.join(&case.source).with_extension("snap");
    assert_check_case(
        driver,
        &workspace,
        &source,
        case.initial_expects_errors,
        &initial_snapshot,
    );

    fs::copy(&edited_source, &source).unwrap_or_else(|error| {
        panic!(
            "replace incremental source {} with {}: {error}",
            source.display(),
            edited_source.display()
        )
    });
    let edited_text = fs::read_to_string(&source)
        .unwrap_or_else(|error| panic!("read edited source {}: {error}", source.display()));
    driver.set_source(source.to_string_lossy().into_owned(), edited_text);
    let edited_snapshot = case_root.join(&case.source).with_extension("after.snap");
    assert_check_case(
        driver,
        &workspace,
        &source,
        case.edited_expects_errors,
        &edited_snapshot,
    );
}

fn load_incremental_check_case(case_root: &Path) -> IncrementalCheckCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path.clone();
    manifest.expect("mode", "incremental-check");
    manifest.expect("resource", "compiler");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let edited_source = fixture_relative_path(&manifest_path, manifest.required("edit"));
    let case = IncrementalCheckCase {
        source,
        edited_source,
        initial_expects_errors: case_expects_errors(
            &manifest_path,
            "initial",
            manifest.required("initial"),
        ),
        edited_expects_errors: case_expects_errors(
            &manifest_path,
            "after",
            manifest.required("after"),
        ),
    };
    manifest.finish();
    case
}

fn run_codegen_suite(driver: &crate::Driver, root: &Path) {
    let suite_root = root
        .parent()
        .expect("check suite root must have cases parent")
        .join("codegen");
    let mut cases = fs::read_dir(&suite_root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", suite_root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "codegen suite must contain cases");

    for case_root in cases {
        assert!(
            case_root.is_dir(),
            "codegen case {} must be a directory",
            case_root.display()
        );
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
            diagnostic_snapshot(&program.diagnostics, &suite_root)
        );
        let snapshot_path = source.with_extension("snap");
        let expected = fs::read_to_string(&snapshot_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", snapshot_path.display()));
        assert_eq!(
            codegen_snapshot(&program, &suite_root),
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
    let suite_root = root
        .parent()
        .expect("check suite root must have cases parent")
        .join("llvm");
    let mut cases = fs::read_dir(&suite_root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", suite_root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "LLVM suite must contain cases");

    for case_root in cases {
        assert!(
            case_root.is_dir(),
            "LLVM case {} must be a directory",
            case_root.display()
        );
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
    let mut cases = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "native-object suite must contain cases");

    for case_root in cases {
        assert!(
            case_root.is_dir(),
            "native-object case {} must be a directory",
            case_root.display()
        );
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
    let mut cases = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "executable suite must contain cases");

    for case_root in cases {
        assert!(
            case_root.is_dir(),
            "executable case {} must be a directory",
            case_root.display()
        );
        run_executable_case(driver, &case_root);
    }
}

fn run_executable_case(driver: &crate::Driver, case_root: &Path) {
    let case = load_executable_case(case_root);
    let workspace = super::common::temp_dir("link-execute-case");
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

impl CaseManifest {
    fn load(case_root: &Path) -> Self {
        let path = case_root.join("case.meta");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let mut values = BTreeMap::new();
        for (line_number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').unwrap_or_else(|| {
                panic!(
                    "invalid {}:{}; expected key=value",
                    path.display(),
                    line_number + 1
                )
            });
            let key = key.trim();
            let value = value.trim();
            assert!(
                !key.is_empty() && !value.is_empty(),
                "empty key or value in {}:{}",
                path.display(),
                line_number + 1
            );
            assert!(
                values.insert(key.to_owned(), value.to_owned()).is_none(),
                "duplicate case key {key:?} in {}",
                path.display()
            );
        }
        Self { path, values }
    }

    fn required(&mut self, key: &str) -> String {
        self.values
            .remove(key)
            .unwrap_or_else(|| panic!("{} must declare {key}", self.path.display()))
    }

    fn expect(&mut self, key: &str, expected: &str) {
        let actual = self.required(key);
        assert_eq!(
            actual,
            expected,
            "{} must declare {key}={expected}",
            self.path.display()
        );
    }

    fn required_usize(&mut self, key: &str) -> usize {
        let value = self.required(key);
        value.parse().unwrap_or_else(|error| {
            panic!(
                "{} must declare numeric {key}: {error}",
                self.path.display()
            )
        })
    }

    fn required_i32(&mut self, key: &str) -> i32 {
        let value = self.required(key);
        value.parse().unwrap_or_else(|error| {
            panic!(
                "{} must declare numeric {key}: {error}",
                self.path.display()
            )
        })
    }

    fn required_module_map(&mut self) -> BTreeMap<String, PathBuf> {
        let keys = self
            .values
            .keys()
            .filter(|key| key.starts_with("module."))
            .cloned()
            .collect::<Vec<_>>();
        let mut module_map = BTreeMap::new();
        for key in keys {
            let name = key
                .strip_prefix("module.")
                .expect("module map key prefix")
                .to_owned();
            assert!(
                !name.is_empty(),
                "{} contains an empty module map name",
                self.path.display()
            );
            let value = self.required(&key);
            let path = fixture_relative_path(&self.path, value);
            assert!(
                module_map.insert(name.clone(), path).is_none(),
                "{} contains duplicate module map name {name}",
                self.path.display()
            );
        }
        module_map
    }

    fn finish(&self) {
        assert!(
            self.values.is_empty(),
            "unknown case keys in {}: {:?}",
            self.path.display(),
            self.values.keys().collect::<Vec<_>>()
        );
    }
}

fn fixture_relative_path(manifest_path: &Path, value: String) -> PathBuf {
    let path = PathBuf::from(value);
    assert!(
        !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
        "{} must use a relative fixture path without parent components",
        manifest_path.display()
    );
    path
}

fn case_expects_errors(manifest_path: &Path, key: &str, value: String) -> bool {
    match value.as_str() {
        "pass" => false,
        "fail" => true,
        _ => panic!(
            "{} must declare {key}=pass or {key}=fail",
            manifest_path.display()
        ),
    }
}

fn copy_case_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination)
        .unwrap_or_else(|error| panic!("create {}: {error}", destination.display()));
    for entry in
        fs::read_dir(source).unwrap_or_else(|error| panic!("read {}: {error}", source.display()))
    {
        let entry = entry.expect("read fixture entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_case_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap_or_else(|error| {
                panic!(
                    "copy fixture {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            });
        }
    }
}

fn assert_check_case(
    driver: &crate::Driver,
    root: &Path,
    source: &Path,
    expects_errors: bool,
    snapshot_path: &Path,
) {
    let program = checked_program_from_output(driver.check_entry(crate::CheckRequest::new(
        source.to_string_lossy().into_owned(),
    )));
    assert_eq!(
        nia_compiler_query::has_error_diagnostics(&program.diagnostics),
        expects_errors,
        "{} error classification mismatch\n{}",
        source.display(),
        diagnostic_snapshot(&program.diagnostics, root)
    );
    let expected = fs::read_to_string(snapshot_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", snapshot_path.display()));
    assert_eq!(
        diagnostic_snapshot(&program.diagnostics, root),
        expected,
        "{} diagnostic snapshot changed",
        source.display()
    );
}

fn codegen_snapshot(program: &crate::CodegenProgram, root: &Path) -> String {
    let diagnostics = diagnostic_snapshot(&program.diagnostics, root);
    let mut snapshot = String::new();
    if diagnostics.is_empty() {
        snapshot.push_str("diagnostics: none\n");
    } else {
        snapshot.push_str("diagnostics:\n");
        snapshot.push_str(&diagnostics);
    }
    snapshot.push_str(&crate::optimization_report(program));
    snapshot
}

fn diagnostic_snapshot(diagnostics: &[crate::ProgramDiagnostic], root: &Path) -> String {
    let mut records = Vec::new();
    for diagnostic in diagnostics {
        let mut record = String::new();
        let path = Path::new(diagnostic.path.as_str())
            .strip_prefix(root)
            .unwrap_or_else(|_| Path::new(diagnostic.path.as_str()));
        let _ = writeln!(record, "path: {}", path.display());
        let _ = writeln!(record, "code: {}", diagnostic.diagnostic.code.as_str());
        let _ = writeln!(record, "summary: {}", diagnostic.diagnostic.summary);
        for label in diagnostic.diagnostic.labels.iter() {
            let _ = writeln!(
                record,
                "label: {}..{} {:?} {:?} {}",
                label.span.start,
                label.span.end,
                label.style,
                label.span_source,
                label.message.as_deref().unwrap_or_default()
            );
        }
        for note in diagnostic.diagnostic.notes.iter() {
            let _ = writeln!(record, "note: {note}");
        }
        for help in diagnostic.diagnostic.help.iter() {
            let _ = writeln!(record, "help: {help}");
        }
        for related in diagnostic.diagnostic.related.iter() {
            let _ = writeln!(
                record,
                "related: {}..{} {}",
                related.span.start, related.span.end, related.message
            );
        }
        for field in diagnostic.diagnostic.debug.iter() {
            let _ = writeln!(record, "debug: {}={}", field.key, field.value);
        }
        records.push(record);
    }
    records.join("\n")
}
