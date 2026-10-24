// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical selected-step closure construction and resource-aware scheduling.

use super::*;
use crate::BuildStepSelection;
use crate::progress::{ProgressEventKind, ProgressOutcome, ProgressSession};
use crate::resources::ActionResourcePermit;

/// Validates targets and computes the selected closure without reading sources,
/// restoring caches, recovering transactions, or executing actions.
pub fn preview_build_plan(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<ExecutionReport, CoordinatorError> {
    validate_invocation_targets(plan, invocation)?;
    let preview = |actions: &[&PlanAction]| {
        Ok(actions
            .iter()
            .map(|_| ActionOutcome::Succeeded(None))
            .collect())
    };
    if matches!(invocation.step, BuildStepSelection::List) {
        return Ok(ExecutionReport::default());
    }
    execute_selected_closure(plan, invocation.keep_going, preview)
}

/// Validates invocation targets, recovers interrupted output transactions, and
/// executes the selected dependency closure in deterministic readiness waves.
pub fn execute_build_plan(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<ExecutionReport, CoordinatorError> {
    execute_build_plan_with_progress(plan, invocation, None, None)
}

/// Executes a selected closure while reporting scheduler lifecycle events.
pub fn execute_build_plan_with_progress(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
    progress: Option<&ProgressSession>,
    process_output_writer: Option<Arc<dyn crate::ProcessOutputWriter>>,
) -> Result<ExecutionReport, CoordinatorError> {
    validate_invocation_targets(plan, invocation)?;
    recover_interrupted_output_transactions(&invocation.cache_dir, &invocation.build_dir)
        .map_err(|error| CoordinatorError::OutputRecovery(Box::new(error)))?;
    let executor = DriverActionExecutor::new_with_output_writer_and_progress(
        plan.clone(),
        invocation.clone(),
        process_output_writer,
        progress.cloned(),
    );
    let session = QuerySession::new().map_err(CoordinatorError::Internal)?;
    let budget = Arc::new(
        ActionResourceBudget::new(action_resource_capacity(
            &session,
            invocation.max_parallel_actions,
        ))
        .map_err(CoordinatorError::Internal)?,
    );
    nia_timing::emit_counter(
        "build.action_resource_capacity",
        action_resource_capacity(&session, invocation.max_parallel_actions) as u64,
    );
    let mut wave = 0usize;
    if matches!(invocation.step, BuildStepSelection::List) {
        return Ok(ExecutionReport::default());
    }
    execute_selected_closure_with_progress(plan, invocation.keep_going, progress, |actions| {
        let current_wave = wave;
        wave += 1;
        execute_action_batch(
            &executor,
            (&session, &budget),
            invocation,
            actions,
            !invocation.keep_going,
            progress,
            current_wave,
        )
    })
}

fn execute_action_batch(
    executor: &DriverActionExecutor,
    resources: (&QuerySession, &Arc<ActionResourceBudget>),
    invocation: &BuildInvocation,
    actions: &[&PlanAction],
    cancel_after_failure: bool,
    progress: Option<&ProgressSession>,
    wave: usize,
) -> Result<Vec<ActionOutcome>, CoordinatorError> {
    if let Some(progress) = progress {
        progress.emit(ProgressEventKind::WaveStarted {
            wave,
            actions: actions.len(),
        });
    }
    let progress = progress.cloned();
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
                enabled: cancel_after_failure,
            };
            let action = (*action).clone();
            let action_key = action.key.clone();
            let resource_class = action.resource_class();
            let action_progress = progress.clone();
            (resource_class, action_key, move || {
                execute_scheduled_action(
                    &executor,
                    &action,
                    &cancellation,
                    action_progress.as_ref(),
                )
            })
        })
        .collect::<Vec<_>>();
    run_action_tasks_with_progress(
        resources.0,
        resources.1,
        invocation.max_parallel_actions,
        tasks,
        progress.as_ref(),
        wave,
    )
    .map_err(CoordinatorError::Internal)
}

