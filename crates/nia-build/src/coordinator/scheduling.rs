// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical selected-step closure construction and resource-aware scheduling.

use super::*;

/// Validates invocation targets, recovers interrupted output transactions, and
/// executes the selected dependency closure in deterministic readiness waves.
pub fn execute_build_plan(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<ExecutionReport, CoordinatorError> {
    validate_invocation_targets(plan, invocation)?;
    recover_interrupted_output_transactions(&invocation.cache_dir, &invocation.build_dir)
        .map_err(|error| CoordinatorError::OutputRecovery(Box::new(error)))?;
    let executor = DriverActionExecutor::new(plan.clone(), invocation.clone());
    let session = QuerySession::new();
    nia_timing::emit_counter(
        "build.action_resource_capacity",
        action_resource_capacity(&session, invocation.max_parallel_actions) as u64,
    );
    execute_selected_closure(plan, |actions| {
        // Positions follow canonical wave order. Recording the earliest failed
        // position makes cancellation deterministic even when later workers
        // observe their failures first.
        let earliest_failure = Arc::new(AtomicUsize::new(usize::MAX));
        let tasks = actions
            .iter()
            .enumerate()
            .map(|(position, action)| {
                let executor = executor.clone();
                let cancellation = ActionCancellation {
                    earliest_failure: Arc::clone(&earliest_failure),
                    position,
                };
                let action = (*action).clone();
                (action.resource_class(), move || {
                    execute_scheduled_action(&executor, &action, &cancellation)
                })
            })
            .collect::<Vec<_>>();
        run_action_tasks(&session, invocation.max_parallel_actions, tasks)
    })
}

pub(super) fn run_action_tasks<T, O>(
    session: &QuerySession,
    max_parallel_actions: Option<std::num::NonZeroUsize>,
    tasks: impl IntoIterator<Item = (ActionResourceClass, T)>,
) -> Vec<O>
where
    T: FnOnce() -> O + Send + 'static,
    O: Send + 'static,
{
    let capacity = action_resource_capacity(session, max_parallel_actions);
    let budget = Arc::new(ActionResourceBudget::new(capacity));
    let tasks = tasks
        .into_iter()
        .map(|(resource_class, task)| {
            let budget = Arc::clone(&budget);
            move || {
                let _permit = budget.acquire(resource_class);
                nia_timing::emit_counter(resource_class_counter(resource_class), 1);
                task()
            }
        })
        .collect::<Vec<_>>();
    match max_parallel_actions {
        Some(limit) => session.run_tasks_bounded(tasks, limit.get()),
        None => session.run_tasks(tasks),
    }
}

pub(super) fn action_resource_capacity(
    session: &QuerySession,
    max_parallel_actions: Option<std::num::NonZeroUsize>,
) -> usize {
    max_parallel_actions
        .map_or_else(|| session.executor_parallelism(), |limit| limit.get())
        .min(session.executor_parallelism())
        .max(1)
}

fn resource_class_counter(class: ActionResourceClass) -> &'static str {
    match class {
        ActionResourceClass::Conservative => "build.resource_class_conservative_actions",
        ActionResourceClass::Cpu => "build.resource_class_cpu_actions",
        ActionResourceClass::Io => "build.resource_class_io_actions",
    }
}

pub(super) enum ActionOutcome {
    Succeeded(Option<ActionCacheOutcome>),
    Cancelled,
    Failed(CoordinatorError),
}

pub(super) struct ActionCancellation {
    pub(super) earliest_failure: Arc<AtomicUsize>,
    pub(super) position: usize,
}

impl ActionCancellation {
    pub(super) fn is_cancelled(&self) -> bool {
        self.earliest_failure.load(Ordering::Acquire) < self.position
    }

    pub(super) fn cancel_later_actions(&self) {
        self.earliest_failure
            .fetch_min(self.position, Ordering::AcqRel);
    }
}

fn execute_scheduled_action(
    executor: &DriverActionExecutor,
    action: &PlanAction,
    cancellation: &ActionCancellation,
) -> ActionOutcome {
    if cancellation.is_cancelled() {
        return ActionOutcome::Cancelled;
    }
    match executor.execute(action, cancellation) {
        Ok(cache) => ActionOutcome::Succeeded(cache),
        Err(error) if is_cancellation_error(&error) => ActionOutcome::Cancelled,
        Err(error) => {
            cancellation.cancel_later_actions();
            ActionOutcome::Failed(error)
        }
    }
}

