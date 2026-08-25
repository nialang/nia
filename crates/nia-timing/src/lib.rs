// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler timing, trace-event collection, and optional heap accounting.
//!
//! Measurements are process-wide while collection sessions serialize report
//! ownership. Allocation metrics are available only when a binary installs and
//! registers [`CountingAllocator`] as its global allocator.
use std::{
    alloc::{GlobalAlloc, Layout},
    cell::{Cell, RefCell},
    cmp::Reverse,
    collections::HashMap,
    fmt::Write as _,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::ThreadId,
    time::{Duration, Instant},
};

const TIMING_REPORT_ENTRY_LIMIT: usize = 64;
const THREAD_TIMING_BAG_FLUSH_LIMIT: usize = 2048;

static ALLOCATION_TRACKING_ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATION_INSTRUMENTATION_AVAILABLE: AtomicBool = AtomicBool::new(false);
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static QUERY_VALUE_CLONE_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_WINDOW_LOCK: Mutex<()> = Mutex::new(());
static LIVE_WINDOW_ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE_WINDOW_PEAK_BYTES: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static THREAD_ALLOCATED_BYTES: Cell<u64> = const { Cell::new(0) };
}

/// Global allocator wrapper that records allocation and live-byte counters.
pub struct CountingAllocator<A> {
    inner: A,
}

impl<A> CountingAllocator<A> {
    /// Wraps an allocator without enabling measurement by itself.
    pub const fn new(inner: A) -> Self {
        Self { inner }
    }
}

// SAFETY: every allocation operation is delegated to `A` with the original
// pointer and layout. The wrapper only records successful operations in atomic
// counters and does not allocate while doing so.
unsafe impl<A: GlobalAlloc> GlobalAlloc for CountingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract, which is
        // forwarded unchanged to the wrapped allocator.
        let pointer = unsafe { self.inner.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract,
        // which is forwarded unchanged to the wrapped allocator.
        let pointer = unsafe { self.inner.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation(layout.size());
        // SAFETY: the caller guarantees that `pointer` and `layout` identify a
        // live allocation owned by the wrapped allocator.
        unsafe { self.inner.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract, which
        // is forwarded unchanged to the wrapped allocator.
        let new_pointer = unsafe { self.inner.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            record_reallocation(layout.size(), new_size);
        }
        new_pointer
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AllocationMeasurement {
    allocation_calls: u64,
    deallocation_calls: u64,
    reallocation_calls: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
    live_bytes: u64,
    peak_live_bytes: u64,
    query_value_clone_bytes: u64,
}

/// Instantaneous process-wide live heap counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationLiveSnapshot {
    /// Bytes currently live.
    pub live_bytes: u64,
    /// Highest live-byte count since tracking began.
    pub peak_live_bytes: u64,
}

/// Live heap counters across one exclusive measurement window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationLiveWindowMeasurement {
    /// Live bytes at window start.
    pub start_live_bytes: u64,
    /// Live bytes at window end.
    pub end_live_bytes: u64,
    /// Highest live-byte count observed within the window.
    pub peak_live_bytes: u64,
}

struct AllocationLiveWindow {
    start_live_bytes: u64,
    active: bool,
}

impl AllocationLiveWindow {
    fn start() -> Option<Self> {
        if !ALLOCATION_INSTRUMENTATION_AVAILABLE.load(Ordering::Acquire)
            || !ALLOCATION_TRACKING_ENABLED.load(Ordering::Acquire)
        {
            return None;
        }
        let _lock = LIVE_WINDOW_LOCK
            .lock()
            .expect("allocation live window lock poisoned");
        if LIVE_WINDOW_ACTIVE.load(Ordering::Acquire) {
            return None;
        }
        let start_live_bytes = LIVE_BYTES.load(Ordering::Relaxed);
        LIVE_WINDOW_PEAK_BYTES.store(start_live_bytes, Ordering::Relaxed);
        LIVE_WINDOW_ACTIVE.store(true, Ordering::Release);
        Some(Self {
            start_live_bytes,
            active: true,
        })
    }

    fn finish(mut self) -> AllocationLiveWindowMeasurement {
        let measurement = self.stop();
        self.active = false;
        measurement
    }

    fn stop(&self) -> AllocationLiveWindowMeasurement {
        let _lock = LIVE_WINDOW_LOCK
            .lock()
            .expect("allocation live window lock poisoned");
        LIVE_WINDOW_ACTIVE.store(false, Ordering::Release);
        let end_live_bytes = LIVE_BYTES.load(Ordering::Relaxed);
        AllocationLiveWindowMeasurement {
            start_live_bytes: self.start_live_bytes,
            end_live_bytes,
            peak_live_bytes: LIVE_WINDOW_PEAK_BYTES
                .load(Ordering::Relaxed)
                .max(end_live_bytes),
        }
    }
}

impl Drop for AllocationLiveWindow {
    fn drop(&mut self) {
        if self.active {
            let _ = self.stop();
        }
    }
}

/// Registers that this process installed [`CountingAllocator`] globally.
///
/// A binary must call this once during startup. Detail timing omits allocation
/// counters until the instrumentation has been registered.
pub fn register_allocation_instrumentation() {
    ALLOCATION_INSTRUMENTATION_AVAILABLE.store(true, Ordering::Release);
}

fn start_allocation_tracking() -> bool {
    if !ALLOCATION_INSTRUMENTATION_AVAILABLE.load(Ordering::Acquire) {
        return false;
    }
    ALLOCATION_TRACKING_ENABLED.store(false, Ordering::Release);
    ALLOCATION_CALLS.store(0, Ordering::Relaxed);
    DEALLOCATION_CALLS.store(0, Ordering::Relaxed);
    REALLOCATION_CALLS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
    QUERY_VALUE_CLONE_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
    ALLOCATION_TRACKING_ENABLED.store(true, Ordering::Release);
    true
}

fn finish_allocation_tracking() -> AllocationMeasurement {
    ALLOCATION_TRACKING_ENABLED.store(false, Ordering::Release);
    let live_bytes = LIVE_BYTES.load(Ordering::Relaxed);
    AllocationMeasurement {
        allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
        deallocation_calls: DEALLOCATION_CALLS.load(Ordering::Relaxed),
        reallocation_calls: REALLOCATION_CALLS.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
        live_bytes,
        peak_live_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed).max(live_bytes),
        query_value_clone_bytes: QUERY_VALUE_CLONE_BYTES.load(Ordering::Relaxed),
    }
}

