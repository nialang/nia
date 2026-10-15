// SPDX-License-Identifier: GPL-3.0-or-later
//! Action-level accounting layered on inherited query-session capacity.
//!
//! CPU and I/O declarations reserve one slot. Conservative work reserves the
//! complete action capacity so an unknown or nested tool cannot overlap another
//! ready action. This budget does not replace compiler or LLVM memory limits.

use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
};

use crate::ActionResourceClass;

pub(crate) struct ActionResourceBudget {
    capacity: usize,
    available: Mutex<usize>,
    generation: AtomicU64,
    failure: Mutex<Option<nia_ice::Ice>>,
    ready: Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActionResourceSnapshot {
    pub(crate) generation: u64,
    pub(crate) capacity: usize,
    pub(crate) used: usize,
}

pub(crate) struct ActionResourcePermit<'a> {
    budget: &'a ActionResourceBudget,
    weight: usize,
    waited: std::time::Duration,
}

impl ActionResourceBudget {
    pub(crate) fn new(capacity: usize) -> nia_ice::IceResult<Self> {
        if capacity == 0 {
            return Err(nia_ice::Ice::new(
                "action resource capacity must be non-zero",
            ));
        }
        Ok(Self {
            capacity,
            available: Mutex::new(capacity),
            generation: AtomicU64::new(0),
            failure: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn acquire(
        &self,
        class: ActionResourceClass,
    ) -> nia_ice::IceResult<ActionResourcePermit<'_>> {
        self.acquire_observed(class, || {})
    }

    pub(crate) fn acquire_observed(
        &self,
        class: ActionResourceClass,
        mut on_contention: impl FnMut(),
    ) -> nia_ice::IceResult<ActionResourcePermit<'_>> {
        let weight = self.weight(class);
        let mut available = self
            .available
            .lock()
            .map_err(|_| nia_ice::Ice::new("action resource budget lock is poisoned"))?;
        let mut started = None;
        while *available < weight {
            if started.is_none() {
                started = Some(std::time::Instant::now());
                on_contention();
            }
            if let Some(failure) = self
                .failure
                .lock()
                .map_err(|_| nia_ice::Ice::new("action resource failure lock is poisoned"))?
                .clone()
            {
                return Err(failure);
            }
            available = self
                .ready
                .wait(available)
                .map_err(|_| nia_ice::Ice::new("action resource budget lock is poisoned"))?;
        }
        *available -= weight;
        self.generation.fetch_add(1, Ordering::Release);
        Ok(ActionResourcePermit {
            budget: self,
            weight,
            waited: started.map_or(std::time::Duration::ZERO, |started| started.elapsed()),
        })
    }

    pub(crate) fn snapshot(&self) -> nia_ice::IceResult<ActionResourceSnapshot> {
        let available = self
            .available
            .lock()
            .map_err(|_| nia_ice::Ice::new("action resource budget lock is poisoned"))?;
        Ok(ActionResourceSnapshot {
            generation: self.generation.load(Ordering::Acquire),
            capacity: self.capacity,
            used: self.capacity - *available,
        })
    }

    fn weight(&self, class: ActionResourceClass) -> usize {
        match class {
            ActionResourceClass::Conservative => self.capacity,
            ActionResourceClass::Cpu | ActionResourceClass::Io => 1,
        }
    }
}

impl ActionResourcePermit<'_> {
    pub(crate) fn wait_duration(&self) -> std::time::Duration {
        self.waited
    }
}

