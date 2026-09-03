// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::{BuildPlanDraft, ModuleImport, PlanAction, PlanPackage, PlanStep};
use std::sync::{Arc, Barrier, Mutex, OnceLock, atomic::AtomicBool};

fn action(name: &str) -> ActionKey {
    ActionKey::new(PackageKey::root(), name).unwrap()
}

fn step(name: &str) -> StepKey {
    StepKey::new(PackageKey::root(), name).unwrap()
}

fn target() -> TargetSpec {
    TargetSpec {
        arch: "x86_64".to_string(),
        vendor: "unknown".to_string(),
        os: "linux".to_string(),
        env: String::new(),
        abi: String::new(),
        endian: "little".to_string(),
        pointer_width: 64,
    }
}

fn test_invocation() -> BuildInvocation {
    static TOOLCHAIN: OnceLock<Arc<nia_toolchain::ToolchainLayout>> = OnceLock::new();
    static TEST_RUN_ID: OnceLock<u128> = OnceLock::new();
    let toolchain = Arc::clone(TOOLCHAIN.get_or_init(|| {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("nia-build lives under crates");
        Arc::new(
            nia_toolchain::ToolchainLayout::resolve(
                nia_toolchain::ToolchainLayoutRequest::explicit(
                    std::env::current_exe().expect("test executable"),
                    workspace.join("lib"),
                ),
            )
            .expect("test toolchain"),
        )
    }));
    let test_run_id = TEST_RUN_ID.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock must follow the Unix epoch")
            .as_nanos()
    });
    let sequence = GENERATED_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let package_root = std::env::temp_dir().join(format!(
        "nia-build-coordinator-test-{}-{test_run_id}-{sequence}",
        std::process::id(),
    ));
    let build_dir = package_root.join(".nia-build");
    BuildInvocation {
        toolchain,
        build_script: package_root.join("build.nia"),
        cache_dir: package_root.join(".nia-cache"),
        runner_dir: build_dir.join("runner"),
        runner_executable: build_dir.join("runner/nia-build-runner"),
        runner_config: build_dir.join("build-runner.config"),
        plan_draft: build_dir.join("build-plan.draft"),
        plan_path: build_dir.join("build-plan.bin"),
        build_dir,
        package_root,
        step: crate::BuildStepSelection::Default,
        test_filter: None,
        test_list: false,
        timings: nia_driver::TimingMode::Off,
        timing_format: nia_timing::TimingFormat::Text,
        max_parallel_actions: None,
        optimization: OptimizationMode::O0,
    }
}

fn aggregate_plan(
    actions: &[&str],
    steps: Vec<(&str, &str, Vec<&str>)>,
    selected: &str,
) -> BuildPlan {
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target(),
        artifact_target: target(),
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions: actions
            .iter()
            .map(|name| PlanAction {
                key: action(name),
                kind: ActionKind::Aggregate,
            })
            .collect(),
        steps: steps
            .into_iter()
            .map(|(name, action_name, dependencies)| PlanStep {
                key: step(name),
                action: action(action_name),
                dependencies: dependencies.into_iter().map(step).collect(),
            })
            .collect(),
        default_step: None,
        selected_step: Some(step(selected)),
    })
    .unwrap()
}

#[test]
fn compiler_requests_preserve_physical_paths_and_stable_build_identities() {
    let module = ModuleKey::new(PackageKey::root(), "app").unwrap();
    let check = action("check");
    let generate = action("generate");
    let check_step = step("check");
    let generate_step = step("generate");
    let plan = BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target(),
        artifact_target: target(),
        modules: vec![PlanModule {
            key: module.clone(),
            root_source: LogicalPath::new(
                LogicalPathRoot::Package(PackageKey::root()),
                "src/main.nia",
            )
            .unwrap(),
            optimization: OptimizationMode::O2,
            imports: vec![ModuleImport {
                name: "generated".to_string(),
                path: LogicalPath::new(LogicalPathRoot::Build, "generated/root.nia").unwrap(),
            }],
        }],
        artifacts: Vec::new(),
        actions: vec![
            PlanAction {
                key: check.clone(),
                kind: ActionKind::CompilerCheck {
                    module: module.clone(),
                    target: target(),
                    runtime: Runtime::Freestanding,
                },
            },
            PlanAction {
                key: generate.clone(),
                kind: ActionKind::GeneratedFile {
                    output: LogicalPath::new(LogicalPathRoot::Build, "generated/root.nia").unwrap(),
                    contents: b"pub fn generated() () {}\n".to_vec(),
                },
            },
        ],
        steps: vec![
            PlanStep {
                key: check_step.clone(),
                action: check,
                dependencies: vec![generate_step.clone()],
            },
            PlanStep {
                key: generate_step,
                action: generate,
                dependencies: Vec::new(),
            },
        ],
        default_step: Some(check_step.clone()),
        selected_step: Some(check_step),
    })
    .unwrap();
    let invocation = test_invocation();
    let executor = DriverActionExecutor::new(plan.clone(), invocation.clone());
    let request = executor
        .check_request(&plan.actions()[0], &module, Runtime::Freestanding)
        .expect("construct compiler check request");
    let generated = request
        .module_map
        .get("generated")
        .expect("generated module mapping");

    assert_eq!(
        request.entry_path.as_str(),
        invocation
            .package_root
            .join("src/main.nia")
            .to_str()
            .unwrap()
    );
    assert_eq!(
        request.entry_path.identity().normalized_path(),
        "build-package:root:/src/main.nia"
    );
    assert_eq!(
        generated.as_str(),
        invocation
            .build_dir
            .join("generated/root.nia")
            .to_str()
            .unwrap()
    );
    assert_eq!(
        generated.identity().normalized_path(),
        "build-output:root:/generated/root.nia"
    );
}

#[test]
fn selected_closure_is_iterative_deterministic_and_excludes_unselected_steps() {
    let plan = aggregate_plan(
        &["shared", "left", "right", "final", "unused"],
        vec![
            ("unused", "unused", vec![]),
            ("final", "final", vec!["right", "left"]),
            ("right", "right", vec!["shared"]),
            ("left", "left", vec!["shared"]),
            ("shared", "shared", vec![]),
        ],
        "final",
    );
    let mut observed = Vec::new();
    let mut waves = Vec::new();

    let report = execute_selected_closure(&plan, |items| {
        observed.extend(items.iter().map(|item| item.key.name().to_string()));
        waves.push(
            items
                .iter()
                .map(|item| item.key.name().to_string())
                .collect::<Vec<_>>(),
        );
        items
            .iter()
            .map(|_| ActionOutcome::Succeeded(None))
            .collect()
    })
    .unwrap();

    assert_eq!(observed, ["shared", "left", "right", "final"]);
    assert_eq!(
        waves,
        [
            vec!["shared".to_string()],
            vec!["left".to_string(), "right".to_string()],
            vec!["final".to_string()],
        ]
    );
    assert_eq!(
        report.steps.iter().map(StepKey::name).collect::<Vec<_>>(),
        ["shared", "left", "right", "final"]
    );
}

#[test]
fn shared_action_executes_once_across_multiple_steps() {
    let plan = aggregate_plan(
        &["shared", "final"],
        vec![
            ("a", "shared", vec![]),
            ("b", "shared", vec![]),
            ("final", "final", vec!["a", "b"]),
        ],
        "final",
    );
    let mut observed = Vec::new();

    let report = execute_selected_closure(&plan, |items| {
        observed.extend(items.iter().map(|item| item.key.name().to_string()));
        items
            .iter()
            .map(|_| ActionOutcome::Succeeded(None))
            .collect()
    })
    .unwrap();

    assert_eq!(observed, ["shared", "final"]);
    assert_eq!(report.steps.len(), 3);
    assert_eq!(report.actions.len(), 2);
}

#[test]
fn action_failure_stops_dependents_and_retains_action_context() {
    let plan = aggregate_plan(
        &["first", "second"],
        vec![
            ("first", "first", vec![]),
            ("second", "second", vec!["first"]),
        ],
        "second",
    );

    let error = execute_selected_closure(&plan, |items| {
        items
            .iter()
            .map(|item| ActionOutcome::Failed(unsupported(item, "test-failure")))
            .collect()
    })
    .unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::UnsupportedAction { action, kind: "test-failure" }
            if action.name() == "first"
    ));
}

#[test]
fn cancellation_preserves_earlier_canonical_failure_candidates() {
    let earliest_failure = Arc::new(AtomicUsize::new(usize::MAX));
    let earlier = ActionCancellation {
        earliest_failure: Arc::clone(&earliest_failure),
        position: 0,
    };
    let first_failure = ActionCancellation {
        earliest_failure: Arc::clone(&earliest_failure),
        position: 1,
    };
    let later = ActionCancellation {
        earliest_failure,
        position: 2,
    };

    first_failure.cancel_later_actions();

    assert!(!earlier.is_cancelled());
    assert!(!first_failure.is_cancelled());
    assert!(later.is_cancelled());
    earlier.cancel_later_actions();
    assert!(first_failure.is_cancelled());
}

