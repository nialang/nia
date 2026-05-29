// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    collections::HashMap,
    fmt::Debug,
    hash::Hash,
    sync::{Arc, Condvar, Mutex},
};

pub trait QueryKey<C>: Clone + Debug + Eq + Hash + Send + Sync + 'static {
    type Value: Clone + Send + Sync + 'static;

    fn name() -> &'static str;
    fn execute(&self, db: &QueryDb<C>) -> Self::Value;
}

pub struct QueryDb<C> {
    inner: Arc<QueryDbInner<C>>,
}

struct QueryDbInner<C> {
    context: C,
    caches: Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

struct QuerySlot<V> {
    state: Mutex<QueryState<V>>,
    ready: Condvar,
}

enum QueryState<V> {
    Empty,
    Computing,
    Ready(V),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryFrame {
    pub name: &'static str,
    pub key: String,
}

struct QueryStackGuard;

thread_local! {
    static QUERY_STACK: RefCell<Vec<QueryFrame>> = const { RefCell::new(Vec::new()) };
}

impl<C> QueryDb<C> {
    pub fn new(context: C) -> Self {
        Self {
            inner: Arc::new(QueryDbInner {
                context,
                caches: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn context(&self) -> &C {
        &self.inner.context
    }

    pub fn query<K>(&self, key: K) -> K::Value
    where
        K: QueryKey<C>,
    {
        let slot = self.slot_for(&key);
        loop {
            let mut state = slot.state.lock().expect("query cache lock poisoned");
            match &*state {
                QueryState::Ready(value) => return value.clone(),
                QueryState::Computing => {
                    self.assert_not_recursive::<K>(&key);
                    drop(
                        slot.ready
                            .wait(state)
                            .expect("query cache lock poisoned while waiting"),
                    );
                }
                QueryState::Empty => {
                    *state = QueryState::Computing;
                    drop(state);

                    let _guard = self.enter_query::<K>(&key);
                    let value = key.execute(self);

                    let mut state = slot.state.lock().expect("query cache lock poisoned");
                    *state = QueryState::Ready(value.clone());
                    slot.ready.notify_all();
                    return value;
                }
            }
        }
    }

    pub fn query_many<K>(&self, keys: impl IntoIterator<Item = K>) -> Vec<K::Value>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        let keys = keys.into_iter().collect::<Vec<_>>();
        std::thread::scope(|scope| {
            let handles = keys
                .into_iter()
                .map(|key| {
                    let db = self.clone();
                    scope.spawn(move || db.query(key))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("query worker thread panicked"))
                .collect()
        })
    }

    fn slot_for<K>(&self, key: &K) -> Arc<QuerySlot<K::Value>>
    where
        K: QueryKey<C>,
    {
        let mut caches = self.inner.caches.lock().expect("query cache lock poisoned");
        let cache = caches
            .entry(TypeId::of::<K>())
            .or_insert_with(|| Box::new(Mutex::new(HashMap::<K, Arc<QuerySlot<K::Value>>>::new())))
            .downcast_ref::<Mutex<HashMap<K, Arc<QuerySlot<K::Value>>>>>()
            .expect("query cache type mismatch");
        let mut cache = cache.lock().expect("query cache lock poisoned");
        cache
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(QuerySlot {
                    state: Mutex::new(QueryState::Empty),
                    ready: Condvar::new(),
                })
            })
            .clone()
    }

    fn enter_query<K>(&self, key: &K) -> QueryStackGuard
    where
        K: QueryKey<C>,
    {
        self.assert_not_recursive::<K>(key);
        QUERY_STACK.with(|stack| {
            stack.borrow_mut().push(QueryFrame {
                name: K::name(),
                key: format!("{key:?}"),
            });
        });
        QueryStackGuard
    }

    fn assert_not_recursive<K>(&self, key: &K)
    where
        K: QueryKey<C>,
    {
        let key = format!("{key:?}");
        QUERY_STACK.with(|stack| {
            let stack = stack.borrow();
            if let Some(position) = stack
                .iter()
                .position(|frame| frame.name == K::name() && frame.key == key)
            {
                let mut cycle = stack[position..].to_vec();
                cycle.push(QueryFrame {
                    name: K::name(),
                    key,
                });
                panic!("query cycle detected: {cycle:?}");
            }
        });
    }
}

impl<C> Clone for QueryDb<C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Drop for QueryStackGuard {
    fn drop(&mut self) {
        QUERY_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestContext {
        executions: AtomicUsize,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct Double(usize);

    impl QueryKey<TestContext> for Double {
        type Value = usize;

        fn name() -> &'static str {
            "double"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            self.0 * 2
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct Recursive;

    impl QueryKey<TestContext> for Recursive {
        type Value = usize;

        fn name() -> &'static str {
            "recursive"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.query(Recursive)
        }
    }

    #[test]
    fn memoizes_query_values() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(Double(21)), 42);
        assert_eq!(db.query(Double(21)), 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn executes_query_many_in_key_order() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let values = db.query_many([Double(1), Double(4), Double(3)]);

        assert_eq!(values, vec![2, 8, 6]);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
    }

    #[test]
    #[should_panic(expected = "query cycle detected")]
    fn detects_same_thread_query_cycles() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        db.query(Recursive);
    }
}
