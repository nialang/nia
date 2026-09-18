// SPDX-License-Identifier: GPL-3.0-or-later
//! Dependency graph maintenance and thread-local query execution context.
//!
//! The graph keeps forward and reverse edges in lockstep. Query-stack snapshots
//! carry dependency recording across executor threads without sharing mutable
//! stack frames between tasks.

use super::*;

impl QueryDependencyGraph {
    pub(super) fn replace_dependencies_from(
        &mut self,
        from: QueryNodeId,
        targets: FastHashSet<QueryNodeId>,
    ) {
        // Replace both directions as one graph operation. Leaving stale reverse edges would make
        // later invalidation reach queries that no longer read the changed node.
        self.remove_dependencies_from(from);
        if targets.is_empty() {
            return;
        }
        for target in &targets {
            self.reverse.entry(*target).or_default().insert(from);
        }
        self.forward.insert(from, targets);
    }

    pub(super) fn dependencies(
        &self,
        db_id: QueryDbId,
        session: &QuerySession,
    ) -> QueryResult<Vec<QueryDependency>> {
        let mut dependencies = Vec::new();
        for (from, targets) in self.forward.iter().filter(|(from, _)| from.db_id == db_id) {
            let from_frame = session.frame(*from)?;
            for to in targets {
                dependencies.push(QueryDependency {
                    from: from_frame.clone(),
                    to: session.frame(*to)?,
                });
            }
        }
        dependencies.sort_by(|left, right| {
            (
                left.from.name,
                left.from.key.as_str(),
                left.to.name,
                left.to.key.as_str(),
            )
                .cmp(&(
                    right.from.name,
                    right.from.key.as_str(),
                    right.to.name,
                    right.to.key.as_str(),
                ))
        });
        Ok(dependencies)
    }

    pub(super) fn collect_dependents(&self, root: QueryNodeId) -> Vec<QueryNodeId> {
        let mut seen = FastHashSet::default();
        let mut queue = vec![root];
        let mut invalidated = Vec::new();

        while let Some(identity) = queue.pop() {
            if !seen.insert(identity) {
                continue;
            }
            invalidated.push(identity);
            queue.extend(
                self.reverse
                    .get(&identity)
                    .into_iter()
                    .flat_map(|dependents| dependents.iter().copied()),
            );
        }

        invalidated
    }

    pub(super) fn remove_dependencies_from(&mut self, from: QueryNodeId) {
        if let Some(targets) = self.forward.remove(&from) {
            for target in targets {
                if let Some(dependents) = self.reverse.get_mut(&target) {
                    dependents.remove(&from);
                    if dependents.is_empty() {
                        self.reverse.remove(&target);
                    }
                }
            }
        }
    }

    pub(super) fn remove_node(&mut self, node: QueryNodeId) {
        self.remove_dependencies_from(node);
        if let Some(dependents) = self.reverse.remove(&node) {
            for dependent in dependents {
                if let Some(targets) = self.forward.get_mut(&dependent) {
                    targets.remove(&node);
                    if targets.is_empty() {
                        self.forward.remove(&dependent);
                    }
                }
            }
        }
    }

    pub(super) fn require_only_predecessor(
        &self,
        predecessor: QueryNodeId,
        current: QueryNodeId,
    ) -> QueryResult<()> {
        let Some(dependents) = self.reverse.get(&predecessor) else {
            return Err(QueryError::internal(
                "sealed predecessor has no current dependent",
            ));
        };
        if dependents.len() != 1 {
            return Err(QueryError::internal(
                "sealed predecessor does not have exactly one dependent",
            ));
        }
        if !dependents.contains(&current) {
            return Err(QueryError::internal(
                "sealed predecessor feeds a different query",
            ));
        }
        Ok(())
    }
}

pub(super) fn query_frame<C, K>(key: &K) -> QueryFrame
where
    K: QueryKey<C>,
{
    QueryFrame {
        name: K::name(),
        key: format!("{key:?}"),
        description: key.description(),
    }
}

fn retired_query_frame(node_id: QueryNodeId) -> QueryFrame {
    let key = format!("{}:{}", node_id.db_id.0, node_id.index);
    QueryFrame {
        name: "<retired-query>",
        description: format!("retired query node {key}"),
        key,
    }
}

pub(super) fn query_slot_identity<C, K>(key: &K) -> QuerySlotIdentity
where
    K: QueryKey<C>,
{
    QuerySlotIdentity {
        frame: query_frame::<C, K>(key),
    }
}

impl<C> ErasedQueryDatabase for QueryDbRegistration<C>
where
    C: Send + Sync + 'static,
{
    fn frame(&self, node_id: QueryNodeId) -> Option<QueryFrame> {
        let inner = self.inner.upgrade()?;
        inner
            .slots
            .lock()
            .get(inner.id, node_id)
            .map(|record| record.identity.frame())
    }

    fn slot(&self, node_id: QueryNodeId) -> Option<Arc<dyn ErasedQuerySlot>> {
        let inner = self.inner.upgrade()?;
        inner
            .slots
            .lock()
            .get(inner.id, node_id)
            .map(|record| Arc::clone(&record.slot))
    }

    fn ensure(&self, node_id: QueryNodeId) -> QueryResult<()> {
        let Some(inner) = self.inner.upgrade() else {
            return Err(QueryError::internal(
                "query dependency database was dropped",
            ));
        };
        let ensure = {
            let slots = inner.slots.lock();
            let Some(record) = slots.get(inner.id, node_id) else {
                return Err(QueryError::InvalidInput {
                    query: retired_query_frame(node_id),
                    message: "query dependency was retired".into(),
                });
            };
            Arc::clone(&record.ensure)
        };
        ensure(&QueryDb { inner })
    }

    fn invalidate_scope(&self, retain: &dyn Fn(&QueryFrame) -> bool) -> QueryResult<()> {
        let Some(inner) = self.inner.upgrade() else {
            return Ok(());
        };
        QueryDb { inner }.retire_scope_during_retirement(retain)
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
        if self.active {
            QUERY_STACK.with(|stack| {
                stack.borrow_mut().pop();
            });
        }
    }
}