#[test]
fn concurrent_completion_order_does_not_change_visible_order() {
    let plan = aggregate_plan(
        &["shared", "left", "right", "final"],
        vec![
            ("final", "final", vec!["left", "right"]),
            ("right", "right", vec!["shared"]),
            ("left", "left", vec!["shared"]),
            ("shared", "shared", vec![]),
        ],
        "final",
    );
    let sequential = execute_selected_closure(&plan, |items| {
        items
            .iter()
            .map(|_| ActionOutcome::Succeeded(None))
            .collect()
    })
    .unwrap();
    let completion_order = Arc::new(Mutex::new(Vec::new()));

    let concurrent = execute_selected_closure(&plan, |items| {
        let barrier = Arc::new(Barrier::new(items.len()));
        std::thread::scope(|scope| {
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let barrier = Arc::clone(&barrier);
                    let completion_order = Arc::clone(&completion_order);
                    scope.spawn(move || {
                        barrier.wait();
                        if items.len() > 1 && index == 0 {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        completion_order
                            .lock()
                            .expect("completion order lock poisoned")
                            .push(item.key.name().to_string());
                        ActionOutcome::Succeeded(None)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().expect("synthetic action worker"))
                .collect()
        })
    })
    .unwrap();

    let completion_order = completion_order
        .lock()
        .expect("completion order lock poisoned");
    let left = completion_order
        .iter()
        .position(|name| name == "left")
        .unwrap();
    let right = completion_order
        .iter()
        .position(|name| name == "right")
        .unwrap();
    assert!(right < left);
    assert_eq!(concurrent, sequential);
    assert_eq!(
        concurrent
            .actions
            .iter()
            .map(ActionKey::name)
            .collect::<Vec<_>>(),
        ["shared", "left", "right", "final"]
    );
}

#[test]
fn single_worker_action_limit_serializes_ready_tasks() {
    let session = QuerySession::new();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tasks = (0..64).map(|value| {
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        (ActionResourceClass::Cpu, move || {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            std::thread::yield_now();
            active.fetch_sub(1, Ordering::SeqCst);
            value
        })
    });

    let values = run_action_tasks(
        &session,
        Some(std::num::NonZeroUsize::new(1).unwrap()),
        tasks,
    );

    assert_eq!(values, (0..64).collect::<Vec<_>>());
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(peak.load(Ordering::SeqCst), 1);
}

#[test]
fn action_resource_capacity_only_reduces_inherited_capacity() {
    let session = QuerySession::new();
    let inherited = session.executor_parallelism();
    assert_eq!(action_resource_capacity(&session, None), inherited);
    assert_eq!(
        action_resource_capacity(
            &session,
            std::num::NonZeroUsize::new(inherited.saturating_add(8))
        ),
        inherited
    );
    assert_eq!(
        action_resource_capacity(&session, std::num::NonZeroUsize::new(1)),
        1
    );
}

#[test]
fn bounded_wide_graph_stress_preserves_deterministic_report() {
    let width = 48usize;
    let leaves = (0..width)
        .map(|index| format!("leaf-{index:02}"))
        .collect::<Vec<_>>();
    let mut actions = leaves
        .iter()
        .map(|name| PlanAction {
            key: action(name),
            kind: ActionKind::Aggregate,
        })
        .collect::<Vec<_>>();
    actions.push(PlanAction {
        key: action("final"),
        kind: ActionKind::Aggregate,
    });
    let mut steps = leaves
        .iter()
        .map(|name| PlanStep {
            key: step(name),
            action: action(name),
            dependencies: Vec::new(),
        })
        .collect::<Vec<_>>();
    steps.push(PlanStep {
        key: step("final"),
        action: action("final"),
        dependencies: leaves.iter().map(|name| step(name)).collect(),
    });
    let plan = BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target(),
        artifact_target: target(),
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions,
        steps,
        default_step: None,
        selected_step: Some(step("final")),
    })
    .unwrap();
    let expected = execute_selected_closure(&plan, |items| {
        items
            .iter()
            .map(|_| ActionOutcome::Succeeded(None))
            .collect()
    })
    .unwrap();

    for iteration in 0..32usize {
        let session = QuerySession::new();
        let limit = std::num::NonZeroUsize::new(if iteration % 4 == 0 { 1 } else { 4 });
        let report = execute_selected_closure(&plan, |items| {
            let tasks = items
                .iter()
                .enumerate()
                .map(|(position, _)| {
                    (
                        if position % 2 == 0 {
                            ActionResourceClass::Cpu
                        } else {
                            ActionResourceClass::Io
                        },
                        move || {
                            for _ in 0..(position + iteration) % 7 {
                                std::thread::yield_now();
                            }
                            ActionOutcome::Succeeded(None)
                        },
                    )
                })
                .collect::<Vec<_>>();
            run_action_tasks(&session, limit, tasks)
        })
        .unwrap();

        assert_eq!(report, expected, "iteration {iteration}");
    }
}

#[test]
fn failure_waits_for_active_wave_and_never_dispatches_dependents() {
    let plan = aggregate_plan(
        &["active", "active-next", "fail", "blocked", "final"],
        vec![
            ("active", "active", vec![]),
            ("fail", "fail", vec![]),
            ("active-next", "active-next", vec!["active"]),
            ("blocked", "blocked", vec!["fail"]),
            ("final", "final", vec!["active-next", "blocked"]),
        ],
        "final",
    );
    let active_settled = Arc::new(AtomicBool::new(false));
    let mut dispatched = Vec::new();

    let error = execute_selected_closure(&plan, |items| {
        dispatched.push(
            items
                .iter()
                .map(|item| item.key.name().to_string())
                .collect::<Vec<_>>(),
        );
        let barrier = Arc::new(Barrier::new(items.len()));
        std::thread::scope(|scope| {
            items
                .iter()
                .map(|item| {
                    let barrier = Arc::clone(&barrier);
                    let active_settled = Arc::clone(&active_settled);
                    scope.spawn(move || {
                        barrier.wait();
                        if item.key.name() == "fail" {
                            ActionOutcome::Failed(unsupported(item, "wave-failure"))
                        } else {
                            std::thread::sleep(Duration::from_millis(25));
                            active_settled.store(true, Ordering::Release);
                            ActionOutcome::Succeeded(None)
                        }
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().expect("synthetic action worker"))
                .collect()
        })
    })
    .unwrap_err();

    assert!(active_settled.load(Ordering::Acquire));
    assert_eq!(dispatched, [vec!["active".to_string(), "fail".to_string()]]);
    assert!(matches!(
        error,
        CoordinatorError::UnsupportedAction { action, kind: "wave-failure" }
            if action.name() == "fail"
    ));
}

#[test]
fn empty_plan_executes_nothing() {
    let plan = BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target(),
        artifact_target: target(),
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions: Vec::new(),
        steps: Vec::new(),
        default_step: None,
        selected_step: None,
    })
    .unwrap();

    let report = execute_selected_closure(&plan, |_| unreachable!()).unwrap();
    assert!(report.steps.is_empty());
    assert!(report.actions.is_empty());
}

#[test]
fn all_optimization_and_runtime_modes_map_exactly() {
    assert_eq!(optimization(OptimizationMode::O0), NiaOptimizationLevel::O0);
    assert_eq!(optimization(OptimizationMode::O1), NiaOptimizationLevel::O1);
    assert_eq!(optimization(OptimizationMode::O2), NiaOptimizationLevel::O2);
    assert_eq!(optimization(OptimizationMode::O3), NiaOptimizationLevel::O3);
    assert_eq!(optimization(OptimizationMode::Os), NiaOptimizationLevel::Os);
    assert_eq!(optimization(OptimizationMode::Oz), NiaOptimizationLevel::Oz);
    assert_eq!(runtime_mode(Runtime::Bare), DriverRuntime::Bare);
    assert_eq!(
        runtime_mode(Runtime::Freestanding),
        DriverRuntime::Freestanding
    );
}

#[test]
fn explicit_uncacheable_action_has_no_legacy_execution_fallback() {
    let invocation = test_invocation();
    let target = target_spec(invocation.toolchain.host_target());
    let plan = BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target.clone(),
        artifact_target: target,
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions: vec![PlanAction {
            key: action("opaque"),
            kind: ActionKind::Uncacheable {
                description: "legacy callback".to_string(),
            },
        }],
        steps: vec![PlanStep {
            key: step("opaque"),
            action: action("opaque"),
            dependencies: Vec::new(),
        }],
        default_step: Some(step("opaque")),
        selected_step: None,
    })
    .unwrap();

    let error = execute_build_plan(&plan, &invocation).unwrap_err();
    assert!(matches!(
        error,
        CoordinatorError::UnsupportedAction { action, kind: "uncacheable" }
            if action.name() == "opaque" && action.package().as_str() == "root"
    ));
}

#[test]
fn invocation_target_mismatch_is_rejected_before_actions() {
    let invocation = test_invocation();
    let mut mismatched = target_spec(invocation.toolchain.host_target());
    mismatched.arch = "mismatched".to_string();
    let plan = BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: mismatched,
        artifact_target: target_spec(invocation.toolchain.artifact_target()),
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions: Vec::new(),
        steps: Vec::new(),
        default_step: None,
        selected_step: None,
    })
    .unwrap();

    let error = execute_build_plan(&plan, &invocation).unwrap_err();
    assert!(matches!(
        error,
        CoordinatorError::TargetMismatch(details)
            if details.role == "host" && details.found.arch == "mismatched"
    ));
}

fn generated_plan(invocation: &BuildInvocation, output: &str, contents: &[u8]) -> BuildPlan {
    let host = target_spec(invocation.toolchain.host_target());
    let artifact = target_spec(invocation.toolchain.artifact_target());
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: host,
        artifact_target: artifact,
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions: vec![PlanAction {
            key: action("generate"),
            kind: ActionKind::GeneratedFile {
                output: LogicalPath::new(LogicalPathRoot::Build, output).unwrap(),
                contents: contents.to_vec(),
            },
        }],
        steps: vec![PlanStep {
            key: step("generate"),
            action: action("generate"),
            dependencies: Vec::new(),
        }],
        default_step: Some(step("generate")),
        selected_step: None,
    })
    .unwrap()
}

fn compiler_check_plan(invocation: &BuildInvocation, optimization: OptimizationMode) -> BuildPlan {
    compiler_check_plan_with_runtime(invocation, optimization, Runtime::Bare)
}

fn compiler_check_plan_with_runtime(
    invocation: &BuildInvocation,
    optimization: OptimizationMode,
    runtime: Runtime,
) -> BuildPlan {
    let module = ModuleKey::new(PackageKey::root(), "app").unwrap();
    let host = target_spec(invocation.toolchain.host_target());
    let artifact = target_spec(invocation.toolchain.artifact_target());
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: host,
        artifact_target: artifact.clone(),
        modules: vec![PlanModule {
            key: module.clone(),
            root_source: LogicalPath::new(
                LogicalPathRoot::Package(PackageKey::root()),
                "src/main.nia",
            )
            .unwrap(),
            optimization,
            imports: Vec::new(),
        }],
        artifacts: Vec::new(),
        actions: vec![PlanAction {
            key: action("check"),
            kind: ActionKind::CompilerCheck {
                module,
                target: artifact,
                runtime,
            },
        }],
        steps: vec![PlanStep {
            key: step("check"),
            action: action("check"),
            dependencies: Vec::new(),
        }],
        default_step: Some(step("check")),
        selected_step: None,
    })
    .unwrap()
}

fn compiler_emit_plan(
    invocation: &BuildInvocation,
    optimization: OptimizationMode,
    runtime: Runtime,
    output: &str,
) -> BuildPlan {
    compiler_emit_plan_kind(
        invocation,
        optimization,
        runtime,
        output,
        PlanArtifactKind::Executable,
    )
}

fn object_set_plan(invocation: &BuildInvocation, output: &str) -> BuildPlan {
    compiler_emit_plan_kind(
        invocation,
        OptimizationMode::O0,
        Runtime::Freestanding,
        output,
        PlanArtifactKind::ObjectSet,
    )
}

fn static_archive_plan(invocation: &BuildInvocation, output: &str) -> BuildPlan {
    compiler_emit_plan_kind(
        invocation,
        OptimizationMode::O0,
        Runtime::Freestanding,
        output,
        PlanArtifactKind::StaticArchive,
    )
}