/// Returns live heap counters when registered allocation tracking is active.
pub fn allocation_live_snapshot() -> Option<AllocationLiveSnapshot> {
    (ALLOCATION_INSTRUMENTATION_AVAILABLE.load(Ordering::Acquire)
        && ALLOCATION_TRACKING_ENABLED.load(Ordering::Acquire))
    .then(|| {
        let live_bytes = LIVE_BYTES.load(Ordering::Relaxed);
        AllocationLiveSnapshot {
            live_bytes,
            peak_live_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed).max(live_bytes),
        }
    })
}

/// Measures process-wide live Rust heap growth while `measure` runs.
///
/// Allocations from worker threads are included. At most one live window may
/// be active in the process; an overlapping request still runs `measure` but
/// returns no measurement instead of merging unrelated intervals.
pub fn measure_allocation_live_window<T>(
    measure: impl FnOnce() -> T,
) -> (T, Option<AllocationLiveWindowMeasurement>) {
    let window = AllocationLiveWindow::start();
    let value = measure();
    let measurement = window.map(AllocationLiveWindow::finish);
    (value, measurement)
}

fn record_allocation(size: usize) {
    increase_live_bytes(size as u64);
    if !ALLOCATION_TRACKING_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    record_thread_allocated_bytes(size);
}

fn record_deallocation(size: usize) {
    decrease_live_bytes(size as u64);
    if !ALLOCATION_TRACKING_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    DEALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    DEALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
}

fn record_reallocation(old_size: usize, new_size: usize) {
    if new_size >= old_size {
        increase_live_bytes((new_size - old_size) as u64);
    } else {
        decrease_live_bytes((old_size - new_size) as u64);
    }
    if !ALLOCATION_TRACKING_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    REALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
    DEALLOCATED_BYTES.fetch_add(old_size as u64, Ordering::Relaxed);
    record_thread_allocated_bytes(new_size);
}

fn increase_live_bytes(size: u64) {
    let live = LIVE_BYTES
        .fetch_add(size, Ordering::Relaxed)
        .saturating_add(size);
    if ALLOCATION_TRACKING_ENABLED.load(Ordering::Relaxed) {
        PEAK_LIVE_BYTES.fetch_max(live, Ordering::Relaxed);
    }
    if LIVE_WINDOW_ACTIVE.load(Ordering::Relaxed) {
        LIVE_WINDOW_PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
    }
}

fn decrease_live_bytes(size: u64) {
    let _ = LIVE_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size))
    });
}

fn record_thread_allocated_bytes(size: usize) {
    let _ = THREAD_ALLOCATED_BYTES.try_with(|bytes| {
        bytes.set(bytes.get().wrapping_add(size as u64));
    });
}

fn thread_allocated_bytes() -> u64 {
    THREAD_ALLOCATED_BYTES
        .try_with(Cell::get)
        .unwrap_or_default()
}

/// Runs a query-value clone and attributes its thread-local allocations.
pub fn track_query_value_clone<T>(enabled: bool, clone: impl FnOnce() -> T) -> T {
    if !enabled || !ALLOCATION_TRACKING_ENABLED.load(Ordering::Relaxed) {
        return clone();
    }
    let allocated_before = thread_allocated_bytes();
    let value = clone();
    let allocated = thread_allocated_bytes().wrapping_sub(allocated_before);
    QUERY_VALUE_CLONE_BYTES.fetch_add(allocated, Ordering::Relaxed);
    value
}

/// Amount of compiler timing detail to collect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimingMode {
    #[default]
    /// Disable timing collection.
    Off,
    /// Collect top-level stage summaries.
    Summary,
    /// Collect stage, query, and allocation details.
    Detail,
}

/// Timing collection, tracing, and output-format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimingOptions {
    /// Measurement detail level.
    pub mode: TimingMode,
    /// Trace-event collection mode.
    pub trace: TimingTrace,
    /// Report encoding.
    pub format: TimingFormat,
}

