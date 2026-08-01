// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::{Condvar, Mutex};

use crate::ActionResourceClass;

pub(crate) struct ActionResourceBudget {
    capacity: usize,
    available: Mutex<usize>,
    ready: Condvar,
}

pub(crate) struct ActionResourcePermit<'a> {
    budget: &'a ActionResourceBudget,
    weight: usize,
}

impl ActionResourceBudget {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "action resource capacity must be non-zero");
        Self {
            capacity,
            available: Mutex::new(capacity),
            ready: Condvar::new(),
        }
    }

    pub(crate) fn acquire(&self, class: ActionResourceClass) -> ActionResourcePermit<'_> {
        let weight = self.weight(class);
        let mut available = self
            .available
            .lock()
            .expect("action resource budget lock poisoned");
        while *available < weight {
            available = self
                .ready
                .wait(available)
                .expect("action resource budget lock poisoned while waiting");
        }
        *available -= weight;
        ActionResourcePermit {
            budget: self,
            weight,
        }
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
        let mut available = self
            .budget
            .available
            .lock()
            .expect("action resource budget lock poisoned");
        *available = available
            .checked_add(self.weight)
            .expect("action resource budget capacity overflow");
        assert!(
            *available <= self.budget.capacity,
            "action resource budget over-release"
        );
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
        let budget = ActionResourceBudget::new(4);
        assert_eq!(budget.weight(ActionResourceClass::Conservative), 4);
        assert_eq!(budget.weight(ActionResourceClass::Cpu), 1);
        assert_eq!(budget.weight(ActionResourceClass::Io), 1);

        let permit = budget.acquire(ActionResourceClass::Conservative);
        assert_eq!(*budget.available.lock().unwrap(), 0);
        drop(permit);
        assert_eq!(*budget.available.lock().unwrap(), 4);
    }

    #[test]
    fn declared_classes_share_and_return_capacity() {
        let budget = ActionResourceBudget::new(3);
        let cpu = budget.acquire(ActionResourceClass::Cpu);
        let io = budget.acquire(ActionResourceClass::Io);
        assert_eq!(*budget.available.lock().unwrap(), 1);
        drop(cpu);
        assert_eq!(*budget.available.lock().unwrap(), 2);
        drop(io);
        assert_eq!(*budget.available.lock().unwrap(), 3);
    }

    #[test]
    fn conservative_class_waits_for_declared_work_to_settle() {
        let budget = std::sync::Arc::new(ActionResourceBudget::new(2));
        let cpu = budget.acquire(ActionResourceClass::Cpu);
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let worker_budget = std::sync::Arc::clone(&budget);
        let worker = std::thread::spawn(move || {
            attempted_tx.send(()).unwrap();
            let _permit = worker_budget.acquire(ActionResourceClass::Conservative);
            acquired_tx.send(()).unwrap();
        });

        attempted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(acquired_rx.recv_timeout(Duration::from_millis(25)).is_err());
        drop(cpu);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }
}
