// SPDX-License-Identifier: GPL-3.0-or-later
//! Invocation-local build progress events.
//!
//! Progress is deliberately an observation boundary. Events are not persisted,
//! do not participate in plan/cache identity, and cannot alter scheduling.

use std::{
    collections::{BTreeMap, BTreeSet},
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::{ActionCacheOutcome, ActionKey, ActionResourceClass, LogicalPath};

struct ProgressInner {
    started: std::time::Instant,
    next_sequence: AtomicU64,
    dispatch_lock: std::sync::Mutex<()>,
    queue: Arc<(std::sync::Mutex<ProgressQueue>, std::sync::Condvar)>,
    terminal: std::sync::Mutex<BTreeSet<ActionKey>>,
    cache_decisions: std::sync::Mutex<BTreeMap<ActionKey, ActionCacheOutcome>>,
    worker: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[derive(Default)]
struct ProgressQueue {
    events: BTreeMap<u64, ProgressEvent>,
    spans: BTreeMap<SpanKey, PendingSpan>,
    resource: Option<(u64, u64)>,
    closed: bool,
}

#[derive(Default)]
struct PendingSpan {
    start: Option<u64>,
    completed: Option<(u64, u64)>,
}

impl ProgressQueue {
    fn push(&mut self, event: ProgressEvent) {
        if let ProgressEventKind::ResourceUsageChanged { generation, .. } = &event.kind {
            if let Some((sequence, previous_generation)) = self.resource {
                if *generation <= previous_generation {
                    return;
                }
                self.events.remove(&sequence);
            }
            self.resource = Some((event.sequence, *generation));
        }
        // Repeated cache mutations can create arbitrarily many spans for one
        // action. Coalesce only complete, undelivered pairs. Once a start was
        // consumed its finish must survive, as must every lifecycle record.
        if let Some(key) = span_started_key(&event.kind) {
            self.spans.entry(key).or_default().start = Some(event.sequence);
        } else if let Some(key) = span_finished_key(&event.kind) {
            let pending = self.spans.entry(key).or_default();
            if let Some((start, finish)) = pending.completed.take()
                && self.events.contains_key(&start)
                && self.events.contains_key(&finish)
            {
                self.events.remove(&start);
                self.events.remove(&finish);
            }
            pending.completed = pending.start.take().map(|start| (start, event.sequence));
        }
        self.events.insert(event.sequence, event);
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SpanKey {
    Wait(ActionKey, ProgressWaitKind),
    Activity(ActionKey, ProgressActivity),
    PhaseWait(ProgressPhase),
}

fn span_started_key(kind: &ProgressEventKind) -> Option<SpanKey> {
    match kind {
        ProgressEventKind::PhaseWaitStarted { phase } => Some(SpanKey::PhaseWait(*phase)),
        ProgressEventKind::ActionWaitStarted { action, kind } => {
            Some(SpanKey::Wait(action.clone(), *kind))
        }
        ProgressEventKind::ActionActivityStarted { action, activity } => {
            Some(SpanKey::Activity(action.clone(), *activity))
        }
        _ => None,
    }
}

fn span_finished_key(kind: &ProgressEventKind) -> Option<SpanKey> {
    match kind {
        ProgressEventKind::PhaseWaitFinished { phase, .. } => Some(SpanKey::PhaseWait(*phase)),
        ProgressEventKind::ActionWaitFinished { action, kind, .. } => {
            Some(SpanKey::Wait(action.clone(), *kind))
        }
        ProgressEventKind::ActionActivityFinished {
            action, activity, ..
        } => Some(SpanKey::Activity(action.clone(), *activity)),
        _ => None,
    }
}

impl Drop for ProgressInner {
    fn drop(&mut self) {
        if let Ok(queue) = self.queue.0.lock() {
            let mut queue = queue;
            queue.closed = true;
        }
        self.queue.1.notify_all();
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

/// One build lifecycle or selected-action observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    /// Event category and associated details.
    pub kind: ProgressEventKind,
    /// Monotonic event sequence assigned by the producer when available.
    pub sequence: u64,
    /// Producer time relative to invocation start, independent of observer lag.
    pub elapsed: Duration,
}

/// Progress lifecycle categories shared by all output backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEventKind {
    /// Build invocation started.
    BuildStarted,
    /// Build completed successfully.
    BuildFinished,
    /// Build terminated with an error.
    BuildFailed,
    /// A coarse bootstrap or execution phase began.
    PhaseStarted { phase: ProgressPhase },
    /// A coarse phase ended, including failure paths.
    PhaseFinished {
        phase: ProgressPhase,
        duration: Duration,
        success: bool,
    },
    /// Authoritative bootstrap cache reuse decision.
    PhaseCacheDecision { phase: ProgressPhase, reused: bool },
    /// Bootstrap phase encountered actual cache-lock contention.
    PhaseWaitStarted { phase: ProgressPhase },
    /// Bootstrap cache-lock contention ended.
    PhaseWaitFinished {
        phase: ProgressPhase,
        outcome: ProgressWaitOutcome,
        duration: Duration,
    },
    /// Selected closure became available.
    SelectionReady {
        total: usize,
        actions: Vec<ActionKey>,
    },
    /// Immutable, bounded logical details from the selected frozen plan.
    ActionDescribed {
        action: ActionKey,
        description: ProgressActionDescription,
    },
    /// Initial authoritative cache lookup, independent of execution success.
    ActionCacheDecision {
        action: ActionKey,
        cache: ActionCacheOutcome,
    },
    /// A scheduler wave became active.
    WaveStarted { wave: usize, actions: usize },
    /// A ready action was submitted to the inherited worker pool.
    ActionQueued {
        action: ActionKey,
        resource_class: ActionResourceClass,
        wave: usize,
    },
    /// A selected action entered execution after resource admission.
    ActionStarted {
        action: ActionKey,
        resource_class: ActionResourceClass,
        wave: usize,
        scheduler_wait: Duration,
        resource_wait: Duration,
    },
    /// A selected action encountered a measured owner-level wait.
    ActionWaitStarted {
        action: ActionKey,
        kind: ProgressWaitKind,
    },
    /// A measured owner-level wait ended.
    ActionWaitFinished {
        action: ActionKey,
        kind: ProgressWaitKind,
        outcome: ProgressWaitOutcome,
        duration: Duration,
    },
    /// A coarse compiler activity began inside one build action.
    ActionActivityStarted {
        action: ActionKey,
        activity: ProgressActivity,
    },
    /// A coarse compiler activity ended inside one build action.
    ActionActivityFinished {
        action: ActionKey,
        activity: ProgressActivity,
        duration: Duration,
        success: bool,
    },
    /// A generation-stamped snapshot of shared action-slot occupancy.
    ResourceUsageChanged {
        generation: u64,
        capacity: usize,
        used: usize,
    },
    /// A selected action reached a terminal state.
    ActionFinished {
        action: ActionKey,
        outcome: ProgressOutcome,
        /// Final cache outcome on success; observed lookup on failure/cancellation.
        cache: Option<ActionCacheOutcome>,
        duration: Duration,
    },
}

/// Declared action details, excluding process arguments and payload contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressActionDescription {
    /// Canonical action kind, derived from the typed plan operation.
    pub kind: &'static str,
    /// First logical inputs, capped at eight paths.
    pub inputs: Vec<LogicalPath>,
    /// Total declared inputs before the display cap.
    pub total_inputs: usize,
    /// First logical outputs, capped at eight paths.
    pub outputs: Vec<LogicalPath>,
    /// Total declared outputs before the display cap.
    pub total_outputs: usize,
}

/// Public build phases that are meaningful without exposing compiler queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProgressPhase {
    Resolve,
    Prepare,
    Runner,
    Configure,
    Preview,
    Execute,
}

/// Terminal action outcome exposed to progress consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressOutcome {
    /// Restored successfully from an action cache.
    Cached,
    /// Executed successfully, including an uncacheable action.
    Executed,
    /// Dependency-only action with no direct work.
    NoWork,
    /// Action failed.
    Failed,
    /// Action was cancelled before successful completion.
    Cancelled,
}

/// Owner-level waits that are safe to expose without leaking physical paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProgressWaitKind {
    Dependency,
    ResourcePermit,
    OutputLock,
    CacheLock,
}

