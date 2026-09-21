// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional process-wide allocation and live-heap accounting.

use std::{
    alloc::{GlobalAlloc, Layout},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use parking_lot::Mutex;

static ALLOCATION_TRACKING_ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATION_INSTRUMENTATION_AVAILABLE: AtomicBool = AtomicBool::new(false);
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_WINDOW_LOCK: Mutex<()> = Mutex::new(());
static LIVE_WINDOW_ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE_WINDOW_PEAK_BYTES: AtomicU64 = AtomicU64::new(0);

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
pub(super) struct AllocationMeasurement {
    pub(super) allocation_calls: u64,
    pub(super) deallocation_calls: u64,
    pub(super) reallocation_calls: u64,
    pub(super) allocated_bytes: u64,
    pub(super) deallocated_bytes: u64,
    pub(super) live_bytes: u64,
    pub(super) peak_live_bytes: u64,
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
        let _lock = LIVE_WINDOW_LOCK.lock();
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
        let _lock = LIVE_WINDOW_LOCK.lock();
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
pub fn register_allocation_instrumentation() {
    ALLOCATION_INSTRUMENTATION_AVAILABLE.store(true, Ordering::Release);
}

pub(super) fn start_allocation_tracking() -> bool {
    if !ALLOCATION_INSTRUMENTATION_AVAILABLE.load(Ordering::Acquire) {
        return false;
    }
    ALLOCATION_TRACKING_ENABLED.store(false, Ordering::Release);
    ALLOCATION_CALLS.store(0, Ordering::Relaxed);
    DEALLOCATION_CALLS.store(0, Ordering::Relaxed);
    REALLOCATION_CALLS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
    ALLOCATION_TRACKING_ENABLED.store(true, Ordering::Release);
    true
}

pub(super) fn finish_allocation_tracking() -> AllocationMeasurement {
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
pub fn measure_allocation_live_window<T>(
    measure: impl FnOnce() -> T,
) -> (T, Option<AllocationLiveWindowMeasurement>) {
    let window = AllocationLiveWindow::start();
    let value = measure();
    let measurement = window.map(AllocationLiveWindow::finish);
    (value, measurement)
}

pub(super) fn record_allocation(size: usize) {
    increase_live_bytes(size as u64);
    if !ALLOCATION_TRACKING_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
}

pub(super) fn record_deallocation(size: usize) {
    decrease_live_bytes(size as u64);
    if !ALLOCATION_TRACKING_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    DEALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    DEALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
}

pub(super) fn record_reallocation(old_size: usize, new_size: usize) {
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

#[cfg(test)]
pub(super) fn current_live_bytes() -> u64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}
