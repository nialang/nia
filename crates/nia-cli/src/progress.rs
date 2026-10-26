// SPDX-License-Identifier: GPL-3.0-or-later
//! Build progress renderers: append-only text, the interactive status line
//! and panel, and JSON Lines. Each implements `nia_build::ProgressObserver`;
//! text renderers also own forwarded child output so a redrawn status line is
//! cleared before child bytes are written.

use std::{
    collections::BTreeMap,
    env,
    io::{self, IsTerminal, Write},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    execute,
    style::Print,
    terminal::{self, Clear, ClearType},
};
use unicode_width::UnicodeWidthChar;

use crate::cli::{DiagnosticsFormat, ProgressMode};

#[derive(Default, Clone)]
struct PlainProgressState {
    total: usize,
    selection_ready: bool,
    execution_started: bool,
    finished: usize,
    flushed: usize,
    cached: usize,
    executed: usize,
    no_work: usize,
    cancelled: usize,
    failed: usize,
    started: Option<Instant>,
    order: Vec<nia_build::ActionKey>,
    ordinals: std::collections::HashMap<nia_build::ActionKey, usize>,
    descriptions: BTreeMap<nia_build::ActionKey, nia_build::ProgressActionDescription>,
    pending: std::collections::BTreeMap<
        nia_build::ActionKey,
        (
            nia_build::ProgressOutcome,
            Option<nia_build::ActionCacheOutcome>,
            Duration,
        ),
    >,
    active: BTreeMap<nia_build::ActionKey, ActiveProgressAction>,
    queued: BTreeMap<nia_build::ActionKey, nia_build::ActionResourceClass>,
    waits: BTreeMap<nia_build::ActionKey, (nia_build::ProgressWaitKind, Instant)>,
    activities: BTreeMap<nia_build::ActionKey, nia_build::ProgressActivity>,
    current_phase: Option<(nia_build::ProgressPhase, Instant)>,
    phase_wait: Option<(nia_build::ProgressPhase, Instant)>,
    wave: usize,
    resource_generation: u64,
    resource_capacity: usize,
    resource_used: usize,
    recent_completion: Option<String>,
    build_complete: bool,
    /// Set once an action that owns the terminal, such as a run step,
    /// started at the default level. Progress then stays silent unless the
    /// build fails.
    handed_over: bool,
}

#[derive(Clone)]
struct ActiveProgressAction {
    resource_class: nia_build::ActionResourceClass,
    started: Instant,
    wave: usize,
    scheduler_wait: Duration,
    resource_wait: Duration,
}

struct PlainProgress {
    state: Arc<Mutex<PlainProgressState>>,
    verbose: u8,
    screen: Option<Arc<InteractiveScreen>>,
    refresh_worker: Mutex<Option<JoinHandle<()>>>,
    output_failed: std::sync::atomic::AtomicBool,
    plain_gate: Arc<Mutex<()>>,
}

impl PlainProgress {
    fn new(verbose: u8, interactive: bool) -> Arc<Self> {
        let state = Arc::new(Mutex::new(PlainProgressState::default()));
        let screen = interactive.then(|| {
            Arc::new(InteractiveScreen {
                state: Arc::clone(&state),
                terminal: Mutex::new(TerminalState {
                    enabled: true,
                    ..TerminalState::default()
                }),
                wake: Condvar::new(),
                verbose,
            })
        });
        let progress = Arc::new(Self {
            state,
            verbose,
            screen,
            refresh_worker: Mutex::new(None),
            output_failed: std::sync::atomic::AtomicBool::new(false),
            plain_gate: Arc::new(Mutex::new(())),
        });
        if let Some(screen) = &progress.screen
            && let Ok(worker) = thread::Builder::new()
                .name("nia-progress-renderer".into())
                .spawn({
                    let screen = Arc::clone(screen);
                    move || interactive_refresh_loop(screen)
                })
            && let Ok(mut slot) = progress.refresh_worker.lock()
        {
            *slot = Some(worker);
        }
        progress
    }