impl TimingOptions {
    /// Creates text timing options with trace events disabled.
    pub fn new(mode: TimingMode) -> Self {
        Self {
            mode,
            trace: TimingTrace::Off,
            format: TimingFormat::Text,
        }
    }

    /// Sets trace-event collection.
    pub fn with_trace(mut self, trace: TimingTrace) -> Self {
        self.trace = trace;
        self
    }

    /// Sets report encoding.
    pub fn with_format(mut self, format: TimingFormat) -> Self {
        self.format = format;
        self
    }
}

/// Whether individual timing events are retained for tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimingTrace {
    #[default]
    /// Do not retain trace events.
    Off,
    /// Retain ordered timing events.
    Events,
}

/// Encoding used for emitted timing reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimingFormat {
    #[default]
    /// Human-readable text report.
    Text,
    /// Machine-readable JSON report.
    Json,
}

#[derive(Debug, Clone, Copy)]
struct ProcessUsage {
    user: Duration,
    system: Duration,
    max_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct ProcessMeasurement {
    wall: Duration,
    user: Option<Duration>,
    system: Option<Duration>,
    max_rss_bytes: Option<u64>,
}

impl ProcessMeasurement {
    fn finish(started_at: Instant, started_usage: Option<ProcessUsage>) -> Self {
        let finished_usage = process_usage();
        Self {
            wall: started_at.elapsed(),
            user: started_usage
                .zip(finished_usage)
                .map(|(start, finish)| finish.user.saturating_sub(start.user)),
            system: started_usage
                .zip(finished_usage)
                .map(|(start, finish)| finish.system.saturating_sub(start.system)),
            max_rss_bytes: finished_usage.and_then(|usage| usage.max_rss_bytes),
        }
    }