/// Result of a measured owner-level wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressWaitOutcome {
    Acquired,
    Cancelled,
    Failed,
}

/// Coarse compiler stages nested under one logical build action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProgressActivity {
    Checking,
    Codegen,
    Linking,
    /// Driver operation that compiles and links as one call.
    CompilingAndLinking,
}

/// Consumer of invocation-local progress events.
///
/// Delivery is ordered by sequence, but sequence gaps are allowed: pending
/// resource snapshots and repeated, fully undelivered wait/activity spans may
/// be coalesced when the consumer falls behind. Lifecycle and terminal events
/// are retained, and a delivered span start always has a matching finish.
pub trait ProgressObserver: Send + Sync {
    /// Receives one event. Implementations should be fast and non-panicking.
    fn observe(&self, event: ProgressEvent);
}

/// Invocation-local event sequencer. Observer failures are isolated from the
/// build so progress remains an optional diagnostic side channel.
#[derive(Clone)]
pub(crate) struct ProgressSession {
    inner: Arc<ProgressInner>,
}

impl ProgressSession {
    fn close_and_join(&self) {
        if let Ok(mut queue) = self.inner.queue.0.lock() {
            queue.closed = true;
        }
        self.inner.queue.1.notify_all();
        let worker = self
            .inner
            .worker
            .lock()
            .ok()
            .and_then(|mut worker| worker.take());
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    pub(crate) fn phase_lock_observer(
        &self,
        phase: ProgressPhase,
    ) -> crate::lock::LockWaitObserver {
        let session = self.clone();
        let started = std::sync::Mutex::new(None::<std::time::Instant>);
        crate::lock::LockWaitObserver::new(Arc::new(move |event| {
            use crate::lock::LockWaitEvent;
            let Ok(mut started) = started.lock() else {
                return;
            };
            match event {
                LockWaitEvent::Contended => {
                    if started.is_none() {
                        *started = Some(std::time::Instant::now());
                        session.emit(ProgressEventKind::PhaseWaitStarted { phase });
                    }
                }
                event => {
                    if let Some(started) = started.take() {
                        session.emit(ProgressEventKind::PhaseWaitFinished {
                            phase,
                            outcome: match event {
                                LockWaitEvent::Acquired => ProgressWaitOutcome::Acquired,
                                LockWaitEvent::Cancelled => ProgressWaitOutcome::Cancelled,
                                _ => ProgressWaitOutcome::Failed,
                            },
                            duration: started.elapsed(),
                        });
                    }
                }
            }
        }))
    }

    pub(crate) fn new(observer: Arc<dyn ProgressObserver>) -> Self {
        let worker_observer = Arc::clone(&observer);
        let queue = std::sync::Arc::new((
            std::sync::Mutex::new(ProgressQueue::default()),
            std::sync::Condvar::new(),
        ));
        let worker_queue = std::sync::Arc::clone(&queue);
        let worker = std::thread::Builder::new()
            .name("nia-build-progress".into())
            .spawn(move || {
                loop {
                    let event = {
                        let (lock, wake) = &*worker_queue;
                        let Ok(mut queue) = lock.lock() else {
                            return;
                        };
                        while queue.events.is_empty() && !queue.closed {
                            let Ok(next) = wake.wait(queue) else {
                                return;
                            };
                            queue = next;
                        }
                        queue.events.pop_first().map(|(_, event)| event)
                    };
                    let Some(event) = event else {
                        break;
                    };
                    let _ =
                        panic::catch_unwind(AssertUnwindSafe(|| worker_observer.observe(event)));
                }
            });
        let worker = worker.ok();
        if worker.is_none()
            && let Ok(mut queue) = queue.0.lock()
        {
            queue.closed = true;
        }
        Self {
            inner: Arc::new(ProgressInner {
                started: std::time::Instant::now(),
                next_sequence: AtomicU64::new(0),
                dispatch_lock: std::sync::Mutex::new(()),
                queue,
                terminal: std::sync::Mutex::new(BTreeSet::new()),
                cache_decisions: std::sync::Mutex::new(BTreeMap::new()),
                worker: std::sync::Mutex::new(worker),
            }),
        }
    }

    pub(crate) fn emit(&self, mut kind: ProgressEventKind) {
        let _dispatch = self.inner.dispatch_lock.lock().ok();
        if let ProgressEventKind::ActionCacheDecision { action, cache } = &kind {
            let Ok(mut decisions) = self.inner.cache_decisions.lock() else {
                return;
            };
            // A suite's fallback execution must not replace its own lookup.
            if decisions.contains_key(action) {
                return;
            }
            decisions.insert(action.clone(), cache.clone());
        }
        if let ProgressEventKind::ActionFinished { action, cache, .. } = &mut kind
            && cache.is_none()
            && let Ok(decisions) = self.inner.cache_decisions.lock()
        {
            *cache = decisions.get(action).cloned();
        }
        if let ProgressEventKind::ActionFinished { action, .. } = &kind {
            let Ok(mut terminal) = self.inner.terminal.lock() else {
                return;
            };
            if !terminal.insert(action.clone()) {
                return;
            }
        }
        let event = ProgressEvent {
            kind,
            sequence: self.inner.next_sequence.fetch_add(1, Ordering::Relaxed),
            elapsed: self.inner.started.elapsed(),
        };
        if let Ok(mut queue) = self.inner.queue.0.lock()
            && !queue.closed
        {
            queue.push(event);
            self.inner.queue.1.notify_one();
        }
    }

    pub(crate) fn cancel_unfinished(&self, actions: &[ActionKey]) {
        for action in actions {
            self.emit(ProgressEventKind::ActionFinished {
                action: action.clone(),
                outcome: ProgressOutcome::Cancelled,
                cache: None,
                duration: Duration::ZERO,
            });
        }
    }

    pub(crate) fn phase<T, E>(
        &self,
        phase: ProgressPhase,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        self.emit(ProgressEventKind::PhaseStarted { phase });
        let started = std::time::Instant::now();
        let result = panic::catch_unwind(AssertUnwindSafe(operation));
        let result = match result {
            Ok(result) => result,
            Err(payload) => {
                self.emit(ProgressEventKind::PhaseFinished {
                    phase,
                    duration: started.elapsed(),
                    success: false,
                });
                panic::resume_unwind(payload);
            }
        };
        self.emit(ProgressEventKind::PhaseFinished {
            phase,
            duration: started.elapsed(),
            success: result.is_ok(),
        });
        result
    }

    pub(crate) fn wait(&self, action: ActionKey, kind: ProgressWaitKind) -> ProgressWaitGuard {
        self.emit(ProgressEventKind::ActionWaitStarted {
            action: action.clone(),
            kind,
        });
        ProgressWaitGuard {
            session: self.clone(),
            action,
            kind,
            started: std::time::Instant::now(),
            finished: false,
        }
    }

    pub(crate) fn activity(
        &self,
        action: ActionKey,
        activity: ProgressActivity,
    ) -> ProgressActivityGuard {
        self.emit(ProgressEventKind::ActionActivityStarted {
            action: action.clone(),
            activity,
        });
        ProgressActivityGuard {
            session: self.clone(),
            action,
            activity,
            started: std::time::Instant::now(),
            finished: false,
        }
    }
}

/// RAII completion for a measured owner-level wait.
pub(crate) struct ProgressWaitGuard {
    session: ProgressSession,
    action: ActionKey,
    kind: ProgressWaitKind,
    started: std::time::Instant,
    finished: bool,
}

impl ProgressWaitGuard {
    pub(crate) fn finish(mut self, outcome: ProgressWaitOutcome) {
        self.finished = true;
        self.session.emit(ProgressEventKind::ActionWaitFinished {
            action: self.action.clone(),
            kind: self.kind,
            outcome,
            duration: self.started.elapsed(),
        });
    }
}

impl Drop for ProgressWaitGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.session.emit(ProgressEventKind::ActionWaitFinished {
                action: self.action.clone(),
                kind: self.kind,
                outcome: ProgressWaitOutcome::Failed,
                duration: self.started.elapsed(),
            });
        }
    }
}