    fn write_plain(&self, lines: impl IntoIterator<Item = String>) {
        if self
            .output_failed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let _gate = self.plain_gate.lock().ok();
        let mut stderr = io::stderr().lock();
        for line in lines {
            if writeln!(stderr, "{line}").is_err() {
                self.output_failed
                    .store(true, std::sync::atomic::Ordering::Release);
                return;
            }
        }
        if stderr.flush().is_err() {
            self.output_failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn output_records(&self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        if let Some(screen) = &self.screen {
            screen.write_records(&lines);
        } else {
            self.write_plain(lines);
        }
    }

    /// Writes final lines and stops the interactive status for good.
    fn finish_output(&self, lines: &[String]) {
        if let Some(screen) = &self.screen {
            screen.finish(lines);
        } else {
            self.write_plain(lines.iter().cloned());
        }
    }
}

impl Drop for PlainProgress {
    fn drop(&mut self) {
        if let Some(screen) = &self.screen
            && let Ok(mut terminal) = screen.terminal.lock()
        {
            terminal.stopped = true;
            screen.wake.notify_all();
        }
        if let Ok(worker) = self.refresh_worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Default)]
struct TerminalState {
    enabled: bool,
    stopped: bool,
    drawn_rows: usize,
    partial_streams: BTreeMap<u64, bool>,
    next_stream_id: u64,
    broken_line: bool,
    last_draw: Option<Instant>,
}

struct InteractiveScreen {
    state: Arc<Mutex<PlainProgressState>>,
    terminal: Mutex<TerminalState>,
    wake: Condvar,
    verbose: u8,
}

impl InteractiveScreen {
    fn notify(&self) {
        self.wake.notify_all();
    }

    fn write_records(&self, lines: &[String]) {
        let Ok(mut terminal) = self.terminal.lock() else {
            return;
        };
        if terminal.enabled && clear_progress_frame(&mut terminal).is_err() {
            terminal.enabled = false;
        }
        let mut stderr = io::stderr().lock();
        for line in lines {
            if writeln!(stderr, "{line}").is_err() {
                terminal.enabled = false;
                return;
            }
        }
        let _ = stderr.flush();
        self.wake.notify_all();
    }

    fn finish(&self, lines: &[String]) {
        let Ok(mut terminal) = self.terminal.lock() else {
            return;
        };
        terminal.stopped = true;
        if terminal.enabled && clear_progress_frame(&mut terminal).is_err() {
            terminal.enabled = false;
        }
        let mut stderr = io::stderr().lock();
        if terminal.broken_line {
            let _ = stderr.write_all(b"\r\n");
            terminal.broken_line = false;
        }
        for line in lines {
            if writeln!(stderr, "{line}").is_err() {
                terminal.enabled = false;
                break;
            }
        }
        let _ = stderr.flush();
        self.wake.notify_all();
    }

    fn open_stream(
        self: &Arc<Self>,
        stream: nia_build::ProcessOutputStream,
    ) -> Box<dyn Write + Send> {
        let stream_id = if let Ok(mut terminal) = self.terminal.lock() {
            let stream_id = terminal.next_stream_id;
            terminal.next_stream_id = terminal.next_stream_id.wrapping_add(1);
            terminal.partial_streams.insert(stream_id, false);
            stream_id
        } else {
            0
        };
        Box::new(CoordinatedChildOutput {
            screen: Arc::clone(self),
            stream,
            stream_id,
            finished: false,
        })
    }
}

struct CoordinatedChildOutput {
    screen: Arc<InteractiveScreen>,
    stream: nia_build::ProcessOutputStream,
    stream_id: u64,
    finished: bool,
}

impl Write for CoordinatedChildOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut terminal = self
            .screen
            .terminal
            .lock()
            .map_err(|_| io::Error::other("progress terminal lock is poisoned"))?;
        if terminal.enabled && clear_progress_frame(&mut terminal).is_err() {
            // A terminal renderer failure disables only the frame; child
            // bytes still follow the existing forwarding/error policy.
            terminal.enabled = false;
        }
        let result = write_child_bytes(self.stream, bytes);
        if let Ok(written) = result {
            if written > 0 {
                let partial = !matches!(bytes[written - 1], b'\n' | b'\r');
                if let Some(current) = terminal.partial_streams.get_mut(&self.stream_id) {
                    *current = partial;
                }
            }
            self.screen.wake.notify_all();
            Ok(written)
        } else {
            terminal.enabled = false;
            result
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.stream {
            nia_build::ProcessOutputStream::Stdout => io::stdout().flush(),
            nia_build::ProcessOutputStream::Stderr => io::stderr().flush(),
        }
    }
}

impl Drop for CoordinatedChildOutput {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Ok(mut terminal) = self.screen.terminal.lock()
            && let Some(partial) = terminal.partial_streams.remove(&self.stream_id)
            && partial
        {
            terminal.broken_line = true;
        }
        self.screen.wake.notify_all();
        self.finished = true;
    }
}

impl nia_build::ProcessOutputWriter for PlainProgress {
    fn open_stream(&self, stream: nia_build::ProcessOutputStream) -> Box<dyn Write + Send> {
        match &self.screen {
            Some(screen) => screen.open_stream(stream),
            None => Box::new(AppendChildOutput {
                gate: Arc::clone(&self.plain_gate),
                stream,
            }),
        }
    }
}

struct AppendChildOutput {
    gate: Arc<Mutex<()>>,
    stream: nia_build::ProcessOutputStream,
}

impl Write for AppendChildOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("progress output lock is poisoned"))?;
        write_child_bytes(self.stream, bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("progress output lock is poisoned"))?;
        match self.stream {
            nia_build::ProcessOutputStream::Stdout => io::stdout().flush(),
            nia_build::ProcessOutputStream::Stderr => io::stderr().flush(),
        }
    }
}