#[cfg(test)]
pub(super) fn run_action_tasks<T, O>(
    session: &QuerySession,
    max_parallel_actions: Option<std::num::NonZeroUsize>,
    tasks: impl IntoIterator<Item = (ActionResourceClass, T)>,
) -> nia_ice::IceResult<Vec<O>>
where
    T: FnOnce() -> O + Send + 'static,
    O: Send + 'static,
{
    let capacity = action_resource_capacity(session, max_parallel_actions);
    let budget = Arc::new(ActionResourceBudget::new(capacity)?);
    let tasks = tasks
        .into_iter()
        .map(|(resource_class, task)| {
            let budget = Arc::clone(&budget);
            move || -> nia_ice::IceResult<O> {
                let _permit = budget.acquire(resource_class)?;
                nia_timing::emit_counter(resource_class_counter(resource_class), 1);
                Ok(task())
            }
        })
        .collect::<Vec<_>>();
    let outcomes = match max_parallel_actions {
        Some(limit) => session.run_tasks_bounded(tasks, limit.get()),
        None => session.run_tasks(tasks),
    }?;
    Ok(outcomes)
}

pub(super) fn run_action_tasks_with_progress<T, O>(
    session: &QuerySession,
    budget: &Arc<ActionResourceBudget>,
    max_parallel_actions: Option<std::num::NonZeroUsize>,
    tasks: impl IntoIterator<Item = (ActionResourceClass, ActionKey, T)>,
    progress: Option<&ProgressSession>,
    wave: usize,
) -> nia_ice::IceResult<Vec<O>>
where
    T: FnOnce() -> O + Send + 'static,
    O: Send + 'static,
{
    let progress = progress.cloned();
    let tasks = tasks
        .into_iter()
        .map(|(resource_class, action, task)| {
            let budget = Arc::clone(budget);
            let progress = progress.clone();
            let queued_at = Instant::now();
            if let Some(progress) = &progress {
                progress.emit(ProgressEventKind::ActionQueued {
                    action: action.clone(),
                    resource_class,
                    wave,
                });
            }
            move || -> nia_ice::IceResult<O> {
                let worker_started = Instant::now();
                let mut wait = None;
                let permit = budget.acquire_observed(resource_class, || {
                    wait = progress.as_ref().map(|progress| {
                        progress.wait(action.clone(), crate::ProgressWaitKind::ResourcePermit)
                    });
                })?;
                let resource_wait = permit.wait_duration();
                if let Some(wait) = wait {
                    wait.finish(crate::ProgressWaitOutcome::Acquired);
                }
                let release = ResourceReleaseObserver {
                    permit: Some(permit),
                    budget: Arc::clone(&budget),
                    progress: progress.clone(),
                };
                if let Some(progress) = &progress {
                    if let Ok(snapshot) = budget.snapshot() {
                        progress.emit(ProgressEventKind::ResourceUsageChanged {
                            generation: snapshot.generation,
                            capacity: snapshot.capacity,
                            used: snapshot.used,
                        });
                    }
                    progress.emit(ProgressEventKind::ActionStarted {
                        action,
                        resource_class,
                        wave,
                        scheduler_wait: worker_started.duration_since(queued_at),
                        resource_wait,
                    });
                }
                nia_timing::emit_counter(resource_class_counter(resource_class), 1);
                let outcome = task();
                drop(release);
                Ok(outcome)
            }
        })
        .collect::<Vec<_>>();
    let outcomes = match max_parallel_actions {
        Some(limit) => session.run_tasks_bounded(tasks, limit.get()),
        None => session.run_tasks(tasks),
    }?;
    Ok(outcomes)
}

struct ResourceReleaseObserver<'a> {
    permit: Option<ActionResourcePermit<'a>>,
    budget: Arc<ActionResourceBudget>,
    progress: Option<ProgressSession>,
}

impl Drop for ResourceReleaseObserver<'_> {
    fn drop(&mut self) {
        drop(self.permit.take());
        if let Some(progress) = &self.progress
            && let Ok(snapshot) = self.budget.snapshot()
        {
            progress.emit(ProgressEventKind::ResourceUsageChanged {
                generation: snapshot.generation,
                capacity: snapshot.capacity,
                used: snapshot.used,
            });
        }
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
    pub(super) enabled: bool,
}

impl ActionCancellation {
    pub(super) fn is_cancelled(&self) -> bool {
        self.enabled && self.earliest_failure.load(Ordering::Acquire) < self.position
    }

    pub(super) fn cancel_later_actions(&self) {
        if self.enabled {
            self.earliest_failure
                .fetch_min(self.position, Ordering::AcqRel);
        }
    }
}