pub(super) fn execute_selected_closure(
    plan: &BuildPlan,
    mut execute_batch: impl FnMut(&[&PlanAction]) -> Vec<ActionOutcome>,
) -> Result<ExecutionReport, CoordinatorError> {
    let Some(selected) = plan.selected_step().or_else(|| plan.default_step()) else {
        return Ok(ExecutionReport {
            steps: Vec::new(),
            actions: Vec::new(),
            action_cache: Vec::new(),
        });
    };

    let steps = plan.steps();
    // Discover only the selected step's transitive dependency closure. An
    // iterative walk avoids coupling valid plan depth to the process stack.
    let mut closure = BTreeSet::new();
    let selected_index = find_step(steps, selected)
        .ok_or_else(|| inconsistent("plan selection", format!("step `{}`", selected.name())))?;
    let mut pending = vec![selected_index];
    while let Some(index) = pending.pop() {
        if !closure.insert(index) {
            continue;
        }
        for dependency in &steps[index].dependencies {
            pending.push(find_step(steps, dependency).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", steps[index].key.name()),
                    format!("dependency step `{}`", dependency.name()),
                )
            })?);
        }
    }

    // Build the induced dependency graph once, then consume it in canonical
    // Kahn waves. The BTreeSet makes allocation and completion order invisible.
    let mut indegree = vec![0usize; steps.len()];
    let mut dependents = vec![Vec::new(); steps.len()];
    for &index in &closure {
        for dependency in &steps[index].dependencies {
            let dependency_index = find_step(steps, dependency).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", steps[index].key.name()),
                    format!("dependency step `{}`", dependency.name()),
                )
            })?;
            if closure.contains(&dependency_index) {
                indegree[index] += 1;
                dependents[dependency_index].push(index);
            }
        }
    }

    let mut ready: BTreeSet<_> = closure
        .iter()
        .copied()
        .filter(|index| indegree[*index] == 0)
        .collect();
    let mut executed_actions = BTreeSet::new();
    let mut report = ExecutionReport {
        steps: Vec::with_capacity(closure.len()),
        actions: Vec::new(),
        action_cache: Vec::new(),
    };

    while !ready.is_empty() {
        let wave = std::mem::take(&mut ready);
        let mut wave_actions = Vec::new();
        for &index in &wave {
            let step = &steps[index];
            // Several steps may intentionally share one action; execute it at
            // most once while retaining every step in the final report.
            if executed_actions.insert(step.action.clone()) {
                let action = find_action(plan.actions(), &step.action).ok_or_else(|| {
                    inconsistent(
                        format!("step `{}`", step.key.name()),
                        format!("action `{}`", step.action.name()),
                    )
                })?;
                wave_actions.push(action);
            }
        }
        let outcomes = execute_batch(&wave_actions);
        if outcomes.len() != wave_actions.len() {
            return Err(inconsistent(
                "coordinator action batch",
                "one outcome per scheduled action".to_string(),
            ));
        }
        let mut cancelled = false;
        for (action, outcome) in wave_actions.iter().zip(outcomes) {
            match outcome {
                ActionOutcome::Succeeded(cache) => {
                    if let Some(outcome) = cache {
                        report.action_cache.push(ActionCacheReport {
                            action: action.key.clone(),
                            outcome,
                        });
                    }
                }
                ActionOutcome::Cancelled => cancelled = true,
                ActionOutcome::Failed(error) => return Err(error),
            }
        }
        if cancelled {
            return Err(inconsistent(
                "coordinator action batch",
                "failure cause for cancelled actions".to_string(),
            ));
        }
        report
            .actions
            .extend(wave_actions.iter().map(|action| action.key.clone()));
        for index in wave {
            let step = &steps[index];
            report.steps.push(step.key.clone());
            for &dependent in &dependents[index] {
                indegree[dependent] = indegree[dependent].checked_sub(1).ok_or_else(|| {
                    inconsistent(
                        format!("step `{}`", steps[dependent].key.name()),
                        "valid dependency degree".to_string(),
                    )
                })?;
                if indegree[dependent] == 0 {
                    ready.insert(dependent);
                }
            }
        }
    }

    if report.steps.len() != closure.len() {
        return Err(inconsistent(
            "selected step closure",
            "acyclic dependency order".to_string(),
        ));
    }
    Ok(report)
}