impl Drop for ActionResourcePermit<'_> {
    fn drop(&mut self) {
        let Ok(mut available) = self.budget.available.lock() else {
            if let Ok(mut failure) = self.budget.failure.lock() {
                *failure = Some(nia_ice::Ice::new("action resource budget lock is poisoned"));
            }
            self.budget.ready.notify_all();
            return;
        };
        let Some(next) = available.checked_add(self.weight) else {
            if let Ok(mut failure) = self.budget.failure.lock() {
                *failure = Some(nia_ice::Ice::new(
                    "action resource budget capacity overflow",
                ));
            }
            drop(available);
            self.budget.ready.notify_all();
            return;
        };
        if next > self.budget.capacity {
            if let Ok(mut failure) = self.budget.failure.lock() {
                *failure = Some(nia_ice::Ice::new("action resource budget over-release"));
            }
        } else {
            *available = next;
            self.budget.generation.fetch_add(1, Ordering::Release);
        }
        drop(available);
        self.budget.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn conservative_class_reserves_complete_inherited_capacity() {
        let budget = ActionResourceBudget::new(4).expect("create action resource budget");
        assert_eq!(budget.weight(ActionResourceClass::Conservative), 4);
        assert_eq!(budget.weight(ActionResourceClass::Cpu), 1);
        assert_eq!(budget.weight(ActionResourceClass::Io), 1);

        let permit = budget
            .acquire(ActionResourceClass::Conservative)
            .expect("acquire conservative permit");
        assert_eq!(permit.wait_duration(), Duration::ZERO);
        assert_eq!(*budget.available.lock().unwrap(), 0);
        assert_eq!(
            budget.snapshot().expect("occupied resource snapshot"),
            ActionResourceSnapshot {
                generation: 1,
                capacity: 4,
                used: 4,
            }
        );
        drop(permit);
        assert_eq!(*budget.available.lock().unwrap(), 4);
        let snapshot = budget.snapshot().expect("resource snapshot");
        assert_eq!(snapshot.capacity, 4);
        assert_eq!(snapshot.used, 0);
        assert_eq!(snapshot.generation, 2);
    }

    #[test]
    fn declared_classes_share_and_return_capacity() {
        let budget = ActionResourceBudget::new(3).expect("create action resource budget");
        let cpu = budget
            .acquire(ActionResourceClass::Cpu)
            .expect("acquire cpu permit");
        let io = budget
            .acquire(ActionResourceClass::Io)
            .expect("acquire io permit");
        assert_eq!(*budget.available.lock().unwrap(), 1);
        drop(cpu);
        assert_eq!(*budget.available.lock().unwrap(), 2);
        drop(io);
        assert_eq!(*budget.available.lock().unwrap(), 3);
    }

    #[test]
    fn conservative_class_waits_for_declared_work_to_settle() {
        let budget = std::sync::Arc::new(
            ActionResourceBudget::new(2).expect("create action resource budget"),
        );
        let cpu = budget
            .acquire(ActionResourceClass::Cpu)
            .expect("acquire cpu permit");
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let worker_budget = std::sync::Arc::clone(&budget);
        let worker = std::thread::spawn(move || {
            let _permit = worker_budget
                .acquire_observed(ActionResourceClass::Conservative, || {
                    attempted_tx.send(()).unwrap();
                })
                .expect("acquire conservative permit");
            assert!(!_permit.wait_duration().is_zero());
            acquired_tx.send(()).unwrap();
        });

        attempted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(acquired_rx.recv_timeout(Duration::from_millis(25)).is_err());
        drop(cpu);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert!(
            attempted_rx.try_recv().is_err(),
            "contention is entered only once"
        );
    }

    #[test]
    fn poisoned_budget_lock_is_reported_as_ice() {
        let budget = std::sync::Arc::new(
            ActionResourceBudget::new(2).expect("create action resource budget"),
        );
        let worker_budget = std::sync::Arc::clone(&budget);
        let worker = std::thread::spawn(move || {
            let _available = worker_budget.available.lock().unwrap();
            panic!("poison resource budget for recovery test");
        });
        assert!(worker.join().is_err());

        let error = match budget.acquire(ActionResourceClass::Cpu) {
            Ok(_) => panic!("poisoned budget must be reported"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("poisoned"));
        assert_eq!(*budget.available.lock().unwrap_err().into_inner(), 2);
    }
}