/// RAII completion for a coarse compiler activity.
pub(crate) struct ProgressActivityGuard {
    session: ProgressSession,
    action: ActionKey,
    activity: ProgressActivity,
    started: std::time::Instant,
    finished: bool,
}

impl ProgressActivityGuard {
    pub(crate) fn finish(mut self, success: bool) {
        self.finished = true;
        self.session
            .emit(ProgressEventKind::ActionActivityFinished {
                action: self.action.clone(),
                activity: self.activity,
                duration: self.started.elapsed(),
                success,
            });
    }
}

impl Drop for ProgressActivityGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.session
                .emit(ProgressEventKind::ActionActivityFinished {
                    action: self.action.clone(),
                    activity: self.activity,
                    duration: self.started.elapsed(),
                    success: false,
                });
        }
    }
}

pub(crate) struct ProgressBuildGuard {
    session: Option<ProgressSession>,
    selected_actions: Vec<ActionKey>,
    completed: bool,
}

impl ProgressBuildGuard {
    pub(crate) fn new(session: Option<ProgressSession>) -> Self {
        if let Some(session) = &session {
            session.emit(ProgressEventKind::BuildStarted);
        }
        Self {
            session,
            selected_actions: Vec::new(),
            completed: false,
        }
    }

    pub(crate) fn session(&self) -> Option<&ProgressSession> {
        self.session.as_ref()
    }