impl nia_build::ProgressObserver for PlainProgress {
    fn observe(&self, event: nia_build::ProgressEvent) {
        use nia_build::{ProgressEventKind, ProgressOutcome};
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let build_failed = matches!(&event.kind, ProgressEventKind::BuildFailed);
        let mut lines = Vec::new();
        let mut summary = None;
        let mut handover = None;
        let occurred_at = state
            .started
            .and_then(|start| start.checked_add(event.elapsed))
            .unwrap_or_else(Instant::now);
        match event.kind {
            ProgressEventKind::BuildStarted => {
                state.started = Instant::now().checked_sub(event.elapsed)
            }
            ProgressEventKind::SelectionReady { total, actions } => {
                state.selection_ready = true;
                state.total = total;
                state.ordinals = actions
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, action)| (action, index + 1))
                    .collect();
                state.order = actions;
            }
            ProgressEventKind::ActionDescribed {
                action,
                description,
            } => {
                if self.verbose >= 3 && self.screen.is_none() {
                    lines.push(format!(
                        "Details {}: {} inputs [{}], {} outputs [{}] ({})",
                        action_progress_label(&action),
                        description.total_inputs,
                        description
                            .inputs
                            .iter()
                            .map(progress_logical_path_label)
                            .collect::<Vec<_>>()
                            .join(", "),
                        description.total_outputs,
                        description
                            .outputs
                            .iter()
                            .map(progress_logical_path_label)
                            .collect::<Vec<_>>()
                            .join(", "),
                        description.kind
                    ));
                }
                state.descriptions.insert(action, description);
            }
            ProgressEventKind::ActionCacheDecision { .. } => {}
            ProgressEventKind::ActionFinished {
                action,
                outcome,
                cache,
                duration,
            } => {
                state.active.remove(&action);
                state.queued.remove(&action);
                state.waits.remove(&action);
                state.activities.remove(&action);
                state.finished += 1;
                match outcome {
                    ProgressOutcome::Cached => state.cached += 1,
                    ProgressOutcome::Executed => state.executed += 1,
                    ProgressOutcome::NoWork => state.no_work += 1,
                    ProgressOutcome::Failed => state.failed += 1,
                    ProgressOutcome::Cancelled => state.cancelled += 1,
                }
                if self.verbose > 0 {
                    state.pending.insert(action, (outcome, cache, duration));
                    while state.flushed < state.order.len() {
                        let Some(action) = state.order.get(state.flushed).cloned() else {
                            break;
                        };
                        let Some((outcome, cache, duration)) = state.pending.remove(&action) else {
                            break;
                        };
                        let ordinal = state.ordinals.get(&action).copied().unwrap_or(0);
                        let label = action_progress_label(&action);
                        let detail = if self.verbose >= 3 {
                            cache
                                .as_ref()
                                .map(|cache| format!("; {}", cache_progress_detail(cache)))
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let line = format!(
                            "[{ordinal}/{}] {} {} ({}, {}{detail})",
                            state.total,
                            progress_outcome_name(&outcome),
                            label,
                            state
                                .descriptions
                                .get(&action)
                                .map_or("action", |description| description.kind),
                            format_duration(duration)
                        );
                        state.recent_completion = Some(line.clone());
                        lines.push(line);
                        state.flushed += 1;
                    }
                } else {
                    state.recent_completion = Some(format!(
                        "{} {} ({})",
                        progress_outcome_name(&outcome),
                        action_progress_label(&action),
                        format_duration(duration)
                    ));
                }
            }
            ProgressEventKind::BuildFinished | ProgressEventKind::BuildFailed => {
                state.build_complete = true;
                let elapsed = event.elapsed;
                summary = Some(format!(
                    "{} {} in {} ({})",
                    if build_failed {
                        "Failed after"
                    } else {
                        "Finished"
                    },
                    count_actions(state.total),
                    format_duration(elapsed),
                    outcome_counts(&state)
                ));
                if !state.selection_ready {
                    summary = Some(format!(
                        "{} build in {}",
                        if build_failed { "Failed" } else { "Finished" },
                        format_duration(elapsed)
                    ));
                } else if !state.execution_started && !build_failed {
                    summary = Some(format!(
                        "Configured build in {} ({} selected actions)",
                        format_duration(elapsed),
                        state.total
                    ));
                } else if state.handed_over && !build_failed {
                    summary = None;
                }
            }
            ProgressEventKind::PhaseStarted { phase } => {
                if phase == nia_build::ProgressPhase::Execute {
                    state.execution_started = true;
                }
                state.current_phase = Some((phase, occurred_at));
            }
            ProgressEventKind::PhaseFinished { phase, .. } => {
                if state
                    .current_phase
                    .is_some_and(|(active, _)| active == phase)
                {
                    state.current_phase = None;
                }
            }
            ProgressEventKind::PhaseWaitStarted { phase } => {
                state.phase_wait = Some((phase, occurred_at));
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!("Waiting for cache lock ({})", phase_name(phase)));
                }
            }
            ProgressEventKind::PhaseCacheDecision { phase, reused } => {
                if self.verbose >= 1 && self.screen.is_none() {
                    lines.push(format!(
                        "{}: {}",
                        phase_name(phase),
                        if reused { "reused" } else { "cache miss" }
                    ));
                }
            }
            ProgressEventKind::PhaseWaitFinished {
                phase,
                outcome,
                duration,
            } => {
                state.phase_wait = None;
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Cache lock {:?} ({}, {})",
                        outcome,
                        phase_name(phase),
                        format_duration(duration)
                    ));
                }
            }
            ProgressEventKind::WaveStarted { wave, actions } => {
                state.wave = wave;
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!("Wave {}: {} actions ready", wave + 1, actions));
                }
            }
            ProgressEventKind::ActionStarted {
                action,
                resource_class,
                wave,
                scheduler_wait,
                resource_wait,
            } => {
                state.queued.remove(&action);
                state.active.insert(
                    action.clone(),
                    ActiveProgressAction {
                        resource_class,
                        started: occurred_at,
                        wave,
                        scheduler_wait,
                        resource_wait,
                    },
                );
                if self.verbose == 0
                    && !state.handed_over
                    && state.descriptions.get(&action).is_some_and(owns_terminal)
                {
                    // Like `cargo run`, report the build before the program
                    // takes over the terminal and say nothing after it.
                    state.handed_over = true;
                    let mut lines = Vec::new();
                    if state.finished > 0 {
                        lines.push(format!(
                            "Finished {} in {} ({})",
                            count_actions(state.finished),
                            format_duration(event.elapsed),
                            outcome_counts(&state)
                        ));
                    }
                    lines.push(format!("Running {}", action_progress_label(&action)));
                    handover = Some(lines);
                }
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Running {} ({}, wave {}, queue {}, permit {})",
                        action_progress_label(&action),
                        resource_class_name(resource_class),
                        wave + 1,
                        format_duration(scheduler_wait),
                        format_duration(resource_wait)
                    ));
                }
            }
            ProgressEventKind::ActionWaitStarted { action, kind } => {
                state.waits.insert(action.clone(), (kind, occurred_at));
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Waiting for {} ({})",
                        action_progress_label(&action),
                        progress_wait_kind_name(kind)
                    ));
                }
            }
            ProgressEventKind::ActionQueued {
                action,
                resource_class,
                ..
            } => {
                state.queued.insert(action, resource_class);
            }
            ProgressEventKind::ActionWaitFinished {
                action,
                kind,
                outcome,
                duration,
            } => {
                state.waits.remove(&action);
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Wait {:?} {} ({}, {})",
                        outcome,
                        action_progress_label(&action),
                        progress_wait_kind_name(kind),
                        format_duration(duration)
                    ));
                }
            }
            ProgressEventKind::ActionActivityStarted { action, activity } => {
                state.activities.insert(action.clone(), activity);
                if self.verbose >= 3 && self.screen.is_none() {
                    lines.push(format!(
                        "Activity {} ({})",
                        action_progress_label(&action),
                        progress_activity_name(activity)
                    ));
                }
            }
            ProgressEventKind::ActionActivityFinished { action, .. } => {
                state.activities.remove(&action);
            }
            ProgressEventKind::ResourceUsageChanged {
                generation,
                capacity,
                used,
            } => {
                if generation >= state.resource_generation {
                    state.resource_generation = generation;
                    state.resource_capacity = capacity;
                    state.resource_used = used;
                }
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!("Resources: {used}/{capacity} action permits"));
                }
            }
        }
        drop(state);
        if let Some(screen) = &self.screen {
            screen.notify();
        }
        self.output_records(lines);
        if let Some(handover) = handover {
            self.finish_output(&handover);
        }
        if let Some(summary) = summary {
            self.finish_output(&[summary]);
        }
    }
}