fn compiler_emit_plan_kind(
    invocation: &BuildInvocation,
    optimization: OptimizationMode,
    runtime: Runtime,
    output: &str,
    kind: PlanArtifactKind,
) -> BuildPlan {
    let module = ModuleKey::new(PackageKey::root(), "app").unwrap();
    let artifact_key = ArtifactKey::new(PackageKey::root(), "app").unwrap();
    let host = target_spec(invocation.toolchain.host_target());
    let artifact_target = target_spec(invocation.toolchain.artifact_target());
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: host,
        artifact_target: artifact_target.clone(),
        modules: vec![PlanModule {
            key: module.clone(),
            root_source: LogicalPath::new(
                LogicalPathRoot::Package(PackageKey::root()),
                "src/main.nia",
            )
            .unwrap(),
            optimization,
            imports: Vec::new(),
        }],
        artifacts: vec![PlanArtifact {
            key: artifact_key.clone(),
            root_module: module,
            kind,
            output: LogicalPath::new(LogicalPathRoot::Build, output).unwrap(),
            runtime,
        }],
        actions: vec![PlanAction {
            key: action("emit"),
            kind: ActionKind::CompilerEmit {
                artifact: artifact_key,
                target: artifact_target,
                static_archives: Vec::new(),
            },
        }],
        steps: vec![PlanStep {
            key: step("emit"),
            action: action("emit"),
            dependencies: Vec::new(),
        }],
        default_step: Some(step("emit")),
        selected_step: None,
    })
    .unwrap()
}

fn install_executable_plan(invocation: &BuildInvocation) -> BuildPlan {
    let module = ModuleKey::new(PackageKey::root(), "app").unwrap();
    let artifact_key = ArtifactKey::new(PackageKey::root(), "app").unwrap();
    let artifact_target = target_spec(invocation.toolchain.artifact_target());
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target_spec(invocation.toolchain.host_target()),
        artifact_target: artifact_target.clone(),
        modules: vec![PlanModule {
            key: module.clone(),
            root_source: LogicalPath::new(
                LogicalPathRoot::Package(PackageKey::root()),
                "src/main.nia",
            )
            .unwrap(),
            optimization: OptimizationMode::O2,
            imports: Vec::new(),
        }],
        artifacts: vec![PlanArtifact {
            key: artifact_key.clone(),
            root_module: module,
            kind: PlanArtifactKind::Executable,
            output: LogicalPath::new(LogicalPathRoot::Build, "bin/app").unwrap(),
            runtime: Runtime::Freestanding,
        }],
        actions: vec![
            PlanAction {
                key: action("emit"),
                kind: ActionKind::CompilerEmit {
                    artifact: artifact_key.clone(),
                    target: artifact_target,
                    static_archives: Vec::new(),
                },
            },
            PlanAction {
                key: action("install"),
                kind: ActionKind::InstallArtifact {
                    artifact: artifact_key,
                    destination: LogicalPath::new(LogicalPathRoot::Build, "install/custom-app")
                        .unwrap(),
                },
            },
        ],
        steps: vec![
            PlanStep {
                key: step("emit"),
                action: action("emit"),
                dependencies: Vec::new(),
            },
            PlanStep {
                key: step("install"),
                action: action("install"),
                dependencies: vec![step("emit")],
            },
        ],
        default_step: Some(step("install")),
        selected_step: None,
    })
    .unwrap()
}

fn mixed_generated_source_emit_plan(
    invocation: &BuildInvocation,
    generated_body: &str,
    generated_helper_value: i32,
) -> BuildPlan {
    let generated_module = ModuleKey::new(PackageKey::root(), "generated").unwrap();
    let stable_module = ModuleKey::new(PackageKey::root(), "stable").unwrap();
    let generated_artifact = ArtifactKey::new(PackageKey::root(), "generated").unwrap();
    let stable_artifact = ArtifactKey::new(PackageKey::root(), "stable").unwrap();
    let host = target_spec(invocation.toolchain.host_target());
    let artifact_target = target_spec(invocation.toolchain.artifact_target());
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: host,
        artifact_target: artifact_target.clone(),
        modules: vec![
            PlanModule {
                key: generated_module.clone(),
                root_source: LogicalPath::new(LogicalPathRoot::Build, "generated/main.nia")
                    .unwrap(),
                optimization: OptimizationMode::O2,
                imports: vec![ModuleImport {
                    name: "helper".to_string(),
                    path: LogicalPath::new(LogicalPathRoot::Build, "generated/helper.nia").unwrap(),
                }],
            },
            PlanModule {
                key: stable_module.clone(),
                root_source: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "src/stable.nia",
                )
                .unwrap(),
                optimization: OptimizationMode::O2,
                imports: Vec::new(),
            },
        ],
        artifacts: vec![
            PlanArtifact {
                key: generated_artifact.clone(),
                root_module: generated_module,
                kind: PlanArtifactKind::Executable,
                output: LogicalPath::new(LogicalPathRoot::Build, "bin/generated").unwrap(),
                runtime: Runtime::Freestanding,
            },
            PlanArtifact {
                key: stable_artifact.clone(),
                root_module: stable_module,
                kind: PlanArtifactKind::Executable,
                output: LogicalPath::new(LogicalPathRoot::Build, "bin/stable").unwrap(),
                runtime: Runtime::Freestanding,
            },
        ],
        actions: vec![
            PlanAction {
                key: action("all"),
                kind: ActionKind::Aggregate,
            },
            PlanAction {
                key: action("emit-generated"),
                kind: ActionKind::CompilerEmit {
                    artifact: generated_artifact,
                    target: artifact_target.clone(),
                    static_archives: Vec::new(),
                },
            },
            PlanAction {
                key: action("emit-stable"),
                kind: ActionKind::CompilerEmit {
                    artifact: stable_artifact,
                    target: artifact_target,
                    static_archives: Vec::new(),
                },
            },
            PlanAction {
                key: action("generate-root"),
                kind: ActionKind::GeneratedFile {
                    output: LogicalPath::new(LogicalPathRoot::Build, "generated/main.nia").unwrap(),
                    contents: [
                        "using helper;\n",
                        &freestanding_source(&format!("_ = helper::value(); {generated_body}")),
                    ]
                    .concat()
                    .into_bytes(),
                },
            },
            PlanAction {
                key: action("generate-helper"),
                kind: ActionKind::GeneratedFile {
                    output: LogicalPath::new(LogicalPathRoot::Build, "generated/helper.nia")
                        .unwrap(),
                    contents: format!("pub fn value() i32 {{\n    {generated_helper_value}\n}}\n")
                        .into_bytes(),
                },
            },
        ],
        steps: vec![
            PlanStep {
                key: step("all"),
                action: action("all"),
                dependencies: vec![step("emit-generated"), step("emit-stable")],
            },
            PlanStep {
                key: step("emit-generated"),
                action: action("emit-generated"),
                dependencies: vec![step("generate-root"), step("generate-helper")],
            },
            PlanStep {
                key: step("emit-stable"),
                action: action("emit-stable"),
                dependencies: Vec::new(),
            },
            PlanStep {
                key: step("generate-root"),
                action: action("generate-root"),
                dependencies: Vec::new(),
            },
            PlanStep {
                key: step("generate-helper"),
                action: action("generate-helper"),
                dependencies: Vec::new(),
            },
        ],
        default_step: Some(step("all")),
        selected_step: None,
    })
    .unwrap()
}

fn write_compiler_check_source(invocation: &BuildInvocation, source: &str) {
    let path = invocation.package_root.join("src/main.nia");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

fn only_compiler_check_outcome(report: &ExecutionReport) -> &ActionCacheOutcome {
    assert_eq!(report.action_cache.len(), 1);
    assert_eq!(report.action_cache[0].action, action("check"));
    &report.action_cache[0].outcome
}

fn only_compiler_emit_outcome(report: &ExecutionReport) -> &ActionCacheOutcome {
    assert_eq!(report.action_cache.len(), 1);
    assert_eq!(report.action_cache[0].action, action("emit"));
    &report.action_cache[0].outcome
}

fn cache_outcome<'a>(report: &'a ExecutionReport, name: &str) -> &'a ActionCacheOutcome {
    &report
        .action_cache
        .iter()
        .find(|entry| entry.action.name() == name)
        .unwrap_or_else(|| panic!("missing action-cache outcome for `{name}`"))
        .outcome
}

fn freestanding_source(body: &str) -> String {
    [
        "using std::process;\n",
        "pub fn main(init: process::Init) process::ExitCode!() {\n",
        "    _ = init;\n",
        "    ",
        body,
        "\n}\n",
    ]
    .concat()
}

fn only_nested_cache_entry(namespace: &Path, extension: &str) -> PathBuf {
    let key_dir = fs::read_dir(namespace)
        .expect("read cache namespace")
        .next()
        .expect("cache key directory")
        .expect("read cache key directory")
        .path();
    fs::read_dir(key_dir)
        .expect("read cache key entries")
        .find_map(|entry| {
            let path = entry.expect("read cache entry").path();
            (path.extension().and_then(|value| value.to_str()) == Some(extension)).then_some(path)
        })
        .expect("cache entry")
}

fn assert_no_output_locks(invocation: &BuildInvocation) {
    let root = invocation.cache_dir.join("coordination/output-locks");
    if root.is_dir() {
        assert!(fs::read_dir(root).unwrap().next().is_none());
    }
}

#[test]
fn output_lock_keys_are_path_stable_and_domain_local() {
    let invocation = test_invocation();
    let first = LogicalPath::new(LogicalPathRoot::Build, "first/output").unwrap();
    let same = LogicalPath::new(LogicalPathRoot::Build, "first/output").unwrap();
    let second = LogicalPath::new(LogicalPathRoot::Build, "second/output").unwrap();

    assert_eq!(
        output_lock_path(&invocation.cache_dir, &first),
        output_lock_path(&invocation.cache_dir, &same)
    );
    assert_ne!(
        output_lock_path(&invocation.cache_dir, &first),
        output_lock_path(&invocation.cache_dir, &second)
    );
    assert!(
        output_lock_path(&invocation.cache_dir, &first)
            .starts_with(invocation.cache_dir.join("coordination/output-locks"))
    );
}

