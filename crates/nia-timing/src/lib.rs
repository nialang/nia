// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimingMode {
    #[default]
    Off,
    Summary,
    Detail,
}

impl TimingMode {
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn detail(self) -> bool {
        matches!(self, Self::Detail)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingLevel {
    Summary,
    Detail,
}

impl TimingMode {
    pub fn includes(self, level: TimingLevel) -> bool {
        match level {
            TimingLevel::Summary => self.enabled(),
            TimingLevel::Detail => self.detail(),
        }
    }
}

pub fn time_stage<T>(mode: TimingMode, level: TimingLevel, name: &str, f: impl FnOnce() -> T) -> T {
    if !mode.includes(level) {
        return f();
    }
    let start = Instant::now();
    let result = f();
    emit_timing(name, start.elapsed());
    result
}

pub fn time_query<T>(mode: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    if !mode.detail() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    emit_query_timing(name, start.elapsed());
    result
}

pub fn time_query_if_slow<T>(
    mode: TimingMode,
    name: &str,
    threshold: Duration,
    f: impl FnOnce() -> T,
) -> T {
    if !mode.detail() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed();
    if elapsed >= threshold {
        emit_timing(name, elapsed);
    }
    result
}

pub fn emit_timing(name: &str, elapsed: Duration) {
    emit(TimingEventKind::Stage, name, elapsed);
}

pub fn emit_query_timing(name: &str, elapsed: Duration) {
    emit(TimingEventKind::Query, name, elapsed);
}

pub fn collect_to_stderr<T>(f: impl FnOnce() -> T) -> T {
    let already_collecting = TIMING_EVENTS.with(|events| events.borrow().is_some());
    if already_collecting {
        return f();
    }

    TIMING_EVENTS.with(|events| {
        *events.borrow_mut() = Some(Vec::new());
    });
    let _guard = TimingCollectionGuard;
    f()
}

#[derive(Debug, Clone)]
struct TimingEvent {
    kind: TimingEventKind,
    name: String,
    elapsed: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimingEventKind {
    Stage,
    Query,
}

thread_local! {
    static TIMING_EVENTS: RefCell<Option<Vec<TimingEvent>>> = const { RefCell::new(None) };
}

struct TimingCollectionGuard;

impl Drop for TimingCollectionGuard {
    fn drop(&mut self) {
        let events = TIMING_EVENTS.with(|events| events.borrow_mut().take().unwrap_or_default());
        for event in events {
            print_event(&event);
        }
    }
}

fn emit(kind: TimingEventKind, name: &str, elapsed: Duration) {
    let captured = TIMING_EVENTS.with(|events| {
        let mut events = events.borrow_mut();
        let Some(events) = events.as_mut() else {
            return false;
        };
        events.push(TimingEvent {
            kind,
            name: name.to_string(),
            elapsed,
        });
        true
    });
    if !captured {
        print_event(&TimingEvent {
            kind,
            name: name.to_string(),
            elapsed,
        });
    }
}

fn print_event(event: &TimingEvent) {
    eprintln!("{}", format_event(event));
}

fn format_event(event: &TimingEvent) -> String {
    match event.kind {
        TimingEventKind::Stage => {
            format!("timing {}: {:.3}s", event.name, event.elapsed.as_secs_f64())
        }
        TimingEventKind::Query => {
            format!(
                "query timing {}: {:.3}s",
                event.name,
                event.elapsed.as_secs_f64()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_mode_levels_are_explicit() {
        assert!(!TimingMode::Off.includes(TimingLevel::Summary));
        assert!(!TimingMode::Off.includes(TimingLevel::Detail));
        assert!(TimingMode::Summary.includes(TimingLevel::Summary));
        assert!(!TimingMode::Summary.includes(TimingLevel::Detail));
        assert!(TimingMode::Detail.includes(TimingLevel::Summary));
        assert!(TimingMode::Detail.includes(TimingLevel::Detail));
    }

    #[test]
    fn formats_stage_and_query_timings_separately() {
        let elapsed = Duration::from_millis(7);
        assert_eq!(
            format_event(&TimingEvent {
                kind: TimingEventKind::Stage,
                name: "check".to_string(),
                elapsed,
            }),
            "timing check: 0.007s"
        );
        assert_eq!(
            format_event(&TimingEvent {
                kind: TimingEventKind::Query,
                name: "checked_module".to_string(),
                elapsed,
            }),
            "query timing checked_module: 0.007s"
        );
    }

    #[test]
    fn collector_restores_outer_state_after_run() {
        collect_to_stderr(|| {
            emit_timing("outer", Duration::from_millis(1));
            collect_to_stderr(|| {
                emit_query_timing("inner", Duration::from_millis(2));
            });
        });
        TIMING_EVENTS.with(|events| assert!(events.borrow().is_none()));
    }
}