/// An external command that declares no outputs exists for what it prints,
/// as a run step does, so it is handed the terminal.
fn owns_terminal(description: &nia_build::ProgressActionDescription) -> bool {
    description.kind == "external-command" && description.total_outputs == 0
}

fn count_actions(count: usize) -> String {
    format!("{count} action{}", if count == 1 { "" } else { "s" })
}

fn outcome_counts(state: &PlainProgressState) -> String {
    let mut counts = format!(
        "{} cached, {} executed, {} no-work",
        state.cached, state.executed, state.no_work
    );
    if state.failed > 0 {
        counts.push_str(&format!(", {} failed", state.failed));
    }
    if state.cancelled > 0 {
        counts.push_str(&format!(", {} cancelled", state.cancelled));
    }
    counts
}

fn interactive_refresh_loop(screen: Arc<InteractiveScreen>) {
    loop {
        let Ok(mut terminal_state) = screen.terminal.lock() else {
            return;
        };
        if terminal_state.stopped {
            return;
        }
        let (next_state, _) = match screen
            .wake
            .wait_timeout(terminal_state, Duration::from_millis(100))
        {
            Ok(result) => result,
            Err(_) => return,
        };
        terminal_state = next_state;
        if terminal_state.stopped {
            return;
        }
        let state = match screen.state.lock() {
            Ok(state) => state.clone(),
            Err(_) => return,
        };
        if state.build_complete
            || terminal_state.broken_line
            || !terminal_state.partial_streams.is_empty()
            || !terminal_state.enabled
        {
            continue;
        }
        let Some(started) = state.started else {
            continue;
        };
        let now = Instant::now();
        if now.duration_since(started) < Duration::from_millis(250)
            || terminal_state
                .last_draw
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(100))
        {
            continue;
        }
        let Ok((width, height)) = terminal::size() else {
            terminal_state.enabled = false;
            continue;
        };
        let lines = progress_frame_lines(&state, screen.verbose, now, width, height);
        if lines.is_empty() {
            continue;
        }
        if clear_progress_frame(&mut terminal_state).is_err() {
            terminal_state.enabled = false;
            continue;
        }
        let mut stderr = io::stderr().lock();
        let rendered = lines.iter().enumerate().try_for_each(|(index, line)| {
            if index > 0 {
                execute!(stderr, Print("\r\n"))?;
            }
            execute!(stderr, Print(line))
        });
        if rendered.is_err() || stderr.flush().is_err() {
            terminal_state.enabled = false;
            terminal_state.drawn_rows = 0;
        } else {
            terminal_state.drawn_rows = lines.len();
            terminal_state.last_draw = Some(now);
        }
    }
}