#[test]
fn output_coordination_serializes_conflicts_only() {
    let invocation = test_invocation();
    let blocked_output = LogicalPath::new(LogicalPathRoot::Build, "blocked/output.txt").unwrap();
    let held =
        ScopedFileLock::acquire(output_lock_path(&invocation.cache_dir, &blocked_output)).unwrap();

    let independent = generated_plan(&invocation, "independent/output.txt", b"independent");
    execute_build_plan(&independent, &invocation).unwrap();

    let conflicting = generated_plan(&invocation, "blocked/output.txt", b"conflicting");
    let concurrent_invocation = invocation.clone();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = execute_build_plan(&conflicting, &concurrent_invocation);
        finished_tx.send(result).unwrap();
    });

    assert!(
        finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    drop(held);
    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    handle.join().unwrap();
    assert_eq!(
        fs::read(invocation.build_dir.join("blocked/output.txt")).unwrap(),
        b"conflicting"
    );
    assert_no_output_locks(&invocation);
}

#[test]
fn output_coordination_failure_retains_action_and_paths() {
    let invocation = test_invocation();
    fs::create_dir_all(invocation.cache_dir.join("coordination")).unwrap();
    fs::write(
        invocation.cache_dir.join("coordination/output-locks"),
        b"occupied",
    )
    .unwrap();
    let plan = generated_plan(&invocation, "generated/output.txt", b"contents");

    let error = execute_build_plan(&plan, &invocation).unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::AcquireOutputLock {
            action,
            output,
            lock,
            ..
        } if action.name() == "generate"
            && output == invocation.build_dir.join("generated/output.txt")
            && lock.starts_with(invocation.cache_dir.join("coordination/output-locks"))
    ));
}

#[test]
fn generated_file_replaces_previous_content_atomically() {
    let invocation = test_invocation();
    let output = invocation.build_dir.join("generated/source.nia");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(&output, b"old").unwrap();
    let plan = generated_plan(&invocation, "generated/source.nia", b"new contents");

    let report = execute_build_plan(&plan, &invocation).unwrap();

    assert_eq!(report.actions, [action("generate")]);
    assert_eq!(fs::read(&output).unwrap(), b"new contents");
    assert_no_output_locks(&invocation);
    assert!(
        fs::read_dir(output.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".nia-generated-"))
    );
}

#[test]
fn generated_file_cache_reports_miss_hit_and_restores_output() {
    let invocation = test_invocation();
    let output = invocation.build_dir.join("generated/source.nia");
    let plan = generated_plan(&invocation, "generated/source.nia", b"source");

    let cold = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        cold.action_cache,
        [ActionCacheReport {
            action: action("generate"),
            outcome: ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound),
        }]
    );
    fs::remove_file(&output).unwrap();

    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        warm.action_cache,
        [ActionCacheReport {
            action: action("generate"),
            outcome: ActionCacheOutcome::Hit,
        }]
    );
    assert_eq!(fs::read(output).unwrap(), b"source");
    assert_no_output_locks(&invocation);
}

#[test]
fn generated_file_cache_reports_content_and_output_invalidation() {
    let invocation = test_invocation();
    let baseline = generated_plan(&invocation, "generated/source.nia", b"source");
    execute_build_plan(&baseline, &invocation).unwrap();

    let changed_contents = generated_plan(&invocation, "generated/source.nia", b"changed source");
    let content_report = execute_build_plan(&changed_contents, &invocation).unwrap();
    assert_eq!(
        content_report.action_cache[0].outcome,
        ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Contents,
        ]))
    );

    let other_invocation = test_invocation();
    let first_output = generated_plan(&other_invocation, "generated/first.nia", b"source");
    execute_build_plan(&first_output, &other_invocation).unwrap();
    let changed_output = generated_plan(&other_invocation, "generated/second.nia", b"source");
    let output_report = execute_build_plan(&changed_output, &other_invocation).unwrap();
    assert_eq!(
        output_report.action_cache[0].outcome,
        ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Output,
        ]))
    );
}

#[test]
fn compiler_check_cache_reports_cold_and_warm_execution() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, "fn main() i32 { 0 }");
    let plan = compiler_check_plan(&invocation, OptimizationMode::O2);

    let cold = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_compiler_check_outcome(&cold),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
    );

    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(only_compiler_check_outcome(&warm), &ActionCacheOutcome::Hit);
}

#[test]
fn compiler_check_cache_reports_source_and_optimization_invalidation() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, "fn main() i32 { 0 }");
    execute_build_plan(
        &compiler_check_plan(&invocation, OptimizationMode::O2),
        &invocation,
    )
    .unwrap();

    write_compiler_check_source(&invocation, "fn main() i32 { 1 }");
    let changed_source = execute_build_plan(
        &compiler_check_plan(&invocation, OptimizationMode::O2),
        &invocation,
    )
    .unwrap();
    assert_eq!(
        only_compiler_check_outcome(&changed_source),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Sources,
        ]))
    );

    let changed_optimization = execute_build_plan(
        &compiler_check_plan(&invocation, OptimizationMode::O0),
        &invocation,
    )
    .unwrap();
    assert_eq!(
        only_compiler_check_outcome(&changed_optimization),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Optimization,
        ]))
    );
}

#[test]
fn compiler_check_cache_reuses_relocated_sources() {
    let first = test_invocation();
    write_compiler_check_source(&first, "fn main() i32 { 0 }");
    execute_build_plan(&compiler_check_plan(&first, OptimizationMode::O2), &first).unwrap();

    let mut relocated = test_invocation();
    relocated.cache_dir = first.cache_dir.clone();
    write_compiler_check_source(&relocated, "fn main() i32 { 0 }");
    let report = execute_build_plan(
        &compiler_check_plan(&relocated, OptimizationMode::O2),
        &relocated,
    )
    .unwrap();

    assert_eq!(
        only_compiler_check_outcome(&report),
        &ActionCacheOutcome::Hit
    );
}

#[test]
fn compiler_check_cache_restores_semantic_provider_source_closure() {
    let invocation = test_invocation();
    write_compiler_check_source(
        &invocation,
        concat!(
            "using std::process;\n",
            "pub fn main(init: process::Init) process::ExitCode!() {\n",
            "    _ = init;\n",
            "    !()\n",
            "}\n",
        ),
    );
    let plan =
        compiler_check_plan_with_runtime(&invocation, OptimizationMode::O2, Runtime::Freestanding);

    let cold = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_compiler_check_outcome(&cold),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
    );
    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(only_compiler_check_outcome(&warm), &ActionCacheOutcome::Hit);
}

#[test]
fn compiler_check_cache_does_not_publish_warnings() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, "using std::collections;\nfn main() () {}\n");
    let plan = compiler_check_plan(&invocation, OptimizationMode::O2);

    for report in [
        execute_build_plan(&plan, &invocation).unwrap(),
        execute_build_plan(&plan, &invocation).unwrap(),
    ] {
        assert_eq!(
            only_compiler_check_outcome(&report),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Uncacheable)
        );
    }
}

#[test]
fn compiler_check_cache_retires_corruption_and_recompiles() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, "fn main() i32 { 0 }");
    let plan = compiler_check_plan(&invocation, OptimizationMode::O2);
    execute_build_plan(&plan, &invocation).unwrap();
    let namespace = invocation.cache_dir.join("actions/compiler-checks/v2");
    let key_dir = fs::read_dir(namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let entry = fs::read_dir(key_dir)
        .unwrap()
        .find_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|value| value.to_str()) == Some("entry")).then_some(path)
        })
        .unwrap();
    fs::write(entry, b"corrupt").unwrap();

    let report = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_compiler_check_outcome(&report),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Corrupt)
    );
    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(only_compiler_check_outcome(&warm), &ActionCacheOutcome::Hit);
}

#[test]
fn compiler_check_cache_never_publishes_missing_sources() {
    let invocation = test_invocation();
    let plan = compiler_check_plan(&invocation, OptimizationMode::O2);

    for _ in 0..2 {
        assert!(matches!(
            execute_build_plan(&plan, &invocation),
            Err(CoordinatorError::Driver { .. })
        ));
    }
    assert!(
        !invocation
            .cache_dir
            .join("actions/compiler-checks/v2")
            .exists()
    );
}

#[test]
fn compiler_emit_cache_reports_cold_hit_and_restores_deleted_output() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, &freestanding_source("!()"));
    let plan = compiler_emit_plan(
        &invocation,
        OptimizationMode::O2,
        Runtime::Freestanding,
        "bin/app",
    );
    let output = invocation.build_dir.join("bin/app");

    let cold = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&cold),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
    );
    let expected = fs::read(&output).expect("read cold executable");
    fs::remove_file(&output).expect("remove cold executable");

    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(only_compiler_emit_outcome(&warm), &ActionCacheOutcome::Hit);
    assert_eq!(
        fs::read(output).expect("read restored executable"),
        expected
    );
}

#[test]
fn object_set_emit_publishes_driver_object_directory_transactionally() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, &freestanding_source("!()"));
    let plan = object_set_plan(&invocation, "objects/app");
    let output = invocation.build_dir.join("objects/app");

    fs::create_dir_all(&output).expect("create stale object directory");
    fs::write(output.join("stale-codegen-unit.o"), b"stale").expect("write stale object");
    execute_build_plan(&plan, &invocation).unwrap();

    assert!(output.is_dir());
    assert!(!output.join("stale-codegen-unit.o").exists());
    let objects = fs::read_dir(&output)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert!(!objects.is_empty());
    assert!(objects.iter().all(|path| {
        fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
    }));
    assert_no_staged_command_directories(output.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn static_archive_emit_publishes_driver_archive_transactionally() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, &freestanding_source("!()"));
    let plan = static_archive_plan(&invocation, "lib/libapp.a");
    let output = invocation.build_dir.join("lib/libapp.a");

    fs::create_dir_all(output.parent().unwrap()).expect("create archive output directory");
    fs::write(&output, b"stale archive").expect("write stale archive");
    let cold = execute_build_plan(&plan, &invocation).unwrap();

    assert!(cold.action_cache.is_empty());
    let expected = fs::read(&output).expect("read static archive");
    assert!(expected.starts_with(b"!<arch>\n"));
    fs::remove_file(&output).expect("remove static archive");

    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert!(warm.action_cache.is_empty());
    assert_eq!(fs::read(&output).expect("read restored archive"), expected);
    assert_no_staged_command_directories(output.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn install_artifact_copies_and_replaces_an_executable_transactionally() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, &freestanding_source("!()"));
    let plan = install_executable_plan(&invocation);
    let source = invocation.build_dir.join("bin/app");
    let destination = invocation.build_dir.join("install/custom-app");

    execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o111,
            0
        );
    }

    fs::write(&destination, b"stale installed executable").unwrap();
    execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
    assert_no_output_locks(&invocation);
}