fn execute_scheduled_action(
    executor: &DriverActionExecutor,
    action: &PlanAction,
    cancellation: &ActionCancellation,
    progress: Option<&ProgressSession>,
) -> ActionOutcome {
    let started = Instant::now();
    if cancellation.is_cancelled() {
        let outcome = ActionOutcome::Cancelled;
        emit_finished(
            progress,
            action,
            outcome_kind(&outcome, action),
            None,
            started.elapsed(),
        );
        return outcome;
    }
    let outcome = match executor.execute(action, cancellation) {
        Ok(cache) => ActionOutcome::Succeeded(cache),
        Err(error) if is_cancellation_error(&error) => ActionOutcome::Cancelled,
        Err(error) => {
            cancellation.cancel_later_actions();
            ActionOutcome::Failed(error)
        }
    };
    emit_finished(
        progress,
        action,
        outcome_kind(&outcome, action),
        match &outcome {
            ActionOutcome::Succeeded(cache) => cache.as_ref(),
            _ => None,
        },
        started.elapsed(),
    );
    outcome
}

fn outcome_kind(outcome: &ActionOutcome, action: &PlanAction) -> ProgressOutcome {
    match outcome {
        ActionOutcome::Cancelled => ProgressOutcome::Cancelled,
        ActionOutcome::Failed(_) => ProgressOutcome::Failed,
        ActionOutcome::Succeeded(Some(ActionCacheOutcome::Hit)) => ProgressOutcome::Cached,
        ActionOutcome::Succeeded(Some(ActionCacheOutcome::Miss(_))) => ProgressOutcome::Executed,
        ActionOutcome::Succeeded(None) if matches!(action.kind, ActionKind::Aggregate) => {
            ProgressOutcome::NoWork
        }
        ActionOutcome::Succeeded(None) => ProgressOutcome::Executed,
    }
}

fn emit_finished(
    progress: Option<&ProgressSession>,
    action: &PlanAction,
    outcome: ProgressOutcome,
    cache: Option<&ActionCacheOutcome>,
    duration: Duration,
) {
    if let Some(progress) = progress {
        progress.emit(ProgressEventKind::ActionFinished {
            action: action.key.clone(),
            outcome,
            cache: cache.cloned(),
            duration,
        });
    }
}

pub(super) fn execute_selected_closure(
    plan: &BuildPlan,
    keep_going: bool,
    execute_batch: impl FnMut(&[&PlanAction]) -> Result<Vec<ActionOutcome>, CoordinatorError>,
) -> Result<ExecutionReport, CoordinatorError> {
    execute_selected_closure_with_progress(plan, keep_going, None, execute_batch)
}