fn clear_progress_frame(terminal_state: &mut TerminalState) -> io::Result<()> {
    if terminal_state.drawn_rows == 0 {
        return Ok(());
    }
    let mut stderr = io::stderr().lock();
    for row in (0..terminal_state.drawn_rows).rev() {
        execute!(stderr, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
        if row > 0 {
            execute!(stderr, MoveUp(1))?;
        }
    }
    stderr.flush()?;
    terminal_state.drawn_rows = 0;
    Ok(())
}

fn write_child_bytes(stream: nia_build::ProcessOutputStream, bytes: &[u8]) -> io::Result<usize> {
    match stream {
        nia_build::ProcessOutputStream::Stdout => io::stdout().lock().write_all(bytes)?,
        nia_build::ProcessOutputStream::Stderr => io::stderr().lock().write_all(bytes)?,
    }
    Ok(bytes.len())
}

fn progress_frame_lines(
    state: &PlainProgressState,
    verbose: u8,
    now: Instant,
    width: u16,
    height: u16,
) -> Vec<String> {
    let elapsed = state
        .started
        .map_or(Duration::ZERO, |started| now.duration_since(started));
    if let Some((phase, started)) = state.current_phase
        && phase != nia_build::ProgressPhase::Execute
    {
        let waiting = if state.phase_wait.is_some() {
            " (cache lock)"
        } else {
            ""
        };
        return vec![truncate_progress_cells(
            &format!(
                "{}{} {}",
                phase_name(phase),
                waiting,
                format_duration(now.duration_since(started))
            ),
            width.saturating_sub(1) as usize,
        )];
    }
    if verbose < 2 {
        let (label, phase_elapsed) = if let Some(action) = state
            .order
            .iter()
            .find(|action| state.active.contains_key(*action))
        {
            (action_progress_label(action), elapsed)
        } else {
            ("Preparing build".to_string(), elapsed)
        };
        return vec![truncate_progress_cells(
            &format!(
                "Building [{}/{} actions, {} active] {} {}",
                state.finished,
                state.total,
                state.active.len(),
                label,
                format_duration(phase_elapsed)
            ),
            width.saturating_sub(1) as usize,
        )];
    }

    let mut lines = vec![truncate_progress_cells(
        &format!(
            "Building [{}/{} actions] wave {}   elapsed {}",
            state.finished,
            state.total,
            state.wave + 1,
            format_duration(elapsed)
        ),
        width.saturating_sub(1) as usize,
    )];
    lines.push(truncate_progress_cells(
        &format!(
            "{} cached  {} executed  {} failed  {} cancelled",
            state.cached, state.executed, state.failed, state.cancelled
        ),
        width.saturating_sub(1) as usize,
    ));
    lines.push(truncate_progress_cells(
        &format!(
            "action permits {}/{} reserved; {} active; {} queued; {} dependency-blocked",
            state.resource_used,
            state.resource_capacity,
            state.active.len(),
            state.queued.len(),
            state
                .waits
                .values()
                .filter(|(kind, _)| *kind == nia_build::ProgressWaitKind::Dependency)
                .count()
        ),
        width.saturating_sub(1) as usize,
    ));
    let max_rows = usize::from(height).saturating_sub(1).min(8);
    let pending = state
        .order
        .iter()
        .filter(|action| {
            state.active.contains_key(*action)
                || state.queued.contains_key(*action)
                || state.waits.contains_key(*action)
        })
        .count();
    let row_limit = if pending > max_rows.saturating_sub(lines.len()) {
        max_rows.saturating_sub(1)
    } else {
        max_rows
    };
    let mut visible = 0usize;
    for action in &state.order {
        let Some(active) = state.active.get(action) else {
            if (state.queued.contains_key(action) || state.waits.contains_key(action))
                && lines.len() < row_limit
            {
                let label = state.waits.get(action).map_or_else(
                    || "queued".to_string(),
                    |(kind, started)| {
                        format!(
                            "{} {}",
                            progress_wait_kind_name(*kind),
                            format_duration(now.saturating_duration_since(*started))
                        )
                    },
                );
                lines.push(truncate_progress_cells(
                    &format!("  {} {}", action_progress_label(action), label),
                    width.saturating_sub(1) as usize,
                ));
                visible += 1;
            }
            continue;
        };
        if lines.len() >= row_limit {
            break;
        }
        let wave = active.wave + 1;
        let active_duration = format_duration(now.duration_since(active.started));
        let queue_wait = format_duration(active.scheduler_wait);
        let permit_wait = format_duration(active.resource_wait);
        let wait = state
            .waits
            .get(action)
            .map(|(kind, started)| {
                format!(
                    " wait:{} {}",
                    progress_wait_kind_name(*kind),
                    format_duration(now.duration_since(*started))
                )
            })
            .unwrap_or_default();
        let activity = state
            .activities
            .get(action)
            .map(|activity| format!(" activity:{}", progress_activity_name(*activity)))
            .unwrap_or_default();
        lines.push(truncate_progress_cells(
            &format!(
                "  {:<12} {}  wave {}  {}  q:{} p:{}{wait}{activity}",
                resource_class_name(active.resource_class),
                action_progress_label(action),
                wave,
                active_duration,
                queue_wait,
                permit_wait
            ),
            width.saturating_sub(1) as usize,
        ));
        visible += 1;
    }
    let pending = state
        .order
        .iter()
        .filter(|action| {
            state.active.contains_key(*action)
                || state.queued.contains_key(*action)
                || state.waits.contains_key(*action)
        })
        .count();
    let hidden = pending.saturating_sub(visible);
    if hidden > 0 && lines.len() < max_rows {
        lines.push(format!("  +{hidden} actions"));
    }
    if verbose >= 3
        && let Some(recent) = &state.recent_completion
        && lines.len() < max_rows
    {
        lines.push(truncate_progress_cells(
            recent,
            width.saturating_sub(1) as usize,
        ));
    }
    lines.truncate(max_rows.max(1));
    lines
}

fn truncate_progress_cells(text: &str, max_width: usize) -> String {
    if unicode_width::UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let suffix = "...";
    let target = max_width.saturating_sub(suffix.len());
    let mut output = String::new();
    let mut width = 0usize;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > target {
            break;
        }
        output.push(character);
        width += character_width;
    }
    output.push_str(&suffix[..suffix.len().min(max_width)]);
    output
}

fn action_progress_label(action: &nia_build::ActionKey) -> String {
    format!(
        "{}:{}",
        escape_progress_label(action.package().as_str()),
        escape_progress_label(action.name())
    )
}

fn progress_outcome_name(outcome: &nia_build::ProgressOutcome) -> &'static str {
    match outcome {
        nia_build::ProgressOutcome::Cached => "Cached",
        nia_build::ProgressOutcome::Executed => "Executed",
        nia_build::ProgressOutcome::NoWork => "Complete",
        nia_build::ProgressOutcome::Failed => "Failed",
        nia_build::ProgressOutcome::Cancelled => "Cancelled",
    }
}

fn cache_progress_detail(cache: &nia_build::ActionCacheOutcome) -> &'static str {
    match cache {
        nia_build::ActionCacheOutcome::Hit => "cache hit",
        nia_build::ActionCacheOutcome::Unavailable => "cache prediction unavailable",
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::NotFound) => {
            "cache miss: not found"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::Invalidated(_)) => {
            "cache miss: invalidated"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::Uncacheable) => {
            "cache miss: uncacheable"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::Corrupt) => {
            "cache miss: corrupt"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::ReadError) => {
            "cache miss: read error"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::WriteError) => {
            "cache miss: write error"
        }
    }
}

fn resource_class_name(class: nia_build::ActionResourceClass) -> &'static str {
    match class {
        nia_build::ActionResourceClass::Cpu => "cpu",
        nia_build::ActionResourceClass::Io => "io",
        nia_build::ActionResourceClass::Conservative => "conservative",
    }
}

fn progress_wait_kind_name(kind: nia_build::ProgressWaitKind) -> &'static str {
    match kind {
        nia_build::ProgressWaitKind::Dependency => "dependency",
        nia_build::ProgressWaitKind::ResourcePermit => "resource permit",
        nia_build::ProgressWaitKind::OutputLock => "output lock",
        nia_build::ProgressWaitKind::CacheLock => "cache lock",
    }
}