#[test]
fn compiler_emit_cache_classifies_source_option_output_and_artifact_changes() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, &freestanding_source("!()"));
    execute_build_plan(
        &compiler_emit_plan(
            &invocation,
            OptimizationMode::O2,
            Runtime::Freestanding,
            "bin/app",
        ),
        &invocation,
    )
    .unwrap();

    write_compiler_check_source(
        &invocation,
        &freestanding_source("let value = 1; _ = value; !()"),
    );
    let changed_source = execute_build_plan(
        &compiler_emit_plan(
            &invocation,
            OptimizationMode::O2,
            Runtime::Freestanding,
            "bin/app",
        ),
        &invocation,
    )
    .unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&changed_source),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Sources,
        ]))
    );

    let changed_optimization = execute_build_plan(
        &compiler_emit_plan(
            &invocation,
            OptimizationMode::O0,
            Runtime::Freestanding,
            "bin/app",
        ),
        &invocation,
    )
    .unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&changed_optimization),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Optimization,
        ]))
    );

    let changed_output = execute_build_plan(
        &compiler_emit_plan(
            &invocation,
            OptimizationMode::O0,
            Runtime::Freestanding,
            "bin/other",
        ),
        &invocation,
    )
    .unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&changed_output),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Output,
        ]))
    );

    let changed_artifact = execute_build_plan(
        &compiler_emit_plan(
            &invocation,
            OptimizationMode::O0,
            Runtime::Bare,
            "bin/other",
        ),
        &invocation,
    )
    .unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&changed_artifact),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Artifact,
        ]))
    );
}

#[test]
fn generated_and_package_source_edits_invalidate_only_their_compiler_closures() {
    let invocation = test_invocation();
    let stable_path = invocation.package_root.join("src/stable.nia");
    fs::create_dir_all(stable_path.parent().unwrap()).unwrap();
    fs::write(&stable_path, freestanding_source("!()")).unwrap();
    let baseline = mixed_generated_source_emit_plan(&invocation, "!()", 1);

    execute_build_plan(&baseline, &invocation).unwrap();
    let warm = execute_build_plan(&baseline, &invocation).unwrap();
    for name in [
        "generate-root",
        "generate-helper",
        "emit-generated",
        "emit-stable",
    ] {
        assert_eq!(cache_outcome(&warm, name), &ActionCacheOutcome::Hit);
    }

    let changed_root =
        mixed_generated_source_emit_plan(&invocation, "let value = 1; _ = value; !()", 1);
    let root_report = execute_build_plan(&changed_root, &invocation).unwrap();
    assert_eq!(
        cache_outcome(&root_report, "generate-root"),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Contents,
        ]))
    );
    assert_eq!(
        cache_outcome(&root_report, "generate-helper"),
        &ActionCacheOutcome::Hit
    );
    assert_eq!(
        cache_outcome(&root_report, "emit-generated"),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Sources,
        ]))
    );
    assert_eq!(
        cache_outcome(&root_report, "emit-stable"),
        &ActionCacheOutcome::Hit
    );

    let changed_import =
        mixed_generated_source_emit_plan(&invocation, "let value = 1; _ = value; !()", 2);
    let import_report = execute_build_plan(&changed_import, &invocation).unwrap();
    assert_eq!(
        cache_outcome(&import_report, "generate-root"),
        &ActionCacheOutcome::Hit
    );
    assert_eq!(
        cache_outcome(&import_report, "generate-helper"),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Contents,
        ]))
    );
    assert_eq!(
        cache_outcome(&import_report, "emit-generated"),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Sources,
        ]))
    );
    assert_eq!(
        cache_outcome(&import_report, "emit-stable"),
        &ActionCacheOutcome::Hit
    );

    fs::write(
        stable_path,
        freestanding_source("let value = 2; _ = value; !()"),
    )
    .unwrap();
    let source_report = execute_build_plan(&changed_import, &invocation).unwrap();
    assert_eq!(
        cache_outcome(&source_report, "generate-root"),
        &ActionCacheOutcome::Hit
    );
    assert_eq!(
        cache_outcome(&source_report, "generate-helper"),
        &ActionCacheOutcome::Hit
    );
    assert_eq!(
        cache_outcome(&source_report, "emit-generated"),
        &ActionCacheOutcome::Hit
    );
    assert_eq!(
        cache_outcome(&source_report, "emit-stable"),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Sources,
        ]))
    );
}

#[test]
fn compiler_emit_cache_retires_corrupt_records_and_driver_references() {
    let invocation = test_invocation();
    write_compiler_check_source(&invocation, &freestanding_source("!()"));
    let plan = compiler_emit_plan(
        &invocation,
        OptimizationMode::O2,
        Runtime::Freestanding,
        "bin/app",
    );
    execute_build_plan(&plan, &invocation).unwrap();

    let action_entry = only_nested_cache_entry(
        &invocation.cache_dir.join("actions/compiler-emits/v3"),
        "entry",
    );
    fs::write(&action_entry, b"corrupt").expect("corrupt compiler emit action entry");
    let corrupt_action = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&corrupt_action),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Corrupt)
    );
    assert_eq!(
        only_compiler_emit_outcome(&execute_build_plan(&plan, &invocation).unwrap()),
        &ActionCacheOutcome::Hit
    );

    let link_entry =
        only_nested_cache_entry(&invocation.cache_dir.join("artifacts/links/v3"), "link");
    fs::write(&link_entry, b"corrupt").expect("corrupt Driver link entry");
    let corrupt_reference = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&corrupt_reference),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Corrupt)
    );
    assert_eq!(
        only_compiler_emit_outcome(&execute_build_plan(&plan, &invocation).unwrap()),
        &ActionCacheOutcome::Hit
    );

    let link_entry =
        only_nested_cache_entry(&invocation.cache_dir.join("artifacts/links/v3"), "link");
    fs::remove_file(link_entry).expect("remove Driver link entry");
    let missing_reference = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_compiler_emit_outcome(&missing_reference),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
    );
    assert_eq!(
        only_compiler_emit_outcome(&execute_build_plan(&plan, &invocation).unwrap()),
        &ActionCacheOutcome::Hit
    );
}

#[test]
fn compiler_emit_cache_reuses_relocated_dynamic_source_closure() {
    let first = test_invocation();
    write_compiler_check_source(&first, &freestanding_source("!()"));
    execute_build_plan(
        &compiler_emit_plan(
            &first,
            OptimizationMode::O2,
            Runtime::Freestanding,
            "bin/app",
        ),
        &first,
    )
    .unwrap();

    let mut relocated = test_invocation();
    relocated.cache_dir = first.cache_dir.clone();
    write_compiler_check_source(&relocated, &freestanding_source("!()"));
    let report = execute_build_plan(
        &compiler_emit_plan(
            &relocated,
            OptimizationMode::O2,
            Runtime::Freestanding,
            "bin/app",
        ),
        &relocated,
    )
    .unwrap();

    assert_eq!(
        only_compiler_emit_outcome(&report),
        &ActionCacheOutcome::Hit
    );
    assert!(relocated.build_dir.join("bin/app").is_file());
}

#[test]
fn compiler_emit_cache_does_not_publish_warnings() {
    let invocation = test_invocation();
    write_compiler_check_source(
        &invocation,
        &format!("using std::build;\n{}", freestanding_source("!()")),
    );
    let plan = compiler_emit_plan(
        &invocation,
        OptimizationMode::O2,
        Runtime::Freestanding,
        "bin/app",
    );

    for report in [
        execute_build_plan(&plan, &invocation).unwrap(),
        execute_build_plan(&plan, &invocation).unwrap(),
    ] {
        assert_eq!(
            only_compiler_emit_outcome(&report),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Uncacheable)
        );
    }
    assert!(
        !invocation
            .cache_dir
            .join("actions/compiler-emits/v3")
            .exists()
    );
}

#[test]
fn generated_file_cache_read_failure_is_an_explicit_nonfatal_miss() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.cache_dir).unwrap();
    fs::write(invocation.cache_dir.join("actions"), b"occupied").unwrap();
    let plan = generated_plan(&invocation, "generated/source.nia", b"source");

    let report = execute_build_plan(&plan, &invocation).unwrap();

    assert_eq!(
        report.action_cache[0].outcome,
        ActionCacheOutcome::Miss(ActionCacheMissReason::ReadError)
    );
    assert_eq!(
        fs::read(invocation.build_dir.join("generated/source.nia")).unwrap(),
        b"source"
    );
}

#[test]
fn failed_generated_file_publication_cleans_temporary_output() {
    let invocation = test_invocation();
    let output = invocation.build_dir.join("occupied");
    fs::create_dir_all(&output).unwrap();
    let plan = generated_plan(&invocation, "occupied", b"contents");

    let error = execute_build_plan(&plan, &invocation).unwrap_err();

    assert!(
        matches!(error, CoordinatorError::GeneratedFileIo { action, .. } if action.name() == "generate")
    );
    assert!(output.is_dir());
    assert_no_output_locks(&invocation);
    assert!(fs::read_dir(&invocation.build_dir).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".nia-generated-")
    }));
}

fn external_action() -> PlanAction {
    PlanAction {
        key: action("run"),
        kind: ActionKind::ExternalCommand {
            resource_class: ActionResourceClass::Conservative,
            environment_policy: crate::CommandEnvironmentPolicy::Inherit,
            cache_policy: crate::CommandCachePolicy::Uncacheable,
            program: CommandProgram::Search("sh".to_string()),
            arguments: Vec::new(),
            working_directory: LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "")
                .unwrap(),
            environment: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        },
    }
}

fn staged_command_plan(invocation: &BuildInvocation, script: &str, output: &str) -> BuildPlan {
    staged_command_plan_outputs(invocation, script, &[output])
}

fn staged_command_plan_outputs(
    invocation: &BuildInvocation,
    script: &str,
    outputs: &[&str],
) -> BuildPlan {
    let outputs = outputs
        .iter()
        .map(|output| LogicalPath::new(LogicalPathRoot::Build, output).unwrap())
        .collect::<Vec<_>>();
    let mut arguments = vec![
        CommandArgument::Literal("-c".to_string()),
        CommandArgument::Literal(script.to_string()),
        CommandArgument::Literal("nia-build-tool".to_string()),
    ];
    arguments.extend(outputs.iter().cloned().map(CommandArgument::OutputPath));
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target_spec(invocation.toolchain.host_target()),
        artifact_target: target_spec(invocation.toolchain.artifact_target()),
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions: vec![PlanAction {
            key: action("tool"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Conservative,
                environment_policy: crate::CommandEnvironmentPolicy::Inherit,
                cache_policy: crate::CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("sh".to_string()),
                arguments,
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs,
            },
        }],
        steps: vec![PlanStep {
            key: step("tool"),
            action: action("tool"),
            dependencies: Vec::new(),
        }],
        default_step: Some(step("tool")),
        selected_step: None,
    })
    .unwrap()
}

