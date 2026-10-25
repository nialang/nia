// SPDX-License-Identifier: GPL-3.0-or-later
//! The process allocator of every binary that compiles Nia.
//!
//! The compiler allocates heavily from many threads at once: checking a small
//! program makes over a million allocations, and builds, LLVM lanes, and test
//! harnesses run several compilations in one process. The Windows process heap
//! serializes all of them on one lock, so adding threads made compilation
//! slower; glibc and macOS allocators contend less but still share arenas.
//! mimalloc keeps a heap per thread on every host. It is built with `no_thp`:
//! on Linux, transparent huge pages back each touched arena region with 2 MiB
//! pages, and checking a small program peaked at 350 MiB instead of 266.
//!
//! A binary installs it with [`global_allocator!`] at its crate root, and so do
//! the test targets that compile Nia in-process, so tests measure the
//! allocator users run. With the `counting` feature the allocator is wrapped in
//! `nia_timing::CountingAllocator`, which feeds the allocation counters of
//! `--timings=detail`; the binary must then call [`register_counting`].

#[doc(hidden)]
pub use mimalloc::MiMalloc;

#[cfg(feature = "counting")]
#[doc(hidden)]
pub use nia_timing::CountingAllocator;

/// The allocator a Nia process installs: mimalloc, counted when the `counting`
/// feature is enabled.
#[cfg(not(feature = "counting"))]
pub type Allocator = MiMalloc;

/// The allocator a Nia process installs: mimalloc, counted when the `counting`
/// feature is enabled.
#[cfg(feature = "counting")]
pub type Allocator = CountingAllocator<MiMalloc>;

/// Returns the allocator value for a `#[global_allocator]` static.
pub const fn allocator() -> Allocator {
    #[cfg(not(feature = "counting"))]
    {
        MiMalloc
    }
    #[cfg(feature = "counting")]
    {
        CountingAllocator::new(MiMalloc)
    }
}

/// Reports to `nia-timing` that the installed allocator records counters.
/// Without the `counting` feature there are none, so this does nothing.
pub fn register_counting() {
    #[cfg(feature = "counting")]
    nia_timing::register_allocation_instrumentation();
}

/// Installs the Nia allocator as this binary's global allocator.
#[macro_export]
macro_rules! global_allocator {
    () => {
        #[global_allocator]
        static NIA_GLOBAL_ALLOCATOR: $crate::Allocator = $crate::allocator();
    };
}

#[cfg(test)]
mod tests {
    crate::global_allocator!();

    // A torn or foreign heap would fail here: memory allocated on one thread is
    // freed on another, as query workers and LLVM lanes do.
    #[test]
    fn cross_thread_allocations_round_trip() {
        let values = (0..64)
            .map(|index| std::thread::spawn(move || vec![index as u8; 4096 + index]))
            .map(|worker| worker.join().expect("allocating worker"))
            .collect::<Vec<_>>();
        for (index, value) in values.iter().enumerate() {
            assert_eq!(value.len(), 4096 + index);
            assert!(value.iter().all(|byte| *byte == index as u8));
        }
    }
}