fn progress_activity_name(activity: nia_build::ProgressActivity) -> &'static str {
    match activity {
        nia_build::ProgressActivity::Checking => "checking",
        nia_build::ProgressActivity::Codegen => "codegen",
        nia_build::ProgressActivity::Linking => "linking",
        nia_build::ProgressActivity::CompilingAndLinking => "compiling and linking",
    }
}

fn interactive_terminal_available() -> bool {
    let term = env::var("TERM").unwrap_or_default();
    io::stderr().is_terminal()
        && !term.eq_ignore_ascii_case("dumb")
        && env::var_os("CI").is_none()
        && terminal::size().is_ok()
}

pub(crate) struct ProgressOutput {
    pub(crate) observer: Arc<dyn nia_build::ProgressObserver>,
    pub(crate) process_output_writer: Option<Arc<dyn nia_build::ProcessOutputWriter>>,
}

#[derive(Default)]
struct JsonProgressState {
    started: Option<Instant>,
    total: usize,
    selection_ready: bool,
    completed: usize,
    ordinals: std::collections::HashMap<nia_build::ActionKey, usize>,
    descriptions: BTreeMap<nia_build::ActionKey, nia_build::ProgressActionDescription>,
}

struct JsonProgress {
    state: Mutex<JsonProgressState>,
    verbosity: u8,
    output_failed: std::sync::atomic::AtomicBool,
}

