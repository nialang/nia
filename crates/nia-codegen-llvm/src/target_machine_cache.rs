// SPDX-License-Identifier: GPL-3.0-or-later
//! Reuses an LLVM target machine on the worker that owns it.

#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;

use nia_llvm::{
    LlvmResult, OptimizationLevel,
    target::{TargetMachine, TargetMachineIdentity},
};

struct CachedTargetMachine {
    identity: TargetMachineIdentity,
    optimization: OptimizationLevel,
    machine: TargetMachine,
}

thread_local! {
    // LLVM target machines stay on one thread: the LLVM wrapper deliberately
    // does not make TargetMachine Send or Sync.
    static TARGET_MACHINE: RefCell<Option<CachedTargetMachine>> = const { RefCell::new(None) };
    #[cfg(test)]
    static TARGET_MACHINE_CONSTRUCTIONS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn with_target_machine<T>(
    identity: &TargetMachineIdentity,
    optimization: OptimizationLevel,
    timings: nia_timing::TimingMode,
    operation: impl FnOnce(&TargetMachine) -> T,
) -> LlvmResult<T> {
    TARGET_MACHINE.with(|cached| {
        let mut cached = cached.borrow_mut();
        let matches = cached
            .as_ref()
            .is_some_and(|entry| entry.identity == *identity && entry.optimization == optimization);
        if matches {
            if timings.enabled() {
                nia_timing::emit_counter("llvm.target_machine_cache_hits", 1);
            }
        } else {
            let machine = nia_timing::time_query(timings, "llvm_codegen.native_target", || {
                TargetMachine::for_identity(identity, optimization)
            })?;
            #[cfg(test)]
            TARGET_MACHINE_CONSTRUCTIONS.with(|count| count.set(count.get() + 1));
            if timings.enabled() {
                nia_timing::emit_counter("llvm.target_machine_cache_misses", 1);
            }
            *cached = Some(CachedTargetMachine {
                identity: identity.clone(),
                optimization,
                machine,
            });
        }
        let entry = cached
            .as_ref()
            .expect("target machine cache entry was populated");
        Ok(operation(&entry.machine))
    })
}

#[cfg(test)]
fn target_machine_constructions() -> usize {
    TARGET_MACHINE_CONSTRUCTIONS.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_target_machine_per_thread_and_complete_identity() {
        std::thread::spawn(|| {
            let identity = TargetMachine::cpu_identity("x86_64-unknown-linux-gnu", "generic");
            let initial_constructions = target_machine_constructions();
            let first = with_target_machine(
                &identity,
                OptimizationLevel::None,
                nia_timing::TimingMode::Off,
                |machine| machine.target_triple().expect("read cached target triple"),
            )
            .expect("create thread-local target machine");
            let same = with_target_machine(
                &identity,
                OptimizationLevel::None,
                nia_timing::TimingMode::Off,
                |machine| machine.target_triple().expect("read cached target triple"),
            )
            .expect("reuse thread-local target machine");
            assert_eq!(first, same);
            assert_eq!(target_machine_constructions(), initial_constructions + 1);

            with_target_machine(
                &identity,
                OptimizationLevel::Default,
                nia_timing::TimingMode::Off,
                |machine| machine.target_triple().expect("read cached target triple"),
            )
            .expect("create target machine for a different optimization level");
            assert_eq!(target_machine_constructions(), initial_constructions + 2);

            let mut different_identity = identity;
            different_identity.relocation_model = nia_llvm::target::TargetRelocationModel::Static;
            let other_thread_identity = different_identity.clone();
            std::thread::spawn(move || {
                assert_eq!(target_machine_constructions(), 0);
                with_target_machine(
                    &other_thread_identity,
                    OptimizationLevel::Default,
                    nia_timing::TimingMode::Off,
                    |machine| machine.target_triple().expect("read worker target triple"),
                )
                .expect("create worker-local target machine");
                assert_eq!(target_machine_constructions(), 1);
            })
            .join()
            .expect("secondary target-machine cache test thread");
            assert_eq!(target_machine_constructions(), initial_constructions + 2);
            with_target_machine(
                &different_identity,
                OptimizationLevel::Default,
                nia_timing::TimingMode::Off,
                |machine| machine.target_triple().expect("read cached target triple"),
            )
            .expect("create target machine for a different target identity");
            assert_eq!(target_machine_constructions(), initial_constructions + 3);
        })
        .join()
        .expect("target-machine cache test thread");
    }
}
