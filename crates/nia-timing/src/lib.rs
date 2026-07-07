// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread::ThreadId,
    time::{Duration, Instant},
};

const TIMING_REPORT_ENTRY_LIMIT: usize = 64;

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

pub fn time_detail<T>(enabled: bool, name: &str, f: impl FnOnce() -> T) -> T {
    if !enabled {
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

pub fn emit_timing(name: impl Into<String>, elapsed: Duration) {
    emit(
        TimingEventKind::Stage,
        name.into(),
        TimingMeasurement::single(elapsed),
    );
}

pub fn emit_query_timing(name: impl Into<String>, elapsed: Duration) {
    emit(
        TimingEventKind::Query,
        name.into(),
        TimingMeasurement::single(elapsed),
    );
}

pub fn emit_query_measurement(name: impl Into<String>, measurement: TimingMeasurement) {
    emit(TimingEventKind::Query, name.into(), measurement);
}

pub fn emit_query_note(name: impl Into<String>, detail: impl Into<String>) {
    emit_note(TimingEventKind::Query, name.into(), detail.into());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingMeasurement {
    pub total: Duration,
    pub max: Duration,
    pub count: usize,
}

impl TimingMeasurement {
    pub fn single(elapsed: Duration) -> Self {
        Self {
            total: elapsed,
            max: elapsed,
            count: 1,
        }
    }
}

#[derive(Debug, Default)]
pub struct TimingAccumulator {
    entries: HashMap<&'static str, TimingAccumulatorEntry>,
}

#[derive(Debug, Default)]
struct TimingAccumulatorEntry {
    total: Duration,
    max: Duration,
    count: usize,
}

impl TimingAccumulator {
    pub fn time<T>(&mut self, name: &'static str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let result = f();
        let elapsed = start.elapsed();
        let entry = self.entries.entry(name).or_default();
        entry.total += elapsed;
        entry.max = entry.max.max(elapsed);
        entry.count += 1;
        result
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn emit_query_timings(&self, name_suffix: impl Fn(&'static str) -> String) {
        let mut entries = self.entries.iter().collect::<Vec<_>>();
        entries.sort_by(|(_, left), (_, right)| right.total.cmp(&left.total));
        for (name, entry) in entries {
            emit_query_measurement(
                name_suffix(name),
                TimingMeasurement {
                    total: entry.total,
                    max: entry.max,
                    count: entry.count,
                },
            );
        }
    }
}

pub fn collect_to_stderr<T>(f: impl FnOnce() -> T) -> T {
    let collector = {
        let current_thread = std::thread::current().id();
        let (state, finished) = timing_collector_state();
        let mut state = state.lock().expect("timing collector state poisoned");
        let nested_on_owner = state
            .active
            .as_ref()
            .is_some_and(|active| active.owner == current_thread);
        if nested_on_owner {
            drop(state);
            return f();
        }
        while state.active.is_some() {
            state = finished
                .wait(state)
                .expect("timing collector state poisoned");
        }
        let collector = Arc::new(Mutex::new(TimingCollector::new(
            timing_trace_events_enabled(),
        )));
        state.active = Some(ActiveTimingCollector {
            owner: current_thread,
            collector: Arc::clone(&collector),
        });
        collector
    };

    struct TimingCollectionGuard {
        collector: Arc<Mutex<TimingCollector>>,
    }

    impl Drop for TimingCollectionGuard {
        fn drop(&mut self) {
            let (state, finished) = timing_collector_state();
            let mut state = state.lock().expect("timing collector state poisoned");
            let Some(current) = state.active.as_ref() else {
                return;
            };
            if !Arc::ptr_eq(&current.collector, &self.collector) {
                return;
            }
            state.active = None;
            finished.notify_all();
            drop(state);

            let (report, trace_events) = self
                .collector
                .lock()
                .expect("timing collector poisoned")
                .drain();
            if let Some(events) = trace_events {
                for event in events {
                    print_event(&event);
                }
            }
            print_report(&report);
        }
    }

    let _guard = TimingCollectionGuard { collector };
    f()
}

#[derive(Debug, Clone)]
struct TimingEvent {
    kind: TimingEventKind,
    name: String,
    data: TimingEventData,
}

#[derive(Debug, Clone)]
enum TimingEventData {
    Measurement(TimingMeasurement),
    Note(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TimingEventKind {
    Stage,
    Query,
}

#[derive(Debug, Default)]
struct TimingCollectorState {
    active: Option<ActiveTimingCollector>,
}

#[derive(Debug)]
struct ActiveTimingCollector {
    owner: ThreadId,
    collector: Arc<Mutex<TimingCollector>>,
}

fn timing_collector_state() -> &'static (Mutex<TimingCollectorState>, Condvar) {
    static COLLECTOR: OnceLock<(Mutex<TimingCollectorState>, Condvar)> = OnceLock::new();
    COLLECTOR.get_or_init(|| (Mutex::new(TimingCollectorState::default()), Condvar::new()))
}

fn active_timing_collector() -> Option<Arc<Mutex<TimingCollector>>> {
    timing_collector_state()
        .0
        .lock()
        .expect("timing collector state poisoned")
        .active
        .as_ref()
        .map(|active| Arc::clone(&active.collector))
}

fn timing_trace_events_enabled() -> bool {
    matches!(
        std::env::var("NIA_TIMING_TRACE_EVENTS").as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}

fn emit(kind: TimingEventKind, name: String, measurement: TimingMeasurement) {
    if let Some(collector) = active_timing_collector() {
        collector
            .lock()
            .expect("timing collector poisoned")
            .emit_measurement(kind, name, measurement);
        return;
    }
    print_event(&TimingEvent {
        kind,
        name,
        data: TimingEventData::Measurement(measurement),
    });
}

fn emit_note(kind: TimingEventKind, name: String, detail: String) {
    if let Some(collector) = active_timing_collector() {
        collector
            .lock()
            .expect("timing collector poisoned")
            .emit_note(kind, name, detail);
        return;
    }
    print_event(&TimingEvent {
        kind,
        name,
        data: TimingEventData::Note(detail),
    });
}

fn print_event(event: &TimingEvent) {
    eprintln!("{}", format_event(event));
}

fn format_event(event: &TimingEvent) -> String {
    match (event.kind, &event.data) {
        (TimingEventKind::Stage, TimingEventData::Measurement(measurement)) => {
            format_measurement_event("timing", &event.name, *measurement)
        }
        (TimingEventKind::Query, TimingEventData::Measurement(measurement)) => {
            format_measurement_event("query timing", &event.name, *measurement)
        }
        (TimingEventKind::Stage, TimingEventData::Note(detail)) => {
            format!("timing {}: {}", event.name, detail)
        }
        (TimingEventKind::Query, TimingEventData::Note(detail)) => {
            format!("query timing {}: {}", event.name, detail)
        }
    }
}

fn format_measurement_event(prefix: &str, name: &str, measurement: TimingMeasurement) -> String {
    if measurement.count == 1 {
        return format!("{prefix} {name}: {:.3}s", measurement.total.as_secs_f64());
    }
    format!(
        "{prefix} {name}: total={:.3}s count={} max={:.3}s",
        measurement.total.as_secs_f64(),
        measurement.count,
        measurement.max.as_secs_f64()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimingReport {
    entries: Vec<TimingReportEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimingReportEntry {
    kind: TimingEventKind,
    name: String,
    count: usize,
    total: Duration,
    max: Duration,
}

#[derive(Debug)]
struct TimingCollector {
    report: TimingReportBuilder,
    trace_events: Option<Vec<TimingEvent>>,
}

impl TimingCollector {
    fn new(trace_events: bool) -> Self {
        Self {
            report: TimingReportBuilder::default(),
            trace_events: trace_events.then(Vec::new),
        }
    }

    fn emit_measurement(
        &mut self,
        kind: TimingEventKind,
        name: String,
        measurement: TimingMeasurement,
    ) {
        if let Some(events) = &mut self.trace_events {
            events.push(TimingEvent {
                kind,
                name: name.clone(),
                data: TimingEventData::Measurement(measurement),
            });
        }
        self.report.record(kind, name, measurement);
    }

    fn emit_note(&mut self, kind: TimingEventKind, name: String, detail: String) {
        if let Some(events) = &mut self.trace_events {
            events.push(TimingEvent {
                kind,
                name,
                data: TimingEventData::Note(detail),
            });
        }
    }

    fn drain(&mut self) -> (TimingReport, Option<Vec<TimingEvent>>) {
        (
            std::mem::take(&mut self.report).finish(),
            self.trace_events.take(),
        )
    }
}

#[derive(Debug, Default)]
struct TimingReportBuilder {
    entries_by_key: HashMap<(TimingEventKind, String), TimingReportEntry>,
}

impl TimingReportBuilder {
    fn record(&mut self, kind: TimingEventKind, name: String, measurement: TimingMeasurement) {
        let entry = self
            .entries_by_key
            .entry((kind, name.clone()))
            .or_insert_with(|| TimingReportEntry {
                kind,
                name,
                count: 0,
                total: Duration::ZERO,
                max: Duration::ZERO,
            });
        entry.count += measurement.count;
        entry.total += measurement.total;
        entry.max = entry.max.max(measurement.max);
    }

    fn finish(self) -> TimingReport {
        let mut entries = self.entries_by_key.into_values().collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .total
                .cmp(&left.total)
                .then_with(|| left.name.cmp(&right.name))
        });
        TimingReport { entries }
    }
}

impl TimingReport {
    #[cfg(test)]
    fn from_events(events: &[TimingEvent]) -> Self {
        let mut builder = TimingReportBuilder::default();
        for event in events {
            let TimingEventData::Measurement(measurement) = &event.data else {
                continue;
            };
            builder.record(event.kind, event.name.clone(), *measurement);
        }
        builder.finish()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn print_report(report: &TimingReport) {
    if report.is_empty() {
        return;
    }
    eprintln!("timing summary:");
    for entry in report.entries.iter().take(TIMING_REPORT_ENTRY_LIMIT) {
        eprintln!("{}", format_report_entry(entry));
    }
    if report.entries.len() > TIMING_REPORT_ENTRY_LIMIT {
        eprintln!(
            "timing summary omitted {} entries",
            report.entries.len() - TIMING_REPORT_ENTRY_LIMIT
        );
    }
}

fn format_report_entry(entry: &TimingReportEntry) -> String {
    let prefix = match entry.kind {
        TimingEventKind::Stage => "timing summary stage",
        TimingEventKind::Query => "timing summary query",
    };
    format!(
        "{prefix} {}: total={:.3}s count={} max={:.3}s",
        entry.name,
        entry.total.as_secs_f64(),
        entry.count,
        entry.max.as_secs_f64()
    )
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
                data: TimingEventData::Measurement(TimingMeasurement::single(elapsed)),
            }),
            "timing check: 0.007s"
        );
        assert_eq!(
            format_event(&TimingEvent {
                kind: TimingEventKind::Query,
                name: "checked_module".to_string(),
                data: TimingEventData::Measurement(TimingMeasurement::single(elapsed)),
            }),
            "query timing checked_module: 0.007s"
        );
        assert_eq!(
            format_event(&TimingEvent {
                kind: TimingEventKind::Query,
                name: "body_check.profile.function.check_block[ModuleId(0)]".to_string(),
                data: TimingEventData::Measurement(TimingMeasurement {
                    total: Duration::from_millis(9),
                    max: Duration::from_millis(4),
                    count: 3,
                }),
            }),
            "query timing body_check.profile.function.check_block[ModuleId(0)]: total=0.009s count=3 max=0.004s"
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
        assert!(active_timing_collector().is_none());
    }

    #[test]
    fn collector_aggregates_without_trace_events_by_default() {
        let mut collector = TimingCollector::new(false);
        collector.emit_measurement(
            TimingEventKind::Query,
            "checked_module".to_string(),
            TimingMeasurement::single(Duration::from_millis(2)),
        );
        collector.emit_measurement(
            TimingEventKind::Query,
            "checked_module".to_string(),
            TimingMeasurement::single(Duration::from_millis(5)),
        );
        collector.emit_note(
            TimingEventKind::Query,
            "checked_module".to_string(),
            "items=4".to_string(),
        );

        let (report, trace_events) = collector.drain();
        assert!(trace_events.is_none());
        assert_eq!(
            report.entries,
            vec![TimingReportEntry {
                kind: TimingEventKind::Query,
                name: "checked_module".to_string(),
                count: 2,
                total: Duration::from_millis(7),
                max: Duration::from_millis(5),
            }]
        );
    }

    #[test]
    fn collector_can_keep_trace_events_when_requested() {
        let mut collector = TimingCollector::new(true);
        collector.emit_measurement(
            TimingEventKind::Stage,
            "check".to_string(),
            TimingMeasurement::single(Duration::from_millis(3)),
        );
        collector.emit_note(
            TimingEventKind::Query,
            "body_check".to_string(),
            "items=4".to_string(),
        );

        let (report, trace_events) = collector.drain();
        assert_eq!(report.entries.len(), 1);
        let trace_events = trace_events.expect("trace events should be retained");
        assert_eq!(trace_events.len(), 2);
    }

    #[test]
    fn collector_captures_events_from_worker_threads() {
        collect_to_stderr(|| {
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    emit_query_timing("worker_query", Duration::from_millis(2));
                });
            });
        });
        assert!(active_timing_collector().is_none());
    }

    #[test]
    fn report_aggregates_duration_events_by_kind_and_name() {
        let report = TimingReport::from_events(&[
            TimingEvent {
                kind: TimingEventKind::Query,
                name: "checked_module".to_string(),
                data: TimingEventData::Measurement(TimingMeasurement::single(
                    Duration::from_millis(2),
                )),
            },
            TimingEvent {
                kind: TimingEventKind::Query,
                name: "checked_module".to_string(),
                data: TimingEventData::Measurement(TimingMeasurement::single(
                    Duration::from_millis(5),
                )),
            },
            TimingEvent {
                kind: TimingEventKind::Stage,
                name: "check".to_string(),
                data: TimingEventData::Measurement(TimingMeasurement::single(
                    Duration::from_millis(3),
                )),
            },
            TimingEvent {
                kind: TimingEventKind::Query,
                name: "checked_module".to_string(),
                data: TimingEventData::Note("items=4".to_string()),
            },
        ]);

        assert_eq!(
            report.entries,
            vec![
                TimingReportEntry {
                    kind: TimingEventKind::Query,
                    name: "checked_module".to_string(),
                    count: 2,
                    total: Duration::from_millis(7),
                    max: Duration::from_millis(5),
                },
                TimingReportEntry {
                    kind: TimingEventKind::Stage,
                    name: "check".to_string(),
                    count: 1,
                    total: Duration::from_millis(3),
                    max: Duration::from_millis(3),
                },
            ]
        );
    }

    #[test]
    fn formats_report_entries() {
        assert_eq!(
            format_report_entry(&TimingReportEntry {
                kind: TimingEventKind::Query,
                name: "checked_module".to_string(),
                count: 2,
                total: Duration::from_millis(7),
                max: Duration::from_millis(5),
            }),
            "timing summary query checked_module: total=0.007s count=2 max=0.005s"
        );
    }

    #[test]
    fn accumulator_records_repeated_scopes() {
        let mut accumulator = TimingAccumulator::default();
        accumulator.time("scope", || {});
        accumulator.time("scope", || {});

        let entry = accumulator.entries.get("scope").expect("missing scope");
        assert_eq!(entry.count, 2);
    }
}
