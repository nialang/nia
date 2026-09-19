// SPDX-License-Identifier: GPL-3.0-or-later
//! Action-level accounting layered on inherited query-session capacity.
//!
//! CPU and I/O declarations reserve one slot. Conservative work reserves the
//! complete action capacity so an unknown or nested tool cannot overlap another
//! ready action. This budget does not replace compiler or LLVM memory limits.

use std::sync::{Condvar, Mutex};

use crate::ActionResourceClass;

pub(crate) struct ActionResourceBudget {
    capacity: usize,
    available: Mutex<usize>,
    failure: Mutex<Option<nia_ice::Ice>>,
    ready: Condvar,
}

pub(crate) struct ActionResourcePermit<'a> {
    budget: &'a ActionResourceBudget,
    weight: usize,
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
            failure: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    pub(crate) fn acquire(
        &self,
        class: ActionResourceClass,
    ) -> nia_ice::IceResult<ActionResourcePermit<'_>> {
        let weight = self.weight(class);
        let mut available = self
            .available
            .lock()
            .map_err(|_| nia_ice::Ice::new("action resource budget lock is poisoned"))?;
        while *available < weight {
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
        Ok(ActionResourcePermit {
            budget: self,
            weight,
        })
    }

    fn weight(&self, class: ActionResourceClass) -> usize {
        match class {
            ActionResourceClass::Conservative => self.capacity,
            ActionResourceClass::Cpu | ActionResourceClass::Io => 1,
        }
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
        assert_eq!(*budget.available.lock().unwrap(), 0);
        drop(permit);
        assert_eq!(*budget.available.lock().unwrap(), 4);
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
            attempted_tx.send(()).unwrap();
            let _permit = worker_budget
                .acquire(ActionResourceClass::Conservative)
                .expect("acquire conservative permit");
            acquired_tx.send(()).unwrap();
        });

        attempted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(acquired_rx.recv_timeout(Duration::from_millis(25)).is_err());
        drop(cpu);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
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