fn cacheable_command_plan(invocation: &BuildInvocation, mode: &str) -> BuildPlan {
    let input = LogicalPath::new(
        LogicalPathRoot::Package(PackageKey::root()),
        "tool-input.txt",
    )
    .unwrap();
    cacheable_command_plan_with_input(invocation, mode, input)
}

fn cacheable_directory_command_plan(invocation: &BuildInvocation, mode: &str) -> BuildPlan {
    let input =
        LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "input-dir").unwrap();
    cacheable_command_plan_with_input(invocation, mode, input)
}

fn cacheable_command_plan_with_input(
    invocation: &BuildInvocation,
    mode: &str,
    input: LogicalPath,
) -> BuildPlan {
    let first = LogicalPath::new(LogicalPathRoot::Build, "tool/first.txt").unwrap();
    let second = LogicalPath::new(LogicalPathRoot::Build, "tool/second.txt").unwrap();
    BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
                root: String::new(),
            }],
            host_target: target_spec(invocation.toolchain.host_target()),
            artifact_target: target_spec(invocation.toolchain.artifact_target()),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: action("cached-tool"),
                kind: ActionKind::ExternalCommand {
                    resource_class: ActionResourceClass::Io,
                    environment_policy: CommandEnvironmentPolicy::Clear,
                    cache_policy: CommandCachePolicy::DeclaredInputs,
                    program: CommandProgram::Search("sh".to_string()),
                    arguments: vec![
                        CommandArgument::Literal("-c".to_string()),
                        CommandArgument::Literal(
                            "if test -d \"$1\"; then printf '%s:DIRECTORY' \"$MODE\" > \"$2\"; else printf '%s:' \"$MODE\" > \"$2\"; tr a-z A-Z < \"$1\" >> \"$2\"; fi; printf meta > \"$3\""
                                .to_string(),
                        ),
                        CommandArgument::Literal("nia-cached-tool".to_string()),
                        CommandArgument::InputPath(input.clone()),
                        CommandArgument::OutputPath(first.clone()),
                        CommandArgument::OutputPath(second.clone()),
                    ],
                    working_directory: LogicalPath::new(
                        LogicalPathRoot::Package(PackageKey::root()),
                        "",
                    )
                    .unwrap(),
                    environment: vec![EnvironmentInput {
                        name: "MODE".to_string(),
                        value: Some(mode.to_string()),
                    }],
                    inputs: vec![input],
                    outputs: vec![first, second],
                },
            }],
            steps: vec![PlanStep {
                key: step("cached-tool"),
                action: action("cached-tool"),
                dependencies: Vec::new(),
            }],
            default_step: Some(step("cached-tool")),
            selected_step: None,
        })
        .unwrap()
}

fn cacheable_path_tool_plan(invocation: &BuildInvocation) -> BuildPlan {
    let output = LogicalPath::new(LogicalPathRoot::Build, "path-tool.txt").unwrap();
    BuildPlan::freeze(BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target_spec(invocation.toolchain.host_target()),
        artifact_target: target_spec(invocation.toolchain.artifact_target()),
        modules: Vec::new(),
        artifacts: Vec::new(),
        actions: vec![PlanAction {
            key: action("path-tool"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy: CommandEnvironmentPolicy::Clear,
                cache_policy: CommandCachePolicy::DeclaredInputs,
                program: CommandProgram::Path(
                    LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "tool.sh")
                        .unwrap(),
                ),
                arguments: vec![CommandArgument::OutputPath(output.clone())],
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![output],
            },
        }],
        steps: vec![PlanStep {
            key: step("path-tool"),
            action: action("path-tool"),
            dependencies: Vec::new(),
        }],
        default_step: Some(step("path-tool")),
        selected_step: None,
    })
    .unwrap()
}

fn only_external_cache_outcome(report: &ExecutionReport) -> &ActionCacheOutcome {
    assert_eq!(report.action_cache.len(), 1);
    &report.action_cache[0].outcome
}

#[test]
fn cacheable_external_command_restores_all_outputs_without_execution() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    fs::write(invocation.package_root.join("tool-input.txt"), b"source").unwrap();
    let plan = cacheable_command_plan(&invocation, "cold");

    let cold = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_external_cache_outcome(&cold),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
    );
    let first = invocation.build_dir.join("tool/first.txt");
    let second = invocation.build_dir.join("tool/second.txt");
    assert_eq!(fs::read(&first).unwrap(), b"cold:SOURCE");
    assert_eq!(fs::read(&second).unwrap(), b"meta");

    fs::write(&first, b"stale first").unwrap();
    fs::remove_file(&second).unwrap();
    let warm = execute_build_plan(&plan, &invocation).unwrap();

    assert_eq!(only_external_cache_outcome(&warm), &ActionCacheOutcome::Hit);
    assert_eq!(fs::read(&first).unwrap(), b"cold:SOURCE");
    assert_eq!(fs::read(&second).unwrap(), b"meta");
    assert_no_staged_command_directories(first.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn external_command_cache_reuses_relocated_logical_inputs_and_outputs() {
    let first = test_invocation();
    fs::create_dir_all(&first.package_root).unwrap();
    fs::write(first.package_root.join("tool-input.txt"), b"relocated").unwrap();
    let first_plan = cacheable_command_plan(&first, "shared");
    let cold = execute_build_plan(&first_plan, &first).unwrap();
    assert!(matches!(
        only_external_cache_outcome(&cold),
        ActionCacheOutcome::Miss(_)
    ));

    let mut second = test_invocation();
    second.cache_dir = first.cache_dir.clone();
    fs::create_dir_all(&second.package_root).unwrap();
    fs::write(second.package_root.join("tool-input.txt"), b"relocated").unwrap();
    let second_plan = cacheable_command_plan(&second, "shared");
    let warm = execute_build_plan(&second_plan, &second).unwrap();

    assert_eq!(only_external_cache_outcome(&warm), &ActionCacheOutcome::Hit);
    assert_eq!(
        fs::read(second.build_dir.join("tool/first.txt")).unwrap(),
        b"shared:RELOCATED"
    );
    assert_eq!(
        fs::read(second.build_dir.join("tool/second.txt")).unwrap(),
        b"meta"
    );
}

#[test]
fn external_command_cache_classifies_input_and_environment_changes() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let input = invocation.package_root.join("tool-input.txt");
    fs::write(&input, b"first").unwrap();
    let initial = cacheable_command_plan(&invocation, "one");
    execute_build_plan(&initial, &invocation).unwrap();

    fs::write(&input, b"second").unwrap();
    let changed_input = execute_build_plan(&initial, &invocation).unwrap();
    assert_eq!(
        only_external_cache_outcome(&changed_input),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Inputs,
        ]))
    );
    assert_eq!(
        fs::read(invocation.build_dir.join("tool/first.txt")).unwrap(),
        b"one:SECOND"
    );

    let changed_environment =
        execute_build_plan(&cacheable_command_plan(&invocation, "two"), &invocation).unwrap();
    assert_eq!(
        only_external_cache_outcome(&changed_environment),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Environment,
        ]))
    );
    assert_eq!(
        fs::read(invocation.build_dir.join("tool/first.txt")).unwrap(),
        b"two:SECOND"
    );
}

#[test]
fn external_command_cache_fingerprints_directory_inputs() {
    let invocation = test_invocation();
    let input = invocation.package_root.join("input-dir");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("unit-a.o"), b"first").unwrap();
    let plan = cacheable_directory_command_plan(&invocation, "directory");

    let cold = execute_build_plan(&plan, &invocation).unwrap();
    assert!(matches!(
        only_external_cache_outcome(&cold),
        ActionCacheOutcome::Miss(_)
    ));
    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(only_external_cache_outcome(&warm), &ActionCacheOutcome::Hit);

    fs::write(input.join("unit-a.o"), b"changed").unwrap();
    let changed = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_external_cache_outcome(&changed),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::Inputs,
        ]))
    );
}

#[test]
fn streamed_directory_identity_preserves_the_registered_encoding() {
    fn legacy_encoding(path: &Path) -> Vec<u8> {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by(|left, right| {
            left.file_name()
                .as_encoded_bytes()
                .cmp(right.file_name().as_encoded_bytes())
        });
        let mut encoded = b"NIA-DIR1\0".to_vec();
        encoded.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        for entry in entries {
            let name = entry.file_name();
            let name = name.as_encoded_bytes();
            encoded.extend_from_slice(&(name.len() as u64).to_le_bytes());
            encoded.extend_from_slice(name);
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_file() {
                let bytes = fs::read(path).unwrap();
                encoded.push(0);
                encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                encoded.extend_from_slice(&bytes);
            } else {
                let nested = legacy_encoding(&path);
                encoded.push(1);
                encoded.extend_from_slice(&(nested.len() as u64).to_le_bytes());
                encoded.extend_from_slice(&nested);
            }
        }
        encoded
    }

    let invocation = test_invocation();
    let input = invocation.package_root.join("input-dir");
    fs::create_dir_all(input.join("nested")).unwrap();
    fs::write(input.join("z.o"), b"last").unwrap();
    fs::write(input.join("nested/a.o"), b"first").unwrap();
    let expected = ExternalCommandContentIdentity::input_from_bytes(&legacy_encoding(&input));

    let actual = read_external_identity_input(&external_action(), &input, "test directory")
        .expect("stream directory identity");

    assert_eq!(actual, expected);
}

#[cfg(unix)]
#[test]
fn external_command_cache_rejects_directory_symlink_inputs() {
    let invocation = test_invocation();
    let input = invocation.package_root.join("input-dir");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("unit-a.o"), b"first").unwrap();
    std::os::unix::fs::symlink("unit-a.o", input.join("link.o")).unwrap();
    let plan = cacheable_directory_command_plan(&invocation, "directory");

    let error = execute_build_plan(&plan, &invocation).unwrap_err();
    assert!(matches!(
        error,
        CoordinatorError::ExternalCommandIo { error, .. }
            if error.kind() == io::ErrorKind::InvalidData
    ));
}

#[test]
fn corrupt_external_command_record_is_retired_and_rebuilt() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    fs::write(invocation.package_root.join("tool-input.txt"), b"source").unwrap();
    let plan = cacheable_command_plan(&invocation, "repair");
    execute_build_plan(&plan, &invocation).unwrap();
    let namespace = invocation.cache_dir.join("actions/external-commands/v3");
    let key_directory = fs::read_dir(namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let entry = fs::read_dir(key_directory)
        .unwrap()
        .find_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|value| value.to_str()) == Some("entry")).then_some(path)
        })
        .unwrap();
    fs::write(&entry, b"corrupt").unwrap();

    let repaired = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(
        only_external_cache_outcome(&repaired),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Corrupt)
    );
    let warm = execute_build_plan(&plan, &invocation).unwrap();
    assert_eq!(only_external_cache_outcome(&warm), &ActionCacheOutcome::Hit);
}