    fn cpu_utilization_percent(self) -> Option<f64> {
        let cpu = self.user? + self.system?;
        let wall = self.wall.as_secs_f64();
        (wall > 0.0).then(|| cpu.as_secs_f64() * 100.0 / wall)
    }
}

#[cfg(unix)]
fn process_usage() -> Option<ProcessUsage> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` initializes the pointed-to `rusage` on success, and
    // the pointer is valid for the duration of the call.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    // SAFETY: a successful `getrusage` call initialized `usage` above.
    let usage = unsafe { usage.assume_init() };
    Some(ProcessUsage {
        user: timeval_duration(usage.ru_utime),
        system: timeval_duration(usage.ru_stime),
        max_rss_bytes: max_rss_bytes(usage.ru_maxrss),
    })
}

#[cfg(unix)]
fn timeval_duration(value: libc::timeval) -> Duration {
    let seconds = u64::try_from(value.tv_sec).unwrap_or_default();
    let micros = u32::try_from(value.tv_usec)
        .unwrap_or_default()
        .min(999_999);
    Duration::new(seconds, micros * 1_000)
}

#[cfg(all(unix, any(target_os = "macos", target_os = "ios")))]
fn max_rss_bytes(value: libc::c_long) -> Option<u64> {
    u64::try_from(value).ok()
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn max_rss_bytes(value: libc::c_long) -> Option<u64> {
    u64::try_from(value).ok()?.checked_mul(1024)
}

#[cfg(not(unix))]
fn process_usage() -> Option<ProcessUsage> {
    None
}

impl TimingMode {
    /// Reports whether any timing is enabled.
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Reports whether query/allocation detail is enabled.
    pub fn detail(self) -> bool {
        matches!(self, Self::Detail)
    }
}

/// Required detail level for one timed stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingLevel {
    /// Available in summary and detail modes.
    Summary,
    /// Available only in detail mode.
    Detail,
}

impl TimingMode {
    /// Reports whether this mode includes `level`.
    pub fn includes(self, level: TimingLevel) -> bool {
        match level {
            TimingLevel::Summary => self.enabled(),
            TimingLevel::Detail => self.detail(),
        }
    }
}

/// Times and emits a compiler stage when `mode` includes `level`.
pub fn time_stage<T>(mode: TimingMode, level: TimingLevel, name: &str, f: impl FnOnce() -> T) -> T {
    if !mode.includes(level) {
        return f();
    }
    let start = Instant::now();
    let result = f();
    emit_timing(name, start.elapsed());
    result
}

/// Times and emits one query in detail mode.
pub fn time_query<T>(mode: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    if !mode.detail() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    emit_query_timing(name, start.elapsed());
    result
}

/// Times and emits one query when an explicit detail flag is enabled.
pub fn time_detail<T>(enabled: bool, name: &str, f: impl FnOnce() -> T) -> T {
    if !enabled {
        return f();
    }
    let start = Instant::now();
    let result = f();
    emit_query_timing(name, start.elapsed());
    result
}

/// Times a query in detail mode and emits it only at or above `threshold`.
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

/// Emits a single stage timing event or prints it without an active collector.
pub fn emit_timing(name: impl Into<String>, elapsed: Duration) {
    emit(
        TimingEventKind::Stage,
        name.into(),
        TimingMeasurement::single(elapsed),
    );
}

/// Emits a single query timing event.
pub fn emit_query_timing(name: impl Into<String>, elapsed: Duration) {
    emit(
        TimingEventKind::Query,
        name.into(),
        TimingMeasurement::single(elapsed),
    );
}

/// Emits an aggregated query measurement.
pub fn emit_query_measurement(name: impl Into<String>, measurement: TimingMeasurement) {
    emit(TimingEventKind::Query, name.into(), measurement);
}

/// Emits a textual note associated with query timing.
pub fn emit_query_note(name: impl Into<String>, detail: impl Into<String>) {
    emit_note(TimingEventKind::Query, name.into(), detail.into());
}

/// Emits a named integer counter.
pub fn emit_counter(name: impl Into<String>, value: u64) {
    let event = TimingEvent {
        kind: TimingEventKind::Counter,
        name: name.into(),
        data: TimingEventData::Counter(value),
    };
    match record_timing_event(event) {
        TimingRecord::Recorded | TimingRecord::Discarded => {}
        TimingRecord::Unscoped(event) => print_event(&event),
    }
}

/// Aggregated duration statistics for repeated work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingMeasurement {
    /// Sum of all observed durations.
    pub total: Duration,
    /// Longest observed duration.
    pub max: Duration,
    /// Number of observations.
    pub count: usize,
}

impl TimingMeasurement {
    /// Creates a one-observation measurement.
    pub fn single(elapsed: Duration) -> Self {
        Self {
            total: elapsed,
            max: elapsed,
            count: 1,
        }
    }
}

/// Per-name accumulator for repeated query substeps.
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
    /// Times `f` and adds the observation under a static name.
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

    /// Reports whether no observations have been accumulated.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Emits accumulated entries as query measurements in descending total time.
    pub fn emit_query_timings(&self, name_suffix: impl Fn(&'static str) -> String) {
        let mut entries = self.entries.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(name, entry)| (Reverse(entry.total), **name));
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

/// Collects timing events while `f` runs and writes the final report to stderr.
///
/// Process-wide collectors serialize across threads; re-entry on the owning
/// thread executes without starting a nested report.
pub fn collect_to_stderr<T>(options: TimingOptions, f: impl FnOnce() -> T) -> T {
    let session = {
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
        let session = Arc::new(TimingSession::new(
            options.mode,
            options.trace,
            options.format,
        ));
        session.start_allocation_tracking(options.mode.detail());
        state.active = Some(ActiveTimingCollector {
            owner: current_thread,
            session: Arc::clone(&session),
        });
        session
    };

    struct TimingCollectionGuard {
        session: Arc<TimingSession>,
    }

    impl Drop for TimingCollectionGuard {
        fn drop(&mut self) {
            let allocation = self.session.finish_allocation_tracking();
            self.session.finish();
            flush_local_timing_bag_for_session(self.session.id);

            let (state, finished) = timing_collector_state();
            let mut state = state.lock().expect("timing collector state poisoned");
            let Some(current) = state.active.as_ref() else {
                return;
            };
            if !Arc::ptr_eq(&current.session, &self.session) {
                return;
            }
            state.active = None;
            finished.notify_all();
            drop(state);

            if !self.session.collects() {
                return;
            }

            let (mut report, trace_events) = self
                .session
                .collector
                .lock()
                .expect("timing collector poisoned")
                .drain();
            if let Some(allocation) = allocation {
                report.add_allocation_counters(allocation);
            }
            if let Some(events) = trace_events {
                for event in events {
                    print_event(&event);
                }
            }
            print_report(
                &report,
                self.session.format,
                ProcessMeasurement::finish(self.session.started_at, self.session.started_usage),
            );
        }
    }

    let _guard = TimingCollectionGuard { session };
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
    Counter(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TimingEventKind {
    Stage,
    Query,
    Counter,
}

#[derive(Debug, Default)]
struct TimingCollectorState {
    active: Option<ActiveTimingCollector>,
}

#[derive(Debug)]
struct ActiveTimingCollector {
    owner: ThreadId,
    session: Arc<TimingSession>,
}

fn timing_collector_state() -> &'static (Mutex<TimingCollectorState>, Condvar) {
    static COLLECTOR: OnceLock<(Mutex<TimingCollectorState>, Condvar)> = OnceLock::new();
    COLLECTOR.get_or_init(|| (Mutex::new(TimingCollectorState::default()), Condvar::new()))
}

// Shared collection state for one `collect_to_stderr` scope. Hot event recording
// stays in `ThreadTimingBag`; this sink is touched only when a thread flushes.
#[derive(Debug)]
struct TimingSession {
    id: u64,
    mode: TimingMode,
    trace: TimingTrace,
    format: TimingFormat,
    started_at: Instant,
    started_usage: Option<ProcessUsage>,
    allocation_tracking: AtomicBool,
    active: AtomicBool,
    collector: Mutex<TimingCollector>,
}

impl TimingSession {
    fn new(mode: TimingMode, trace: TimingTrace, format: TimingFormat) -> Self {
        Self {
            id: next_timing_session_id(),
            mode,
            trace,
            format,
            started_at: Instant::now(),
            started_usage: process_usage(),
            allocation_tracking: AtomicBool::new(false),
            active: AtomicBool::new(true),
            collector: Mutex::new(TimingCollector::new(trace)),
        }
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Reports whether this scope retains events and produces a report.
    ///
    /// A scope opened with `TimingMode::Off` still absorbs events so that no
    /// individual emitter has to re-check the mode, but it keeps nothing.
    fn collects(&self) -> bool {
        self.mode.enabled()
    }

    fn start_allocation_tracking(&self, enabled: bool) {
        if !enabled || !start_allocation_tracking() {
            return;
        }
        self.allocation_tracking.store(true, Ordering::Release);
    }

    fn finish_allocation_tracking(&self) -> Option<AllocationMeasurement> {
        self.allocation_tracking
            .swap(false, Ordering::AcqRel)
            .then(finish_allocation_tracking)
    }

    fn finish(&self) {
        self.active.store(false, Ordering::Release);
    }

    fn merge(&self, collector: TimingCollector) {
        self.collector
            .lock()
            .expect("timing collector poisoned")
            .merge(collector);
    }
}

fn next_timing_session_id() -> u64 {
    static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    assert_ne!(id, u64::MAX, "timing session id overflowed");
    id
}

fn active_timing_session() -> Option<Arc<TimingSession>> {
    timing_collector_state()
        .0
        .lock()
        .expect("timing collector state poisoned")
        .active
        .as_ref()
        .map(|active| Arc::clone(&active.session))
}

fn emit(kind: TimingEventKind, name: String, measurement: TimingMeasurement) {
    let event = TimingEvent {
        kind,
        name,
        data: TimingEventData::Measurement(measurement),
    };
    match record_timing_event(event) {
        TimingRecord::Recorded | TimingRecord::Discarded => {}
        TimingRecord::Unscoped(event) => print_event(&event),
    }
}

fn emit_note(kind: TimingEventKind, name: String, detail: String) {
    let event = TimingEvent {
        kind,
        name,
        data: TimingEventData::Note(detail),
    };
    match record_timing_event(event) {
        TimingRecord::Recorded | TimingRecord::Discarded => {}
        TimingRecord::Unscoped(event) => print_event(&event),
    }
}

thread_local! {
    static LOCAL_TIMING_BAG: RefCell<Option<ThreadTimingBag>> = const { RefCell::new(None) };
}

/// Outcome of offering one event to the current collection scope.
///
/// `Discarded` and `Unscoped` are deliberately distinct: a scope opened with
/// `TimingMode::Off` owns the decision to produce no output, while an event
/// emitted with no scope at all has no owner and falls back to direct printing.
enum TimingRecord {
    /// The active collecting scope buffered the event.
    Recorded,
    /// An active scope is not collecting, so the event is dropped silently.
    Discarded,
    /// No collection scope is active; the caller decides what to do.
    Unscoped(TimingEvent),
}

fn record_timing_event(event: TimingEvent) -> TimingRecord {
    LOCAL_TIMING_BAG.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(bag) = slot.as_mut() {
            if bag.session.is_active() {
                bag.record(event);
                return TimingRecord::Recorded;
            }
            if let Some(mut stale_bag) = slot.take() {
                stale_bag.flush();
            }
        }

        let Some(session) = active_timing_session() else {
            return TimingRecord::Unscoped(event);
        };
        if !session.is_active() {
            return TimingRecord::Unscoped(event);
        }
        if !session.collects() {
            return TimingRecord::Discarded;
        }
        let mut bag = ThreadTimingBag::new(session);
        bag.record(event);
        *slot = Some(bag);
        TimingRecord::Recorded
    })
}

fn flush_local_timing_bag_for_session(session_id: u64) {
    LOCAL_TIMING_BAG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let should_flush = slot
            .as_ref()
            .is_some_and(|bag| bag.session.id == session_id);
        if !should_flush {
            return;
        }
        if let Some(mut bag) = slot.take() {
            bag.flush();
        }
    });
}

// Per-thread hot-path buffer. Once a thread has attached to a session, events only
// touch this TLS bag until it flushes or the collection scope ends.
#[derive(Debug)]
struct ThreadTimingBag {
    session: Arc<TimingSession>,
    collector: TimingCollector,
    pending_events: usize,
}

impl ThreadTimingBag {
    fn new(session: Arc<TimingSession>) -> Self {
        let trace = session.trace;
        Self {
            session,
            collector: TimingCollector::new(trace),
            pending_events: 0,
        }
    }

    fn record(&mut self, event: TimingEvent) {
        self.collector.record(event);
        self.pending_events += 1;
        if self.pending_events >= THREAD_TIMING_BAG_FLUSH_LIMIT {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.pending_events == 0 {
            return;
        }
        let collector = std::mem::replace(
            &mut self.collector,
            TimingCollector::new(self.session.trace),
        );
        self.pending_events = 0;
        self.session.merge(collector);
    }
}

impl Drop for ThreadTimingBag {
    fn drop(&mut self) {
        self.flush();
    }
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
        (TimingEventKind::Counter, TimingEventData::Counter(value)) => {
            format!("timing counter {}: {value}", event.name)
        }
        _ => unreachable!("timing event kind and data must agree"),
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
    counters: Vec<TimingCounter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimingReportEntry {
    kind: TimingEventKind,
    name: String,
    count: usize,
    total: Duration,
    max: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimingCounter {
    name: String,
    value: u64,
}

#[derive(Debug)]
struct TimingCollector {
    report: TimingReportBuilder,
    counters: HashMap<String, u64>,
    trace_events: Option<Vec<TimingEvent>>,
}

impl TimingCollector {
    fn new(trace: TimingTrace) -> Self {
        Self {
            report: TimingReportBuilder::default(),
            counters: HashMap::new(),
            trace_events: matches!(trace, TimingTrace::Events).then(Vec::new),
        }
    }

    fn record(&mut self, event: TimingEvent) {
        match event.data {
            TimingEventData::Measurement(measurement) => {
                self.emit_measurement(event.kind, event.name, measurement);
            }
            TimingEventData::Note(detail) => {
                self.emit_note(event.kind, event.name, detail);
            }
            TimingEventData::Counter(value) => {
                self.emit_counter(event.name, value);
            }
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

    fn emit_counter(&mut self, name: String, value: u64) {
        if let Some(events) = &mut self.trace_events {
            events.push(TimingEvent {
                kind: TimingEventKind::Counter,
                name: name.clone(),
                data: TimingEventData::Counter(value),
            });
        }
        *self.counters.entry(name).or_default() += value;
    }

    fn merge(&mut self, mut other: TimingCollector) {
        self.report.merge(std::mem::take(&mut other.report));
        for (name, value) in other.counters.drain() {
            *self.counters.entry(name).or_default() += value;
        }
        if let (Some(events), Some(other_events)) =
            (&mut self.trace_events, other.trace_events.take())
        {
            events.extend(other_events);
        }
    }

    fn drain(&mut self) -> (TimingReport, Option<Vec<TimingEvent>>) {
        let entries = std::mem::take(&mut self.report).finish_entries();
        let value_clones = entries
            .iter()
            .filter(|entry| {
                entry.name.starts_with("query.clone.cache_hit[")
                    || entry.name.starts_with("query.clone.store[")
            })
            .map(|entry| entry.count as u64)
            .sum::<u64>();
        if value_clones != 0 {
            *self
                .counters
                .entry("query.value_clones".to_string())
                .or_default() += value_clones;
        }
        let mut counters = std::mem::take(&mut self.counters)
            .into_iter()
            .map(|(name, value)| TimingCounter { name, value })
            .collect::<Vec<_>>();
        counters.sort_by(|left, right| left.name.cmp(&right.name));
        (TimingReport { entries, counters }, self.trace_events.take())
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

    fn merge(&mut self, other: TimingReportBuilder) {
        for entry in other.entries_by_key.into_values() {
            self.record(
                entry.kind,
                entry.name,
                TimingMeasurement {
                    total: entry.total,
                    max: entry.max,
                    count: entry.count,
                },
            );
        }
    }

    fn finish_entries(self) -> Vec<TimingReportEntry> {
        let mut entries = self.entries_by_key.into_values().collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .total
                .cmp(&left.total)
                .then_with(|| left.name.cmp(&right.name))
        });
        entries
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
        TimingReport {
            entries: builder.finish_entries(),
            counters: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.counters.is_empty()
    }

    fn add_allocation_counters(&mut self, measurement: AllocationMeasurement) {
        self.counters.extend([
            TimingCounter {
                name: "allocator.alloc_calls".to_string(),
                value: measurement.allocation_calls,
            },
            TimingCounter {
                name: "allocator.allocated_bytes".to_string(),
                value: measurement.allocated_bytes,
            },
            TimingCounter {
                name: "allocator.dealloc_calls".to_string(),
                value: measurement.deallocation_calls,
            },
            TimingCounter {
                name: "allocator.deallocated_bytes".to_string(),
                value: measurement.deallocated_bytes,
            },
            TimingCounter {
                name: "allocator.live_bytes".to_string(),
                value: measurement.live_bytes,
            },
            TimingCounter {
                name: "allocator.peak_live_bytes".to_string(),
                value: measurement.peak_live_bytes,
            },
            TimingCounter {
                name: "allocator.realloc_calls".to_string(),
                value: measurement.reallocation_calls,
            },
            TimingCounter {
                name: "query.value_clone_bytes".to_string(),
                value: measurement.query_value_clone_bytes,
            },
        ]);
        self.counters
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
}

fn print_report(report: &TimingReport, format: TimingFormat, process: ProcessMeasurement) {
    match format {
        TimingFormat::Text => print_text_report(report),
        TimingFormat::Json => eprintln!("{}", format_json_report(report, process)),
    }
}

fn print_text_report(report: &TimingReport) {
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
    for counter in &report.counters {
        eprintln!("timing summary counter {}: {}", counter.name, counter.value);
    }
}

fn format_report_entry(entry: &TimingReportEntry) -> String {
    let prefix = match entry.kind {
        TimingEventKind::Stage => "timing summary stage",
        TimingEventKind::Query => "timing summary query",
        TimingEventKind::Counter => unreachable!("counters are not timing entries"),
    };
    format!(
        "{prefix} {}: total={:.3}s count={} max={:.3}s",
        entry.name,
        entry.total.as_secs_f64(),
        entry.count,
        entry.max.as_secs_f64()
    )
}

fn format_json_report(report: &TimingReport, process: ProcessMeasurement) -> String {
    let mut output = String::new();
    output.push_str("{\"schema_version\":1,\"process\":{");
    push_json_duration(&mut output, "wall_seconds", Some(process.wall));
    output.push(',');
    push_json_duration(&mut output, "user_seconds", process.user);
    output.push(',');
    push_json_duration(&mut output, "system_seconds", process.system);
    output.push_str(",\"max_rss_bytes\":");
    push_json_optional_u64(&mut output, process.max_rss_bytes);
    output.push_str(",\"cpu_utilization_percent\":");
    push_json_optional_f64(&mut output, process.cpu_utilization_percent());
    output.push_str("},\"timings\":[");
    for (index, entry) in report.entries.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str("{\"kind\":");
        push_json_string(
            &mut output,
            match entry.kind {
                TimingEventKind::Stage => "stage",
                TimingEventKind::Query => "query",
                TimingEventKind::Counter => unreachable!("counters are not timing entries"),
            },
        );
        output.push_str(",\"name\":");
        push_json_string(&mut output, &entry.name);
        write!(
            output,
            ",\"count\":{},\"total_seconds\":{:.9},\"max_seconds\":{:.9}}}",
            entry.count,
            entry.total.as_secs_f64(),
            entry.max.as_secs_f64(),
        )
        .expect("writing JSON to a string cannot fail");
    }
    output.push_str("],\"counters\":{");
    for (index, counter) in report.counters.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        push_json_string(&mut output, &counter.name);
        write!(output, ":{}", counter.value).expect("writing JSON to a string cannot fail");
    }
    output.push_str("}}");
    output
}

fn push_json_duration(output: &mut String, name: &str, value: Option<Duration>) {
    push_json_string(output, name);
    output.push(':');
    push_json_optional_f64(output, value.map(|duration| duration.as_secs_f64()));
}

fn push_json_optional_u64(output: &mut String, value: Option<u64>) {
    match value {
        Some(value) => write!(output, "{value}").expect("writing JSON to a string cannot fail"),
        None => output.push_str("null"),
    }
}

fn push_json_optional_f64(output: &mut String, value: Option<f64>) {
    match value {
        Some(value) => write!(output, "{value:.9}").expect("writing JSON to a string cannot fail"),
        None => output.push_str("null"),
    }
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing JSON to a string cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn collector_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("timing collector test lock poisoned")
    }

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
    fn allocation_tracking_records_only_the_active_interval() {
        let _lock = collector_test_lock();
        register_allocation_instrumentation();
        let baseline = LIVE_BYTES.load(Ordering::Relaxed);
        record_allocation(100);
        assert!(start_allocation_tracking());
        record_allocation(16);
        record_allocation(32);
        track_query_value_clone(true, || record_allocation(24));
        record_reallocation(32, 64);
        assert_eq!(
            allocation_live_snapshot(),
            Some(AllocationLiveSnapshot {
                live_bytes: baseline + 204,
                peak_live_bytes: baseline + 204,
            })
        );
        record_deallocation(16);
        let measurement = finish_allocation_tracking();
        record_allocation(200);
        record_deallocation(388);

        assert_eq!(
            measurement,
            AllocationMeasurement {
                allocation_calls: 3,
                deallocation_calls: 1,
                reallocation_calls: 1,
                allocated_bytes: 136,
                deallocated_bytes: 48,
                live_bytes: baseline + 188,
                peak_live_bytes: baseline + 204,
                query_value_clone_bytes: 24,
            }
        );
    }

    #[test]
    fn allocation_live_window_tracks_worker_visible_peak_without_overlapping() {
        let _lock = collector_test_lock();
        register_allocation_instrumentation();
        let baseline = LIVE_BYTES.load(Ordering::Relaxed);
        assert!(start_allocation_tracking());

        let ((), measurement) = measure_allocation_live_window(|| {
            record_allocation(64);
            let ((), nested) = measure_allocation_live_window(|| record_allocation(32));
            assert_eq!(nested, None);
            record_deallocation(24);
        });

        assert_eq!(
            measurement,
            Some(AllocationLiveWindowMeasurement {
                start_live_bytes: baseline,
                end_live_bytes: baseline + 72,
                peak_live_bytes: baseline + 96,
            })
        );
        let _ = finish_allocation_tracking();
        record_deallocation(72);
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
        let _lock = collector_test_lock();
        collect_to_stderr(TimingOptions::default(), || {
            emit_timing("outer", Duration::from_millis(1));
            collect_to_stderr(TimingOptions::default(), || {
                emit_query_timing("inner", Duration::from_millis(2));
            });
        });
        assert!(active_timing_session().is_none());
    }

    #[test]
    fn collector_aggregates_without_trace_events_by_default() {
        let mut collector = TimingCollector::new(TimingTrace::Off);
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
        collector.emit_counter("query.executions".to_string(), 3);
        collector.emit_counter("query.executions".to_string(), 4);

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
        assert_eq!(
            report.counters,
            vec![TimingCounter {
                name: "query.executions".to_string(),
                value: 7,
            }]
        );
    }

    #[test]
    fn collector_can_keep_trace_events_when_requested() {
        let mut collector = TimingCollector::new(TimingTrace::Events);
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
    fn thread_bag_flushes_measurements_and_trace_into_session() {
        let session = Arc::new(TimingSession::new(
            TimingMode::Detail,
            TimingTrace::Events,
            TimingFormat::Text,
        ));
        let mut bag = ThreadTimingBag::new(Arc::clone(&session));
        bag.record(TimingEvent {
            kind: TimingEventKind::Query,
            name: "body_check".to_string(),
            data: TimingEventData::Measurement(TimingMeasurement::single(Duration::from_millis(2))),
        });
        bag.record(TimingEvent {
            kind: TimingEventKind::Query,
            name: "body_check".to_string(),
            data: TimingEventData::Measurement(TimingMeasurement::single(Duration::from_millis(5))),
        });
        bag.record(TimingEvent {
            kind: TimingEventKind::Query,
            name: "body_check".to_string(),
            data: TimingEventData::Note("items=4".to_string()),
        });
        bag.flush();

        let (report, trace_events) = session
            .collector
            .lock()
            .expect("timing collector poisoned")
            .drain();
        assert_eq!(
            report.entries,
            vec![TimingReportEntry {
                kind: TimingEventKind::Query,
                name: "body_check".to_string(),
                count: 2,
                total: Duration::from_millis(7),
                max: Duration::from_millis(5),
            }]
        );
        assert_eq!(
            trace_events.expect("trace events should be retained").len(),
            3
        );
    }

    /// Counters retained by the active scope after flushing this thread's bag.
    fn active_scope_counters() -> Vec<TimingCounter> {
        let session = active_timing_session().expect("a collection scope must be active");
        flush_local_timing_bag_for_session(session.id);
        let (report, _) = session
            .collector
            .lock()
            .expect("timing collector poisoned")
            .drain();
        report.counters
    }

    #[test]
    fn disabled_scope_retains_no_counters() {
        let _lock = collector_test_lock();
        collect_to_stderr(TimingOptions::new(TimingMode::Off), || {
            emit_counter("probe.counter", 1);
            emit_timing("probe.stage", Duration::from_millis(1));
            assert!(
                active_scope_counters().is_empty(),
                "a scope opened with TimingMode::Off must retain nothing"
            );
        });
        assert!(active_timing_session().is_none());
    }

    #[test]
    fn enabled_scope_retains_ungated_counters() {
        let _lock = collector_test_lock();
        collect_to_stderr(TimingOptions::new(TimingMode::Summary), || {
            emit_counter("probe.counter", 7);
            assert_eq!(
                active_scope_counters(),
                vec![TimingCounter {
                    name: "probe.counter".to_string(),
                    value: 7,
                }]
            );
        });
        assert!(active_timing_session().is_none());
    }

    #[test]
    fn disabled_scope_discards_instead_of_falling_back_to_printing() {
        let _lock = collector_test_lock();
        let counter = || TimingEvent {
            kind: TimingEventKind::Counter,
            name: "probe.counter".to_string(),
            data: TimingEventData::Counter(1),
        };
        assert!(
            matches!(record_timing_event(counter()), TimingRecord::Unscoped(_)),
            "with no scope the event has no owner and stays printable"
        );
        collect_to_stderr(TimingOptions::new(TimingMode::Off), || {
            assert!(
                matches!(record_timing_event(counter()), TimingRecord::Discarded),
                "a disabled scope owns the decision to produce no output"
            );
        });
        collect_to_stderr(TimingOptions::new(TimingMode::Summary), || {
            assert!(matches!(
                record_timing_event(counter()),
                TimingRecord::Recorded
            ));
        });
    }

    #[test]
    fn collector_captures_events_from_worker_threads() {
        let _lock = collector_test_lock();
        collect_to_stderr(TimingOptions::default(), || {
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    emit_query_timing("worker_query", Duration::from_millis(2));
                });
            });
        });
        assert!(active_timing_session().is_none());
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
    fn json_report_contains_process_timings_entries_and_counters() {
        let report = TimingReport {
            entries: vec![TimingReportEntry {
                kind: TimingEventKind::Query,
                name: "query\nname".to_string(),
                count: 2,
                total: Duration::from_millis(7),
                max: Duration::from_millis(5),
            }],
            counters: vec![TimingCounter {
                name: "query.executions".to_string(),
                value: 4,
            }],
        };
        let json = format_json_report(
            &report,
            ProcessMeasurement {
                wall: Duration::from_secs(2),
                user: Some(Duration::from_millis(750)),
                system: Some(Duration::from_millis(250)),
                max_rss_bytes: Some(4096),
            },
        );

        assert!(json.starts_with("{\"schema_version\":1,"), "{json}");
        assert!(
            json.contains("\"cpu_utilization_percent\":50.000000000"),
            "{json}"
        );
        assert!(json.contains("\"name\":\"query\\nname\""), "{json}");
        assert!(json.contains("\"query.executions\":4"), "{json}");
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