impl QueryStackGuard {
    pub(super) fn discard(&mut self) {
        if self.active {
            QUERY_STACK.with(|stack| {
                stack.borrow_mut().pop();
            });
            self.active = false;
        }
    }

    pub(super) fn take_dependencies(&mut self) -> RecordedDependencies {
        if !self.active {
            return RecordedDependencies::default();
        }
        self.active = false;
        QUERY_STACK.with(|stack| {
            stack
                .borrow_mut()
                .pop()
                .map(|entry| RecordedDependencies {
                    nodes: entry.dependencies,
                    fingerprints: entry.dependency_fingerprints,
                })
                .unwrap_or_default()
        })
    }
}

impl Drop for QueryStackInstallGuard {
    fn drop(&mut self) {
        QUERY_STACK.with(|stack| {
            *stack.borrow_mut() = std::mem::take(&mut self.previous);
        });
    }
}

pub(super) fn current_query_stack() -> Vec<QueryStackEntry> {
    QUERY_STACK.with(|stack| stack.borrow().clone())
}

pub(super) fn current_query_entry() -> Option<(QueryNodeId, Arc<QuerySlotIdentity>)> {
    QUERY_STACK.with(|stack| {
        stack
            .borrow()
            .last()
            .map(|entry| (entry.node_id, Arc::clone(&entry.identity)))
    })
}

pub(super) fn query_executor_is_active(executor: usize) -> bool {
    QUERY_EXECUTOR_STACK.with(|stack| stack.borrow().contains(&executor))
}

pub(super) fn query_execution_budget_is_active(budget: usize) -> bool {
    QUERY_EXECUTION_BUDGET_STACK.with(|stack| stack.borrow().contains(&budget))
}

pub(super) fn query_activity_is_active(session: usize) -> bool {
    QUERY_ACTIVITY_DEPTHS.with(|depths| {
        depths
            .borrow()
            .iter()
            .any(|(active_session, _depth)| *active_session == session)
    })
}

pub(super) fn enter_query_activity(session: usize) {
    QUERY_ACTIVITY_DEPTHS.with(|depths| {
        let mut depths = depths.borrow_mut();
        if let Some((_, depth)) = depths
            .iter_mut()
            .find(|(active_session, _depth)| *active_session == session)
        {
            *depth += 1;
        } else {
            depths.push((session, 1));
        }
    });
}

pub(super) fn leave_query_activity(session: usize) -> Result<bool, nia_ice::Ice> {
    QUERY_ACTIVITY_DEPTHS.with(|depths| {
        let mut depths = depths.borrow_mut();
        let Some(position) = depths
            .iter()
            .position(|(active_session, _depth)| *active_session == session)
        else {
            return Err(nia_ice::Ice::new(
                "query activity guard dropped without an active depth",
            ));
        };
        let depth = &mut depths[position].1;
        let Some(next_depth) = depth.checked_sub(1) else {
            return Err(nia_ice::Ice::new("query activity depth underflow"));
        };
        *depth = next_depth;
        if *depth == 0 {
            depths.swap_remove(position);
            Ok(true)
        } else {
            Ok(false)
        }
    })
}

pub(super) fn take_current_stack_dependencies() -> RecordedDependencies {
    QUERY_STACK.with(|stack| {
        stack
            .borrow_mut()
            .last_mut()
            .map(|entry| RecordedDependencies {
                nodes: std::mem::take(&mut entry.dependencies),
                fingerprints: entry.dependency_fingerprints.as_mut().map(std::mem::take),
            })
            .unwrap_or_default()
    })
}

pub(super) fn record_dependency_on_current_stack(session_id: QuerySessionId, to: QueryNodeId) {
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(from) = stack.last_mut() else {
            return;
        };
        if from.session_id == session_id {
            from.dependencies.insert(to);
            if let Some(fingerprints) = &mut from.dependency_fingerprints {
                fingerprints.entry(to).or_insert(None);
            }
        }
    });
}

pub(super) fn record_dependency_fingerprint_on_current_stack(
    session_id: QuerySessionId,
    to: QueryNodeId,
    fingerprint: Option<QueryFingerprint>,
) {
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(from) = stack.last_mut() else {
            return;
        };
        if from.session_id == session_id
            && let Some(fingerprints) = &mut from.dependency_fingerprints
        {
            fingerprints.insert(to, fingerprint);
        }
    });
}

pub(super) fn merge_dependencies_into_current_stack(dependencies: RecordedDependencies) {
    if dependencies.nodes.is_empty() {
        return;
    }
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(entry) = stack.last_mut() else {
            return;
        };
        entry.dependencies.extend(dependencies.nodes);
        if let (Some(entry_fingerprints), Some(fingerprints)) = (
            entry.dependency_fingerprints.as_mut(),
            dependencies.fingerprints,
        ) {
            entry_fingerprints.extend(fingerprints);
        }
    });
}

pub(super) fn install_query_stack(stack_snapshot: Vec<QueryStackEntry>) -> QueryStackInstallGuard {
    // Executor tasks receive an owned snapshot: dependency recording stays thread-local while
    // preserving the logical parent chain. The guard restores any worker-local outer stack.
    QUERY_STACK.with(|stack| QueryStackInstallGuard {
        previous: std::mem::replace(&mut *stack.borrow_mut(), stack_snapshot),
    })
}