#[cfg(unix)]
#[test]
fn external_command_cache_hashes_resolved_tool_bytes() {
    use std::os::unix::fs::PermissionsExt as _;

    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let tool = invocation.package_root.join("tool.sh");
    fs::write(&tool, b"#!/bin/sh\nprintf first > \"$1\"\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let plan = cacheable_path_tool_plan(&invocation);
    execute_build_plan(&plan, &invocation).unwrap();

    fs::write(&tool, b"#!/bin/sh\nprintf second > \"$1\"\n").unwrap();
    let changed = execute_build_plan(&plan, &invocation).unwrap();

    assert_eq!(
        only_external_cache_outcome(&changed),
        &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
            crate::ActionCacheInvalidation::ExternalTool,
        ]))
    );
    assert_eq!(
        fs::read(invocation.build_dir.join("path-tool.txt")).unwrap(),
        b"second"
    );
}

fn assert_no_staged_command_directories(parent: &Path) {
    assert!(fs::read_dir(parent).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".nia-command-")
    }));
}

fn assert_no_output_transaction_journals(invocation: &BuildInvocation) {
    let root = invocation.build_dir.join(".nia-transactions/v2");
    if root.is_dir() {
        assert!(fs::read_dir(root).unwrap().next().is_none());
    }
}

fn prepare_test_staged_outputs(
    invocation: &BuildInvocation,
    destinations: &[PathBuf],
) -> Result<StagedOutputTransaction, CoordinatorError> {
    let outputs = destinations
        .iter()
        .cloned()
        .map(|destination| (destination, TransactionOutputKind::File))
        .collect::<Vec<_>>();
    prepare_test_typed_staged_outputs(invocation, &outputs)
}

fn prepare_test_typed_staged_outputs(
    invocation: &BuildInvocation,
    outputs: &[(PathBuf, TransactionOutputKind)],
) -> Result<StagedOutputTransaction, CoordinatorError> {
    let logical = outputs
        .iter()
        .map(|(destination, _)| {
            let relative = destination.strip_prefix(&invocation.build_dir).unwrap();
            let path = relative
                .components()
                .map(|component| component.as_os_str().to_str().unwrap())
                .collect::<Vec<_>>()
                .join("/");
            LogicalPath::new(LogicalPathRoot::Build, &path).unwrap()
        })
        .collect::<Vec<_>>();
    let resolved = logical
        .iter()
        .zip(outputs.iter())
        .map(|(logical, (destination, kind))| ResolvedTransactionOutput {
            logical,
            destination: destination.clone(),
            kind: *kind,
        })
        .collect::<Vec<_>>();
    prepare_typed_staged_outputs(&external_action(), &invocation.build_dir, &resolved)
}

#[test]
fn stale_pid_only_staging_names_do_not_exhaust_new_process_generation() {
    let invocation = test_invocation();
    let output = invocation.build_dir.join("tool/result.txt");
    let parent = output.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    for sequence in 0..128 {
        fs::create_dir(parent.join(format!(
            ".nia-command-{}-{sequence}.stage",
            std::process::id()
        )))
        .unwrap();
    }

    let staged = prepare_test_staged_outputs(&invocation, &[output]).unwrap();

    let name = staged.directory.file_name().unwrap().to_string_lossy();
    let owner = ProcessIdentity::current();
    assert!(name.starts_with(&format!(".nia-command-{}-{}-", owner.pid, owner.start_time)));
    cleanup_staged_outputs(&external_action(), staged, None).unwrap();
}

#[test]
fn external_command_publishes_one_declared_output_atomically() {
    let invocation = test_invocation();
    let output = invocation.build_dir.join("tool/result.txt");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(&output, b"old").unwrap();
    let plan = staged_command_plan(&invocation, "printf new > \"$1\"", "tool/result.txt");

    let report = execute_build_plan(&plan, &invocation).unwrap();

    assert_eq!(report.actions, [action("tool")]);
    assert_eq!(fs::read(&output).unwrap(), b"new");
    assert_no_staged_command_directories(output.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn external_command_publishes_multiple_outputs_as_one_transaction() {
    let invocation = test_invocation();
    let first = invocation.build_dir.join("tool/first.txt");
    let second = invocation.build_dir.join("other/second.txt");
    fs::create_dir_all(first.parent().unwrap()).unwrap();
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    fs::write(&first, b"old first").unwrap();
    fs::write(&second, b"old second").unwrap();
    let plan = staged_command_plan_outputs(
        &invocation,
        "printf 'new first' > \"$1\"; printf 'new second' > \"$2\"",
        &["tool/first.txt", "other/second.txt"],
    );

    let report = execute_build_plan(&plan, &invocation).unwrap();

    assert_eq!(report.actions, [action("tool")]);
    assert_eq!(fs::read(&first).unwrap(), b"new first");
    assert_eq!(fs::read(&second).unwrap(), b"new second");
    assert_no_staged_command_directories(second.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn directory_output_atomically_replaces_previous_tree() {
    let invocation = test_invocation();
    let destination = invocation.build_dir.join("tool/objects");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("old.o"), b"old").unwrap();
    let staged = prepare_test_typed_staged_outputs(
        &invocation,
        &[(destination.clone(), TransactionOutputKind::Directory)],
    )
    .unwrap();
    fs::create_dir(&staged.outputs[0].temporary).unwrap();
    fs::create_dir(staged.outputs[0].temporary.join("nested")).unwrap();
    fs::write(staged.outputs[0].temporary.join("new.o"), b"new").unwrap();
    fs::write(staged.outputs[0].temporary.join("nested/more.o"), b"more").unwrap();

    publish_staged_outputs(&external_action(), staged).unwrap();

    assert_eq!(fs::read(destination.join("new.o")).unwrap(), b"new");
    assert_eq!(
        fs::read(destination.join("nested/more.o")).unwrap(),
        b"more"
    );
    assert!(!destination.join("old.o").exists());
    assert_no_staged_command_directories(destination.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn partial_directory_commit_restores_previous_and_absent_trees() {
    let invocation = test_invocation();
    let previous = invocation.build_dir.join("tool/previous-objects");
    let absent = invocation.build_dir.join("tool/new-objects");
    fs::create_dir_all(&previous).unwrap();
    fs::write(previous.join("old.o"), b"old").unwrap();
    let staged = prepare_test_typed_staged_outputs(
        &invocation,
        &[
            (previous.clone(), TransactionOutputKind::Directory),
            (absent.clone(), TransactionOutputKind::Directory),
        ],
    )
    .unwrap();
    for output in &staged.outputs {
        fs::create_dir(&output.temporary).unwrap();
        fs::write(output.temporary.join("new.o"), b"new").unwrap();
    }

    let error = publish_staged_outputs_with(&external_action(), staged, |index| {
        if index == 1 {
            Err(io::Error::other("injected directory commit failure"))
        } else {
            Ok(())
        }
    })
    .unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::StagedOutput {
            operation: "commit transaction entry for",
            ..
        }
    ));
    assert_eq!(fs::read(previous.join("old.o")).unwrap(), b"old");
    assert!(!previous.join("new.o").exists());
    assert!(!absent.exists());
    assert_no_staged_command_directories(previous.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn wrong_previous_directory_type_is_rejected_without_replacement() {
    let invocation = test_invocation();
    let destination = invocation.build_dir.join("tool/objects");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(&destination, b"accepted file").unwrap();
    let staged = prepare_test_typed_staged_outputs(
        &invocation,
        &[(destination.clone(), TransactionOutputKind::Directory)],
    )
    .unwrap();
    fs::create_dir(&staged.outputs[0].temporary).unwrap();
    fs::write(staged.outputs[0].temporary.join("new.o"), b"new").unwrap();

    let error = publish_staged_outputs(&external_action(), staged).unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::StagedOutput {
            operation: "validate previous",
            ..
        }
    ));
    assert_eq!(fs::read(&destination).unwrap(), b"accepted file");
    assert_no_staged_command_directories(destination.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[cfg(unix)]
#[test]
fn staged_directory_with_symlink_is_rejected_before_replacement() {
    use std::os::unix::fs::symlink;

    let invocation = test_invocation();
    let destination = invocation.build_dir.join("tool/objects");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("old.o"), b"old").unwrap();
    let staged = prepare_test_typed_staged_outputs(
        &invocation,
        &[(destination.clone(), TransactionOutputKind::Directory)],
    )
    .unwrap();
    fs::create_dir(&staged.outputs[0].temporary).unwrap();
    fs::write(staged.outputs[0].temporary.join("new.o"), b"new").unwrap();
    symlink("new.o", staged.outputs[0].temporary.join("object-alias.o")).unwrap();

    let error = publish_staged_outputs(&external_action(), staged).unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::StagedOutput {
            operation: "validate and sync staged",
            ..
        }
    ));
    assert_eq!(fs::read(destination.join("old.o")).unwrap(), b"old");
    assert!(!destination.join("new.o").exists());
    assert_no_staged_command_directories(destination.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn missing_transaction_output_preserves_every_previous_output() {
    let invocation = test_invocation();
    let first = invocation.build_dir.join("first.txt");
    let second = invocation.build_dir.join("second.txt");
    fs::create_dir_all(&invocation.build_dir).unwrap();
    fs::write(&first, b"accepted first").unwrap();
    fs::write(&second, b"accepted second").unwrap();
    let plan = staged_command_plan_outputs(
        &invocation,
        "printf 'rejected first' > \"$1\"",
        &["first.txt", "second.txt"],
    );

    let error = execute_build_plan(&plan, &invocation).unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::StagedOutput {
            operation: "validate and sync staged",
            ..
        }
    ));
    assert_eq!(fs::read(&first).unwrap(), b"accepted first");
    assert_eq!(fs::read(&second).unwrap(), b"accepted second");
    assert_no_staged_command_directories(&invocation.build_dir);
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn partial_transaction_commit_restores_old_and_absent_destinations() {
    let invocation = test_invocation();
    let first = invocation.build_dir.join("first.txt");
    let absent = invocation.build_dir.join("absent.txt");
    let last = invocation.build_dir.join("last.txt");
    fs::create_dir_all(&invocation.build_dir).unwrap();
    fs::write(&first, b"old first").unwrap();
    fs::write(&last, b"old last").unwrap();
    let staged =
        prepare_test_staged_outputs(&invocation, &[first.clone(), absent.clone(), last.clone()])
            .unwrap();
    fs::write(&staged.outputs[0].temporary, b"new first").unwrap();
    fs::write(&staged.outputs[1].temporary, b"new absent").unwrap();
    fs::write(&staged.outputs[2].temporary, b"new last").unwrap();

    let error = publish_staged_outputs_with(&external_action(), staged, |index| {
        if index == 2 {
            Err(io::Error::other("injected transaction commit failure"))
        } else {
            Ok(())
        }
    })
    .unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::StagedOutput {
            operation: "commit transaction entry for",
            ..
        }
    ));
    assert_eq!(fs::read(&first).unwrap(), b"old first");
    assert!(!absent.exists());
    assert_eq!(fs::read(&last).unwrap(), b"old last");
    assert_no_staged_command_directories(&invocation.build_dir);
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn coordinator_recovers_interrupted_transaction_before_dispatch() {
    let invocation = test_invocation();
    let previous = invocation.build_dir.join("tool/previous.txt");
    let absent = invocation.build_dir.join("other/absent.txt");
    fs::create_dir_all(previous.parent().unwrap()).unwrap();
    fs::create_dir_all(absent.parent().unwrap()).unwrap();
    fs::write(&previous, b"old").unwrap();
    let staged =
        prepare_test_staged_outputs(&invocation, &[previous.clone(), absent.clone()]).unwrap();
    fs::write(&staged.outputs[0].temporary, b"new old").unwrap();
    fs::write(&staged.outputs[1].temporary, b"new absent").unwrap();
    staged.journal.mark_prepared(&[true, false]).unwrap();
    fs::rename(&previous, &staged.outputs[0].backup).unwrap();
    fs::rename(&staged.outputs[0].temporary, &previous).unwrap();
    fs::rename(&staged.outputs[1].temporary, &absent).unwrap();
    let plan = generated_plan(&invocation, "after-recovery.txt", b"continued");

    execute_build_plan(&plan, &invocation).unwrap();

    assert_eq!(fs::read(previous).unwrap(), b"old");
    assert!(!absent.exists());
    assert_eq!(
        fs::read(invocation.build_dir.join("after-recovery.txt")).unwrap(),
        b"continued"
    );
    assert_no_staged_command_directories(&invocation.build_dir.join("tool"));
}

#[test]
fn failed_transaction_acceptance_restores_every_destination() {
    let invocation = test_invocation();
    let first = invocation.build_dir.join("first.txt");
    let second = invocation.build_dir.join("second.txt");
    fs::create_dir_all(&invocation.build_dir).unwrap();
    fs::write(&first, b"old first").unwrap();
    fs::write(&second, b"old second").unwrap();
    let staged =
        prepare_test_staged_outputs(&invocation, &[first.clone(), second.clone()]).unwrap();
    fs::write(&staged.outputs[0].temporary, b"new first").unwrap();
    fs::write(&staged.outputs[1].temporary, b"new second").unwrap();
    let committed_directory = staged.committed_directory.clone();

    let error = publish_staged_outputs_with(&external_action(), staged, |index| {
        if index == 1 {
            fs::create_dir(&committed_directory)?;
            fs::write(committed_directory.join("occupied"), b"occupied")?;
        }
        Ok(())
    })
    .unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::StagedOutput {
            operation: "mark transaction committed at",
            ..
        }
    ));
    assert_eq!(fs::read(&first).unwrap(), b"old first");
    assert_eq!(fs::read(&second).unwrap(), b"old second");
    fs::remove_dir_all(committed_directory).unwrap();
    assert_no_staged_command_directories(&invocation.build_dir);
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn failed_external_command_retires_staging_and_preserves_old_output() {
    let invocation = test_invocation();
    let output = invocation.build_dir.join("tool/result.txt");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(&output, b"accepted").unwrap();
    let plan = staged_command_plan(
        &invocation,
        "printf rejected > \"$1\"; exit 9",
        "tool/result.txt",
    );

    let error = execute_build_plan(&plan, &invocation).unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::ExternalCommand(details)
            if matches!(details.failure, ExternalCommandFailure::Exit { status, .. }
                if status.code() == Some(9))
    ));
    assert_eq!(fs::read(&output).unwrap(), b"accepted");
    assert_no_staged_command_directories(output.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[test]
fn missing_external_output_is_typed_and_retires_staging() {
    let invocation = test_invocation();
    let output = invocation.build_dir.join("tool/result.txt");
    let plan = staged_command_plan(&invocation, "true", "tool/result.txt");

    let error = execute_build_plan(&plan, &invocation).unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::StagedOutput {
            action,
            operation: "validate and sync staged",
            ..
        } if action.name() == "tool"
    ));
    assert!(!output.exists());
    assert_no_staged_command_directories(output.parent().unwrap());
    assert_no_output_transaction_journals(&invocation);
}