// Without `keep_going`, the wave holding the first failure is the last one.
// With it, a failed step's dependents never become ready and every
// independent action still runs; the failures are reported together.
fn execute_selected_closure_with_progress(
    plan: &BuildPlan,
    keep_going: bool,
    progress: Option<&ProgressSession>,
    mut execute_batch: impl FnMut(&[&PlanAction]) -> Result<Vec<ActionOutcome>, CoordinatorError>,
) -> Result<ExecutionReport, CoordinatorError> {
    let Some(selected) = plan.selected_step().or_else(|| plan.default_step()) else {
        return Ok(ExecutionReport::default());
    };
    let roots = [selected];
    let steps = plan.steps();
    // Discover only the selected step's transitive dependency closure. An
    // iterative walk avoids coupling valid plan depth to the process stack.
    let mut closure = BTreeSet::new();
    let mut pending = Vec::with_capacity(roots.len());
    for root in roots {
        pending
            .push(find_step(steps, root).ok_or_else(|| {
                inconsistent("plan selection", format!("step `{}`", root.name()))
            })?);
    }
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
    let initial_ready = ready.clone();
    let mut dependency_waits = BTreeMap::<ActionKey, Instant>::new();
    if let Some(progress) = progress {
        let mut initial_actions = BTreeSet::new();
        for index in &initial_ready {
            initial_actions.insert(steps[*index].action.clone());
        }
        for &index in &closure {
            let action = steps[index].action.clone();
            if !initial_actions.contains(&action)
                && dependency_waits
                    .insert(action.clone(), Instant::now())
                    .is_none()
            {
                progress.emit(ProgressEventKind::ActionWaitStarted {
                    action,
                    kind: crate::progress::ProgressWaitKind::Dependency,
                });
            }
        }
    }
    let mut executed_actions = BTreeSet::new();
    let mut report = ExecutionReport {
        steps: Vec::with_capacity(closure.len()),
        actions: Vec::new(),
        action_cache: Vec::new(),
    };
    let mut failures = Vec::new();
    let mut failed_actions = BTreeSet::new();

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
        if let Some(progress) = progress {
            for action in &wave_actions {
                finish_wait(
                    progress,
                    &mut dependency_waits,
                    &action.key,
                    crate::progress::ProgressWaitKind::Dependency,
                    crate::progress::ProgressWaitOutcome::Acquired,
                );
            }
        }
        let outcomes = match execute_batch(&wave_actions) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                finish_waits(
                    progress,
                    &mut dependency_waits,
                    crate::progress::ProgressWaitKind::Dependency,
                );
                return Err(error);
            }
        };
        if outcomes.len() != wave_actions.len() {
            finish_waits(
                progress,
                &mut dependency_waits,
                crate::progress::ProgressWaitKind::Dependency,
            );
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
                ActionOutcome::Failed(error) => {
                    failed_actions.insert(action.key.clone());
                    failures.push(ActionFailure {
                        action: action.key.clone(),
                        test: action.kind.is_test(),
                        error: Box::new(error),
                    });
                }
            }
        }
        if cancelled || (!keep_going && !failures.is_empty()) {
            finish_waits(
                progress,
                &mut dependency_waits,
                crate::progress::ProgressWaitKind::Dependency,
            );
            if failures.is_empty() {
                return Err(inconsistent(
                    "coordinator action batch",
                    "failure cause for cancelled actions".to_string(),
                ));
            }
            return Err(CoordinatorError::from_failures(failures));
        }
        report.actions.extend(
            wave_actions
                .iter()
                .filter(|action| !failed_actions.contains(&action.key))
                .map(|action| action.key.clone()),
        );
        for index in wave {
            let step = &steps[index];
            if failed_actions.contains(&step.action) {
                continue;
            }
            report.steps.push(step.key.clone());
            for &dependent in &dependents[index] {
                indegree[dependent] = indegree[dependent].checked_sub(1).ok_or_else(|| {
                    inconsistent(
                        format!("step `{}`", steps[dependent].key.name()),
                        "valid dependency degree".to_string(),
                    )
                })?;
                if indegree[dependent] == 0 {
                    let action = steps[dependent].action.clone();
                    if !executed_actions.contains(&action)
                        && let Some(progress) = progress
                        && let Some(started) = dependency_waits.remove(&action)
                    {
                        progress.emit(ProgressEventKind::ActionWaitFinished {
                            action: action.clone(),
                            kind: crate::progress::ProgressWaitKind::Dependency,
                            outcome: crate::progress::ProgressWaitOutcome::Acquired,
                            duration: started.elapsed(),
                        });
                    }
                    ready.insert(dependent);
                }
            }
        }
    }

    if !failures.is_empty() {
        finish_waits(
            progress,
            &mut dependency_waits,
            crate::progress::ProgressWaitKind::Dependency,
        );
        return Err(CoordinatorError::from_failures(failures));
    }
    if report.steps.len() != closure.len() {
        finish_waits(
            progress,
            &mut dependency_waits,
            crate::progress::ProgressWaitKind::Dependency,
        );
        return Err(inconsistent(
            "selected step closure",
            "acyclic dependency order".to_string(),
        ));
    }
    Ok(report)
}

fn finish_wait(
    progress: &ProgressSession,
    waits: &mut BTreeMap<ActionKey, Instant>,
    action: &ActionKey,
    kind: crate::progress::ProgressWaitKind,
    outcome: crate::progress::ProgressWaitOutcome,
) {
    if let Some(started) = waits.remove(action) {
        progress.emit(ProgressEventKind::ActionWaitFinished {
            action: action.clone(),
            kind,
            outcome,
            duration: started.elapsed(),
        });
    }
}

fn finish_waits(
    progress: Option<&ProgressSession>,
    waits: &mut BTreeMap<ActionKey, Instant>,
    kind: crate::progress::ProgressWaitKind,
) {
    let Some(progress) = progress else {
        waits.clear();
        return;
    };
    for (action, started) in std::mem::take(waits) {
        progress.emit(ProgressEventKind::ActionWaitFinished {
            action,
            kind,
            outcome: crate::progress::ProgressWaitOutcome::Cancelled,
            duration: started.elapsed(),
        });
    }
}