    pub(crate) fn select(&mut self, actions: Vec<ActionKey>) {
        self.selected_actions = actions;
    }

    pub(crate) fn finish(&mut self, success: bool) {
        if self.completed {
            return;
        }
        if let Some(session) = &self.session {
            if !success {
                session.cancel_unfinished(&self.selected_actions);
            }
            session.emit(if success {
                ProgressEventKind::BuildFinished
            } else {
                ProgressEventKind::BuildFailed
            });
            session.close_and_join();
        }
        self.completed = true;
    }
}

impl Drop for ProgressBuildGuard {
    fn drop(&mut self) {
        self.finish(false);
    }
}

impl<F> ProgressObserver for F
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    fn observe(&self, event: ProgressEvent) {
        self(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn session_assigns_monotonic_sequences_and_survives_observer_panics() {
        let (sender, receiver) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            let _ = sender.send(event);
        }));
        session.emit(ProgressEventKind::BuildStarted);
        session.emit(ProgressEventKind::SelectionReady {
            total: 0,
            actions: Vec::new(),
        });
        let result: Result<(), ()> = session.phase(ProgressPhase::Execute, || Ok(()));
        assert!(result.is_ok());
        drop(session);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert!(matches!(
            events[2].kind,
            ProgressEventKind::PhaseStarted {
                phase: ProgressPhase::Execute
            }
        ));
    }

    #[test]
    fn session_contains_observer_panic() {
        let session = ProgressSession::new(Arc::new(|_: ProgressEvent| panic!("observer")));
        session.emit(ProgressEventKind::BuildStarted);
        session.emit(ProgressEventKind::BuildFinished);
    }

    #[test]
    fn cancelled_actions_are_emitted_once_for_unfinished_selection() {
        let (sender, receiver) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            let _ = sender.send(event);
        }));
        let first = ActionKey::new(crate::PackageKey::root(), "first").unwrap();
        let second = ActionKey::new(crate::PackageKey::root(), "second").unwrap();
        session.emit(ProgressEventKind::ActionFinished {
            action: first.clone(),
            outcome: ProgressOutcome::Executed,
            cache: None,
            duration: Duration::ZERO,
        });
        session.cancel_unfinished(&[first, second.clone()]);
        drop(session);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match &event.kind {
                    ProgressEventKind::ActionFinished {
                        action, outcome, ..
                    } if *action == second && *outcome == ProgressOutcome::Cancelled => {
                        Some(())
                    }
                    _ => None,
                })
                .count(),
            1
        );
    }

    #[test]
    fn cancelled_action_retains_its_cache_lookup() {
        let (sender, receiver) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            let _ = sender.send(event);
        }));
        let action = ActionKey::new(crate::PackageKey::root(), "cancelled").unwrap();
        let cache = ActionCacheOutcome::Miss(crate::ActionCacheMissReason::Corrupt);
        session.emit(ProgressEventKind::ActionCacheDecision {
            action: action.clone(),
            cache: cache.clone(),
        });
        session.cancel_unfinished(&[action]);
        drop(session);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(matches!(&events[1].kind,
            ProgressEventKind::ActionFinished {
                outcome: ProgressOutcome::Cancelled, cache: Some(observed), ..
            } if *observed == cache));
    }

    #[test]
    fn cache_lookup_is_bounded_and_does_not_override_final_publication_result() {
        let (sender, receiver) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            let _ = sender.send(event);
        }));
        let action = ActionKey::new(crate::PackageKey::root(), "cache").unwrap();
        let lookup = ActionCacheOutcome::Miss(crate::ActionCacheMissReason::NotFound);
        for cache in std::iter::once(lookup.clone()).chain(std::iter::repeat_n(
            ActionCacheOutcome::Miss(crate::ActionCacheMissReason::Uncacheable),
            10_000,
        )) {
            session.emit(ProgressEventKind::ActionCacheDecision {
                action: action.clone(),
                cache,
            });
        }
        let publication = ActionCacheOutcome::Miss(crate::ActionCacheMissReason::WriteError);
        session.emit(ProgressEventKind::ActionFinished {
            action,
            outcome: ProgressOutcome::Executed,
            cache: Some(publication.clone()),
            duration: Duration::ZERO,
        });
        drop(session);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0].kind,
            ProgressEventKind::ActionCacheDecision { cache, .. } if *cache == lookup));
        assert!(matches!(&events[1].kind,
            ProgressEventKind::ActionFinished { cache: Some(cache), .. } if *cache == publication));
    }

    #[test]
    fn duplicate_action_terminal_events_are_suppressed() {
        let (sender, receiver) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            let _ = sender.send(event);
        }));
        let action = ActionKey::new(crate::PackageKey::root(), "once").unwrap();
        for outcome in [ProgressOutcome::Failed, ProgressOutcome::Cancelled] {
            session.emit(ProgressEventKind::ActionFinished {
                action: action.clone(),
                outcome,
                cache: None,
                duration: Duration::ZERO,
            });
        }
        drop(session);
        let terminal_events = receiver
            .try_iter()
            .filter(|event| matches!(event.kind, ProgressEventKind::ActionFinished { .. }))
            .count();
        assert_eq!(terminal_events, 1);
    }

    #[test]
    fn measured_waits_and_activities_close_on_finish_and_drop() {
        let (sender, receiver) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            let _ = sender.send(event);
        }));
        let action = ActionKey::new(crate::PackageKey::root(), "observed").unwrap();
        session
            .wait(action.clone(), ProgressWaitKind::OutputLock)
            .finish(ProgressWaitOutcome::Acquired);
        let activity = session.activity(action, ProgressActivity::Codegen);
        drop(activity);
        drop(session);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ProgressEventKind::ActionWaitFinished {
                outcome: ProgressWaitOutcome::Acquired,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ProgressEventKind::ActionActivityFinished { success: false, .. }
        )));
    }

    #[test]
    fn bounded_queue_keeps_terminal_records_under_pressure() {
        let action = ActionKey::new(crate::PackageKey::root(), "terminal").unwrap();
        let mut queue = ProgressQueue::default();
        queue.push(ProgressEvent {
            kind: ProgressEventKind::BuildStarted,
            sequence: 0,
            elapsed: Duration::ZERO,
        });
        for sequence in 1..=10_000 {
            queue.push(ProgressEvent {
                kind: ProgressEventKind::ActionWaitStarted {
                    action: action.clone(),
                    kind: ProgressWaitKind::CacheLock,
                },
                sequence: sequence * 2,
                elapsed: Duration::ZERO,
            });
            queue.push(ProgressEvent {
                kind: ProgressEventKind::ActionWaitFinished {
                    action: action.clone(),
                    kind: ProgressWaitKind::CacheLock,
                    outcome: ProgressWaitOutcome::Acquired,
                    duration: Duration::from_millis(1),
                },
                sequence: sequence * 2 + 1,
                elapsed: Duration::ZERO,
            });
            assert_eq!(queue.events.len(), 3);
        }
        queue.push(ProgressEvent {
            kind: ProgressEventKind::ActionFinished {
                action,
                outcome: ProgressOutcome::Executed,
                cache: None,
                duration: Duration::ZERO,
            },
            sequence: 20_002,
            elapsed: Duration::ZERO,
        });
        queue.push(ProgressEvent {
            kind: ProgressEventKind::BuildFinished,
            sequence: 20_003,
            elapsed: Duration::ZERO,
        });
        assert_eq!(queue.events.len(), 5);
        assert!(matches!(
            queue.events.pop_first().unwrap().1.kind,
            ProgressEventKind::BuildStarted
        ));
        assert!(matches!(
            queue.events.pop_last().unwrap().1.kind,
            ProgressEventKind::BuildFinished
        ));
        assert!(matches!(
            queue.events.pop_last().unwrap().1.kind,
            ProgressEventKind::ActionFinished { .. }
        ));
    }

    #[test]
    fn consumed_span_start_keeps_its_finish_during_coalescing() {
        let action = ActionKey::new(crate::PackageKey::root(), "waiting").unwrap();
        let mut queue = ProgressQueue::default();
        for sequence in 0..6 {
            let kind = if sequence % 2 == 0 {
                ProgressEventKind::ActionWaitStarted {
                    action: action.clone(),
                    kind: ProgressWaitKind::CacheLock,
                }
            } else {
                ProgressEventKind::ActionWaitFinished {
                    action: action.clone(),
                    kind: ProgressWaitKind::CacheLock,
                    outcome: ProgressWaitOutcome::Acquired,
                    duration: Duration::ZERO,
                }
            };
            queue.push(ProgressEvent {
                kind,
                sequence,
                elapsed: Duration::ZERO,
            });
            if sequence == 0 {
                queue.events.pop_first();
            }
        }
        assert_eq!(queue.events.keys().copied().collect::<Vec<_>>(), [1, 4, 5]);
    }

    #[test]
    fn resource_coalescing_keeps_delivery_order_and_newest_generation() {
        let mut queue = ProgressQueue::default();
        queue.push(ProgressEvent {
            kind: ProgressEventKind::ResourceUsageChanged {
                generation: 2,
                capacity: 4,
                used: 2,
            },
            sequence: 0,
            elapsed: Duration::ZERO,
        });
        queue.push(ProgressEvent {
            kind: ProgressEventKind::BuildStarted,
            sequence: 1,
            elapsed: Duration::ZERO,
        });
        queue.push(ProgressEvent {
            kind: ProgressEventKind::ResourceUsageChanged {
                generation: 3,
                capacity: 4,
                used: 0,
            },
            sequence: 2,
            elapsed: Duration::ZERO,
        });
        queue.push(ProgressEvent {
            kind: ProgressEventKind::ResourceUsageChanged {
                generation: 1,
                capacity: 4,
                used: 4,
            },
            sequence: 3,
            elapsed: Duration::ZERO,
        });
        assert_eq!(queue.events.keys().copied().collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn slow_observer_cannot_block_producers_and_terminal_events_are_flushed() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        let (events_tx, events_rx) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            if event.kind == ProgressEventKind::BuildStarted {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }
            events_tx.send(event).unwrap();
        }));
        session.emit(ProgressEventKind::BuildStarted);
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let producer = session.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let action = ActionKey::new(crate::PackageKey::root(), "slow-consumer").unwrap();
            for _ in 0..1_000 {
                producer
                    .wait(action.clone(), ProgressWaitKind::CacheLock)
                    .finish(ProgressWaitOutcome::Acquired);
            }
            producer.cancel_unfinished(&[action]);
            producer.emit(ProgressEventKind::BuildFailed);
            done_tx.send(()).unwrap();
        });
        let produced = done_rx.recv_timeout(Duration::from_secs(5));
        // Unblock the observer even if this assertion would fail.
        release_tx.send(()).unwrap();
        assert!(produced.is_ok(), "producer waited on the observer");
        thread.join().unwrap();
        drop(session);
        let events = events_rx.try_iter().collect::<Vec<_>>();
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        assert_eq!(events.len(), 5);
        assert!(matches!(
            events[3].kind,
            ProgressEventKind::ActionFinished { .. }
        ));
        assert_eq!(events[4].kind, ProgressEventKind::BuildFailed);
    }

    #[test]
    fn phase_lock_observer_balances_only_real_contention() {
        use crate::lock::LockWaitEvent;
        let (sender, receiver) = mpsc::channel();
        let session = ProgressSession::new(Arc::new(move |event: ProgressEvent| {
            sender.send(event).unwrap();
        }));
        let observer = session.phase_lock_observer(ProgressPhase::Runner);
        observer.observe(LockWaitEvent::Acquired);
        for outcome in [
            LockWaitEvent::Acquired,
            LockWaitEvent::Cancelled,
            LockWaitEvent::Failed,
        ] {
            observer.observe(LockWaitEvent::Contended);
            observer.observe(outcome);
        }
        drop(observer);
        drop(session);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(!events.is_empty());
        assert_eq!(events.len() % 2, 0);
        for pair in events.chunks(2) {
            assert!(matches!(
                pair[0].kind,
                ProgressEventKind::PhaseWaitStarted {
                    phase: ProgressPhase::Runner
                }
            ));
            assert!(matches!(
                pair[1].kind,
                ProgressEventKind::PhaseWaitFinished {
                    phase: ProgressPhase::Runner,
                    ..
                }
            ));
        }
    }
}