#[cfg(unix)]
#[test]
fn non_utf8_staged_argument_failure_retires_staging() {
    use std::os::unix::ffi::OsStringExt as _;

    let mut invocation = test_invocation();
    invocation.build_dir = invocation
        .package_root
        .join(std::ffi::OsString::from_vec(b"build-\xff".to_vec()));
    let output = invocation.build_dir.join("tool/result.txt");
    let plan = staged_command_plan(&invocation, "printf new > \"$1\"", "tool/result.txt");

    let error = execute_build_plan(&plan, &invocation).unwrap_err();

    assert!(matches!(error, CoordinatorError::NonUtf8Path { .. }));
    assert!(!output.exists());
    assert_no_staged_command_directories(output.parent().unwrap());
}

fn execute_test_command(
    action: &PlanAction,
    arguments: &[String],
    working_directory: &Path,
    timeout: Duration,
) -> Result<(), CoordinatorError> {
    execute_external_command(
        action,
        ResolvedExternalCommand {
            program: "sh",
            arguments,
            working_directory,
            environment_policy: crate::CommandEnvironmentPolicy::Inherit,
            environment: &[],
        },
        ExternalExecutionPolicy {
            timeout,
            forward_output: false,
            cancellation: None,
        },
    )
}

#[test]
fn external_command_failure_retains_status_and_bounded_output_tails() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let action = external_action();
    let arguments = vec![
        "-c".to_string(),
        "printf 'command stdout'; printf 'command stderr' >&2; exit 7".to_string(),
    ];

    let error = execute_test_command(
        &action,
        &arguments,
        &invocation.package_root,
        Duration::from_secs(5),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::ExternalCommand(details)
            if details.action.name() == "run"
                && details.arguments == arguments
                && matches!(details.failure, ExternalCommandFailure::Exit {
                    status,
                    ref stdout,
                    ref stderr,
                } if status.code() == Some(7)
                    && stdout == b"command stdout"
                    && stderr == b"command stderr")
    ));
}

#[test]
fn external_command_clear_environment_keeps_only_declared_values() {
    assert!(std::env::var_os("HOME").is_some());
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let action = external_action();
    let arguments = vec![
        "-c".to_string(),
        "test -z \"${HOME+x}\" && test \"$MODE\" = explicit".to_string(),
    ];
    let environment = [crate::EnvironmentInput {
        name: "MODE".to_string(),
        value: Some("explicit".to_string()),
    }];

    execute_external_command(
        &action,
        ResolvedExternalCommand {
            program: "sh",
            arguments: &arguments,
            working_directory: &invocation.package_root,
            environment_policy: crate::CommandEnvironmentPolicy::Clear,
            environment: &environment,
        },
        ExternalExecutionPolicy {
            timeout: Duration::from_secs(5),
            forward_output: false,
            cancellation: None,
        },
    )
    .unwrap();
}

#[test]
fn cacheable_program_search_honors_explicit_path_removal() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let error = resolve_search_program(
        &external_action(),
        "sh",
        &invocation.package_root,
        &[EnvironmentInput {
            name: "PATH".to_string(),
            value: None,
        }],
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::ExternalCommandIo {
            operation: "resolve",
            ..
        }
    ));
}

#[test]
fn external_output_tail_discards_only_the_oldest_bytes() {
    let mut tail = vec![b'a'; EXTERNAL_OUTPUT_TAIL_BYTES - 2];
    crate::process_output::append_output_tail(&mut tail, b"bcdef", EXTERNAL_OUTPUT_TAIL_BYTES);

    assert_eq!(tail.len(), EXTERNAL_OUTPUT_TAIL_BYTES);
    assert_eq!(&tail[..3], b"aaa");
    assert_eq!(&tail[tail.len() - 5..], b"bcdef");
}

#[test]
fn external_command_timeout_terminates_owned_process_group() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let action = external_action();
    let started = Instant::now();

    let error = execute_test_command(
        &action,
        &["-c".to_string(), "sleep 30".to_string()],
        &invocation.package_root,
        Duration::from_millis(50),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CoordinatorError::ExternalCommand(details)
            if matches!(details.failure, ExternalCommandFailure::TimedOut { .. })
    ));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn external_command_cancellation_terminates_owned_process_group() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let marker = invocation.package_root.join("command-started");
    let earliest_failure = Arc::new(AtomicUsize::new(usize::MAX));
    let cancellation = ActionCancellation {
        earliest_failure: Arc::clone(&earliest_failure),
        position: 1,
    };
    let working_directory = invocation.package_root.clone();
    let action = external_action();
    let worker = std::thread::spawn(move || {
        let arguments = vec![
            "-c".to_string(),
            "touch \"$1\"; sleep 30".to_string(),
            "nia-build-test".to_string(),
            marker.to_string_lossy().into_owned(),
        ];
        execute_external_command(
            &action,
            ResolvedExternalCommand {
                program: "sh",
                arguments: &arguments,
                working_directory: &working_directory,
                environment_policy: crate::CommandEnvironmentPolicy::Inherit,
                environment: &[],
            },
            ExternalExecutionPolicy {
                timeout: Duration::from_secs(30),
                forward_output: false,
                cancellation: Some(&cancellation),
            },
        )
    });
    let started = Instant::now();
    let marker = invocation.package_root.join("command-started");
    while !marker.exists() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let command_started = marker.exists();
    earliest_failure.store(0, Ordering::Release);

    let error = worker.join().expect("external command worker").unwrap_err();

    assert!(command_started);
    assert!(matches!(
        error,
        CoordinatorError::ExternalCommand(details)
            if matches!(details.failure, ExternalCommandFailure::Cancelled { .. })
    ));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(unix)]
#[test]
fn external_command_success_retires_background_descendants_holding_pipes() {
    let invocation = test_invocation();
    fs::create_dir_all(&invocation.package_root).unwrap();
    let action = external_action();
    let started = Instant::now();

    execute_test_command(
        &action,
        &["-c".to_string(), "sleep 30 & exit 0".to_string()],
        &invocation.package_root,
        Duration::from_secs(5),
    )
    .unwrap();

    assert!(started.elapsed() < Duration::from_secs(5));
}