impl nia_build::ProgressObserver for JsonProgress {
    fn observe(&self, event: nia_build::ProgressEvent) {
        use nia_build::ProgressEventKind;
        if self
            .output_failed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let elapsed = event.elapsed.as_millis() as u64;
        let mut record = serde_json::json!({
            "kind": "build-progress",
            "schema_version": 1,
            "sequence": event.sequence,
            "elapsed_ms": elapsed,
        });
        let object = record.as_object_mut().expect("progress record object");
        let action = match &event.kind {
            ProgressEventKind::ActionDescribed { action, .. }
            | ProgressEventKind::ActionCacheDecision { action, .. }
            | ProgressEventKind::ActionQueued { action, .. }
            | ProgressEventKind::ActionStarted { action, .. }
            | ProgressEventKind::ActionWaitStarted { action, .. }
            | ProgressEventKind::ActionWaitFinished { action, .. }
            | ProgressEventKind::ActionActivityStarted { action, .. }
            | ProgressEventKind::ActionActivityFinished { action, .. }
            | ProgressEventKind::ActionFinished { action, .. } => Some(action),
            _ => None,
        };
        if let Some(action) = action
            && let Some(ordinal) = state.ordinals.get(action)
        {
            object.insert("ordinal".into(), (*ordinal).into());
        }
        match event.kind {
            ProgressEventKind::BuildStarted => {
                state.started = Some(Instant::now());
                object.insert("elapsed_ms".into(), 0.into());
                object.insert("event".into(), "build-started".into());
            }
            ProgressEventKind::BuildFinished => {
                object.insert("event".into(), "build-finished".into());
                if state.selection_ready {
                    object.insert("completed".into(), state.completed.into());
                    object.insert("total".into(), state.total.into());
                }
            }
            ProgressEventKind::BuildFailed => {
                object.insert("event".into(), "build-failed".into());
                if state.selection_ready {
                    object.insert("completed".into(), state.completed.into());
                    object.insert("total".into(), state.total.into());
                }
            }
            ProgressEventKind::PhaseStarted { phase } => {
                object.insert("event".into(), "phase-started".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
            }
            ProgressEventKind::PhaseFinished {
                phase,
                duration,
                success,
            } => {
                object.insert("event".into(), "phase-finished".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
                object.insert("success".into(), success.into());
            }
            ProgressEventKind::PhaseWaitStarted { phase } if self.verbosity >= 2 => {
                object.insert("event".into(), "phase-wait-started".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("wait".into(), "cache lock".into());
            }
            ProgressEventKind::PhaseCacheDecision { phase, reused } if self.verbosity >= 1 => {
                object.insert("event".into(), "phase-cache-decision".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("reused".into(), reused.into());
            }
            ProgressEventKind::PhaseWaitFinished {
                phase,
                outcome,
                duration,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "phase-wait-finished".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("wait".into(), "cache lock".into());
                object.insert(
                    "outcome".into(),
                    format!("{outcome:?}").to_lowercase().into(),
                );
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
            }
            ProgressEventKind::SelectionReady { total, actions } => {
                state.selection_ready = true;
                state.total = total;
                state.ordinals = actions
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, action)| (action, index + 1))
                    .collect();
                object.insert("event".into(), "selection-ready".into());
                object.insert("total".into(), total.into());
                object.insert(
                    "actions".into(),
                    actions
                        .into_iter()
                        .map(|action| {
                            serde_json::json!({
                                "package": action.package().as_str(),
                                "name": action.name(),
                            })
                        })
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
            ProgressEventKind::ActionDescribed {
                action,
                description,
            } => {
                state
                    .descriptions
                    .insert(action.clone(), description.clone());
                if self.verbosity < 3 {
                    return;
                }
                object.insert("event".into(), "action-described".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert("action_kind".into(), description.kind.into());
                object.insert("total_inputs".into(), description.total_inputs.into());
                object.insert("total_outputs".into(), description.total_outputs.into());
                object.insert(
                    "inputs".into(),
                    description
                        .inputs
                        .iter()
                        .map(progress_logical_path_json)
                        .collect::<Vec<_>>()
                        .into(),
                );
                object.insert(
                    "outputs".into(),
                    description
                        .outputs
                        .iter()
                        .map(progress_logical_path_json)
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
            ProgressEventKind::WaveStarted { wave, actions } if self.verbosity >= 2 => {
                object.insert("event".into(), "wave-started".into());
                object.insert("wave".into(), wave.into());
                object.insert("actions".into(), actions.into());
            }
            ProgressEventKind::ActionStarted {
                action,
                resource_class,
                wave,
                scheduler_wait,
                resource_wait,
            } if self.verbosity >= 1 => {
                object.insert("event".into(), "action-started".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert(
                    "resource_class".into(),
                    format!("{resource_class:?}").to_lowercase().into(),
                );
                object.insert("wave".into(), wave.into());
                object.insert(
                    "scheduler_wait_ms".into(),
                    (scheduler_wait.as_millis() as u64).into(),
                );
                object.insert(
                    "resource_wait_ms".into(),
                    (resource_wait.as_millis() as u64).into(),
                );
            }
            ProgressEventKind::ActionWaitStarted { action, kind } if self.verbosity >= 2 => {
                object.insert("event".into(), "action-wait-started".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("wait".into(), progress_wait_kind_name(kind).into());
            }
            ProgressEventKind::ActionQueued {
                action,
                resource_class,
                wave,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "action-queued".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert(
                    "resource_class".into(),
                    resource_class_name(resource_class).into(),
                );
                object.insert("wave".into(), wave.into());
            }
            ProgressEventKind::ActionWaitFinished {
                action,
                kind,
                outcome,
                duration,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "action-wait-finished".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("wait".into(), progress_wait_kind_name(kind).into());
                object.insert(
                    "outcome".into(),
                    format!("{outcome:?}").to_lowercase().into(),
                );
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
            }
            ProgressEventKind::ActionActivityStarted { action, activity }
                if self.verbosity >= 3 =>
            {
                object.insert("event".into(), "action-activity-started".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("activity".into(), progress_activity_name(activity).into());
            }
            ProgressEventKind::ActionActivityFinished {
                action,
                activity,
                duration,
                success,
            } if self.verbosity >= 3 => {
                object.insert("event".into(), "action-activity-finished".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("activity".into(), progress_activity_name(activity).into());
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
                object.insert("success".into(), success.into());
            }
            ProgressEventKind::ResourceUsageChanged {
                generation,
                capacity,
                used,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "resource-usage-changed".into());
                object.insert("generation".into(), generation.into());
                object.insert("capacity".into(), capacity.into());
                object.insert("used".into(), used.into());
            }
            ProgressEventKind::ActionCacheDecision { action, cache } if self.verbosity >= 1 => {
                object.insert("event".into(), "action-cache-decision".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert("cache".into(), cache_json_value(cache));
            }
            ProgressEventKind::ActionFinished {
                action,
                outcome,
                cache,
                duration,
            } => {
                object.insert("event".into(), "action-finished".into());
                if let Some(description) = state.descriptions.get(&action) {
                    object.insert("action_kind".into(), description.kind.into());
                }
                state.completed += 1;
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert(
                    "ordinal".into(),
                    state
                        .ordinals
                        .get(&action)
                        .copied()
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                );
                object.insert(
                    "outcome".into(),
                    format!("{outcome:?}").to_lowercase().into(),
                );
                if let Some(cache) = cache {
                    object.insert("cache".into(), cache_json_value(cache));
                }
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
                object.insert("completed".into(), state.completed.into());
                object.insert("total".into(), state.total.into());
            }
            ProgressEventKind::WaveStarted { .. }
            | ProgressEventKind::ActionCacheDecision { .. }
            | ProgressEventKind::PhaseCacheDecision { .. }
            | ProgressEventKind::ActionQueued { .. }
            | ProgressEventKind::PhaseWaitStarted { .. }
            | ProgressEventKind::PhaseWaitFinished { .. }
            | ProgressEventKind::ActionStarted { .. }
            | ProgressEventKind::ActionWaitStarted { .. }
            | ProgressEventKind::ActionWaitFinished { .. }
            | ProgressEventKind::ActionActivityStarted { .. }
            | ProgressEventKind::ActionActivityFinished { .. }
            | ProgressEventKind::ResourceUsageChanged { .. } => return,
        }
        drop(state);
        if writeln!(io::stderr().lock(), "{}", record).is_err() {
            self.output_failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

fn progress_logical_path_json(path: &nia_build::LogicalPath) -> serde_json::Value {
    use nia_build::LogicalPathRoot;
    let root = match path.root() {
        LogicalPathRoot::Package(package) => {
            serde_json::json!({"kind": "package", "package": package.as_str()})
        }
        LogicalPathRoot::Build => serde_json::json!({"kind": "build"}),
        LogicalPathRoot::Cache => serde_json::json!({"kind": "cache"}),
        LogicalPathRoot::Toolchain => serde_json::json!({"kind": "toolchain"}),
        LogicalPathRoot::Artifact(artifact) => {
            serde_json::json!({"kind": "artifact", "package": artifact.package().as_str(), "artifact": artifact.name()})
        }
    };
    serde_json::json!({"root": root, "path": path.protocol_path()})
}

fn progress_logical_path_label(path: &nia_build::LogicalPath) -> String {
    use nia_build::LogicalPathRoot;
    let root = match path.root() {
        LogicalPathRoot::Package(package) => {
            format!("package:{}", escape_progress_label(package.as_str()))
        }
        LogicalPathRoot::Build => "build".into(),
        LogicalPathRoot::Cache => "cache".into(),
        LogicalPathRoot::Toolchain => "toolchain".into(),
        LogicalPathRoot::Artifact(artifact) => format!(
            "artifact:{}:{}",
            escape_progress_label(artifact.package().as_str()),
            escape_progress_label(artifact.name())
        ),
    };
    format!("{root}/{}", escape_progress_label(&path.protocol_path()))
}

fn cache_json_value(cache: nia_build::ActionCacheOutcome) -> serde_json::Value {
    use nia_build::ActionCacheOutcome;
    match cache {
        ActionCacheOutcome::Hit => serde_json::json!({"status": "hit"}),
        ActionCacheOutcome::Unavailable => serde_json::json!({
            "status": "unavailable",
            "reason": "identity_unavailable"
        }),
        ActionCacheOutcome::Miss(reason) => {
            let reason = match reason {
                nia_build::ActionCacheMissReason::NotFound => "not_found",
                nia_build::ActionCacheMissReason::Invalidated(_) => "invalidated",
                nia_build::ActionCacheMissReason::Uncacheable => "uncacheable",
                nia_build::ActionCacheMissReason::Corrupt => "corrupt",
                nia_build::ActionCacheMissReason::ReadError => "read_error",
                nia_build::ActionCacheMissReason::WriteError => "write_error",
            };
            serde_json::json!({"status": "miss", "reason": reason})
        }
    }
}

fn escape_progress_label(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect::<String>()
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.1}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn phase_name(phase: nia_build::ProgressPhase) -> &'static str {
    match phase {
        nia_build::ProgressPhase::Resolve => "Resolving package graph",
        nia_build::ProgressPhase::Prepare => "Preparing build directories",
        nia_build::ProgressPhase::Runner => "Preparing build runner",
        nia_build::ProgressPhase::Configure => "Configuring build plan",
        nia_build::ProgressPhase::Preview => "Selecting build actions",
        nia_build::ProgressPhase::Execute => "Executing build actions",
    }
}

pub(crate) fn progress_observer(
    mode: ProgressMode,
    quiet: u8,
    verbose: u8,
    diagnostics_format: DiagnosticsFormat,
) -> Option<ProgressOutput> {
    if quiet > 0
        || (mode == ProgressMode::None)
        || (mode == ProgressMode::Auto && diagnostics_format == DiagnosticsFormat::Json)
    {
        return None;
    }
    if mode == ProgressMode::Json {
        let observer: Arc<dyn nia_build::ProgressObserver> = Arc::new(JsonProgress {
            state: Mutex::new(JsonProgressState::default()),
            verbosity: verbose,
            output_failed: std::sync::atomic::AtomicBool::new(false),
        });
        Some(ProgressOutput {
            observer,
            process_output_writer: None,
        })
    } else {
        let interactive = mode == ProgressMode::Auto && interactive_terminal_available();
        let progress = PlainProgress::new(verbose, interactive);
        let observer: Arc<dyn nia_build::ProgressObserver> = progress.clone();
        let process_output_writer =
            Some(progress.clone() as Arc<dyn nia_build::ProcessOutputWriter>);
        Some(ProgressOutput {
            observer,
            process_output_writer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_labels_escape_terminal_controls() {
        assert_eq!(escape_progress_label("pkg\n\x1b[31m"), "pkg\\n\\u{1b}[31m");
        assert_eq!(escape_progress_label("任务/编译"), "任务/编译");
    }

    #[test]
    fn progress_frame_truncates_by_display_width() {
        let rendered = truncate_progress_cells("任务/very-long-action", 10);
        assert!(unicode_width::UnicodeWidthStr::width(rendered.as_str()) <= 10);
        assert!(rendered.ends_with("..."));
    }

    #[test]
    fn progress_panel_caps_rows_and_keeps_canonical_order() {
        let first = nia_build::ActionKey::new(nia_build::PackageKey::root(), "first")
            .expect("first action");
        let second = nia_build::ActionKey::new(nia_build::PackageKey::root(), "second")
            .expect("second action");
        let now = Instant::now();
        let mut state = PlainProgressState {
            total: 2,
            finished: 0,
            started: Some(now),
            order: vec![first.clone(), second.clone()],
            wave: 0,
            resource_capacity: 2,
            resource_used: 2,
            ..PlainProgressState::default()
        };
        state.active.insert(
            first,
            ActiveProgressAction {
                resource_class: nia_build::ActionResourceClass::Cpu,
                started: now,
                wave: 0,
                scheduler_wait: Duration::ZERO,
                resource_wait: Duration::ZERO,
            },
        );
        state.active.insert(
            second,
            ActiveProgressAction {
                resource_class: nia_build::ActionResourceClass::Io,
                started: now,
                wave: 0,
                scheduler_wait: Duration::ZERO,
                resource_wait: Duration::ZERO,
            },
        );
        let lines = progress_frame_lines(&state, 2, now, 80, 6);
        assert!(lines.len() <= 5);
        assert!(lines.iter().any(|line| line.contains("first")));
        assert!(lines.iter().any(|line| line.contains("second")));
    }

    #[test]
    fn progress_panel_exposes_blocked_actions_without_inventing_active_workers() {
        let now = Instant::now();
        let keys = (0..10)
            .map(|index| {
                nia_build::ActionKey::new(nia_build::PackageKey::root(), format!("task-{index}"))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut state = PlainProgressState {
            total: 10,
            order: keys.clone(),
            started: Some(now),
            resource_capacity: 2,
            ..Default::default()
        };
        state
            .queued
            .insert(keys[0].clone(), nia_build::ActionResourceClass::Cpu);
        for action in keys {
            state
                .waits
                .insert(action, (nia_build::ProgressWaitKind::Dependency, now));
        }
        let lines = progress_frame_lines(&state, 2, now, 100, 10);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("0 active; 1 queued; 10 dependency-blocked"))
        );
        assert!(lines.iter().any(|line| line.contains("dependency")));
        assert!(lines.iter().any(|line| line.contains("+6 actions")));
        assert!(lines.len() <= 8);
    }

    #[test]
    fn bootstrap_panel_names_real_phase_and_cache_wait() {
        let now = Instant::now();
        let state = PlainProgressState {
            current_phase: Some((nia_build::ProgressPhase::Configure, now)),
            phase_wait: Some((nia_build::ProgressPhase::Configure, now)),
            ..Default::default()
        };
        let lines = progress_frame_lines(&state, 3, now, 80, 10);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Configuring build plan (cache lock)"));
    }
}
