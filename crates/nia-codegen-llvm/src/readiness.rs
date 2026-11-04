use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use nia_backend_ir::{
    BackendModuleOwnerDirectory, BackendModuleStore, CodegenPartition, CodegenPartitionPlan,
    CodegenUnitId, CodegenUnitKey,
};
use nia_diagnostic::Diagnostic;
use nia_ids::ModuleId;
use nia_ty::TypeStore;

use crate::backend_validate::validate_backend_partition_definitions;
use crate::declaration_membership::{
    CodegenDeclarationMembership, CodegenDeclarationMembershipBuild,
};
use crate::program_index::{ProgramIndex, ProgramIndexPublisher};

pub(super) struct PreparedCodegenPartition {
    pub(super) partition: CodegenPartition,
    pub(super) declarations: Box<CodegenDeclarationMembership>,
}

pub(super) enum CodegenPartitionPreparation {
    Ready(PreparedCodegenPartition),
    Invalid {
        partition: CodegenPartition,
        diagnostics: Vec<Diagnostic>,
    },
}

impl CodegenPartitionPreparation {
    pub(super) fn key(&self) -> &CodegenUnitKey {
        match self {
            Self::Ready(prepared) => &prepared.partition.key,
            Self::Invalid { partition, .. } => &partition.key,
        }
    }
}

pub(super) struct CodegenReadinessCoordinator {
    pub(super) index: Arc<ProgramIndex>,
    publisher: ProgramIndexPublisher,
    owners: Arc<BackendModuleOwnerDirectory>,
    pending: PendingPartitions<CodegenPartition>,
    unit_keys: HashSet<CodegenUnitKey>,
    published_module_count: usize,
}

struct PendingPartitions<T> {
    entries: HashMap<CodegenUnitId, PendingPartition<T>>,
    by_module: HashMap<ModuleId, BTreeSet<CodegenUnitId>>,
}

struct PendingPartition<T> {
    value: T,
    waiting_for: Vec<ModuleId>,
}

impl<T> Default for PendingPartitions<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            by_module: HashMap::new(),
        }
    }
}

impl<T> PendingPartitions<T> {
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn insert(
        &mut self,
        unit: CodegenUnitId,
        value: T,
        waiting_for: &[ModuleId],
    ) -> nia_ice::IceResult<()> {
        let waiting_for = waiting_for.iter().copied().collect::<BTreeSet<_>>();
        if waiting_for.is_empty() {
            return Err(nia_ice::Ice::new(
                "pending codegen partition has no unpublished module dependencies",
            ));
        }
        if self.entries.contains_key(&unit) {
            return Err(nia_ice::Ice::new(
                "pending codegen partition was registered more than once",
            ));
        }
        let waiting_for = waiting_for.into_iter().collect::<Vec<_>>();
        self.entries.insert(
            unit,
            PendingPartition {
                value,
                waiting_for: waiting_for.clone(),
            },
        );
        for module_id in waiting_for {
            self.by_module.entry(module_id).or_default().insert(unit);
        }
        Ok(())
    }

    fn take_waiting_for(
        &mut self,
        module_id: ModuleId,
    ) -> nia_ice::IceResult<Vec<(CodegenUnitId, T)>> {
        let Some(units) = self.by_module.remove(&module_id) else {
            return Ok(Vec::new());
        };
        let mut candidates = Vec::with_capacity(units.len());
        for unit in units {
            let Some(entry) = self.entries.remove(&unit) else {
                return Err(nia_ice::Ice::new(
                    "pending codegen module index references a missing partition",
                ));
            };
            if !entry.waiting_for.contains(&module_id) {
                return Err(nia_ice::Ice::new(
                    "pending codegen module index returned an unrelated partition",
                ));
            }
            for waiting_for in entry.waiting_for {
                if waiting_for == module_id {
                    continue;
                }
                let is_empty = {
                    let Some(waiting) = self.by_module.get_mut(&waiting_for) else {
                        return Err(nia_ice::Ice::new(
                            "pending codegen partition lost a module dependency index",
                        ));
                    };
                    if !waiting.remove(&unit) {
                        return Err(nia_ice::Ice::new(
                            "pending codegen module index lost a partition",
                        ));
                    }
                    waiting.is_empty()
                };
                if is_empty {
                    self.by_module.remove(&waiting_for);
                }
            }
            candidates.push((unit, entry.value));
        }
        Ok(candidates)
    }

    fn take_all(&mut self) -> Vec<(CodegenUnitId, T)> {
        self.by_module.clear();
        let mut entries = self
            .entries
            .drain()
            .map(|(unit, entry)| (unit, entry.value))
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(unit, _)| *unit);
        entries
    }
}

impl CodegenReadinessCoordinator {
    pub(super) fn new(
        modules: Arc<BackendModuleStore>,
        type_store: Arc<TypeStore>,
        owners: Arc<BackendModuleOwnerDirectory>,
    ) -> Self {
        let (index, publisher) = ProgramIndex::new(modules, type_store);
        Self {
            index,
            publisher,
            owners,
            pending: PendingPartitions::default(),
            unit_keys: HashSet::new(),
            published_module_count: 0,
        }
    }

    pub(super) fn publish(
        &mut self,
        module_id: ModuleId,
    ) -> nia_ice::IceResult<Vec<CodegenPartitionPreparation>> {
        self.publisher.publish(module_id)?;
        self.published_module_count += 1;
        let module = self.index.module(module_id).ok_or_else(|| {
            nia_ice::Ice::new("published backend module is missing from codegen index")
        })?;
        let plan = CodegenPartitionPlan::for_ready_module(module)?;
        for partition in plan.partitions() {
            if !self.unit_keys.insert(partition.key.clone()) {
                return Err(nia_ice::Ice::new(
                    "incremental codegen planning produced a duplicate stable unit key",
                ));
            }
        }
        let all_modules_published = self.published_module_count == self.index.module_ids().len();
        let mut candidates = if all_modules_published {
            self.pending.take_all()
        } else {
            self.pending.take_waiting_for(module_id)?
        };
        candidates.extend(
            plan.partitions()
                .iter()
                .cloned()
                .map(|partition| (partition.id, partition)),
        );
        candidates.sort_unstable_by_key(|(unit, _)| *unit);
        self.prepare_candidates(candidates)
    }

    pub(super) fn finish(self) -> nia_ice::IceResult<Arc<ProgramIndex>> {
        if !self
            .index
            .module_ids()
            .iter()
            .all(|module_id| self.index.is_published(*module_id))
        {
            return Err(nia_ice::Ice::new(
                "codegen readiness finished before every backend module was published",
            ));
        }
        if !self.pending.is_empty() {
            return Err(nia_ice::Ice::new(
                "codegen readiness finished with unresolved partitions",
            ));
        }
        self.publisher.freeze()?;
        Ok(self.index)
    }

    fn prepare_candidates(
        &mut self,
        candidates: Vec<(CodegenUnitId, CodegenPartition)>,
    ) -> nia_ice::IceResult<Vec<CodegenPartitionPreparation>> {
        let mut ready = Vec::new();
        let all_modules_published = self.published_module_count == self.index.module_ids().len();
        for (candidate_unit, partition) in candidates {
            if candidate_unit != partition.id {
                return Err(nia_ice::Ice::new(
                    "pending codegen index returned a different partition identity",
                ));
            }
            if all_modules_published {
                let diagnostics = validate_backend_partition_definitions(&partition, &self.index);
                if !diagnostics.is_empty() {
                    ready.push(CodegenPartitionPreparation::Invalid {
                        partition,
                        diagnostics,
                    });
                    continue;
                }
            }
            match CodegenDeclarationMembership::build(&partition, &self.index, &self.owners)? {
                CodegenDeclarationMembershipBuild::Ready(declarations) => {
                    if !all_modules_published {
                        let diagnostics =
                            validate_backend_partition_definitions(&partition, &self.index);
                        if !diagnostics.is_empty() {
                            ready.push(CodegenPartitionPreparation::Invalid {
                                partition,
                                diagnostics,
                            });
                            continue;
                        }
                    }
                    ready.push(CodegenPartitionPreparation::Ready(
                        PreparedCodegenPartition {
                            partition,
                            declarations,
                        },
                    ));
                }
                CodegenDeclarationMembershipBuild::Pending(pending) => {
                    if pending.unit() != partition.id {
                        ready.push(CodegenPartitionPreparation::Invalid {
                            partition,
                            diagnostics: vec![nia_diagnostic::Diagnostic::from(
                                nia_ice::Ice::new(
                                    "pending declaration membership belongs to a different codegen unit",
                                ),
                            )],
                        });
                        continue;
                    }
                    if pending
                        .modules()
                        .iter()
                        .any(|module_id| self.index.is_published(*module_id))
                    {
                        ready.push(CodegenPartitionPreparation::Invalid {
                            partition,
                            diagnostics: vec![nia_diagnostic::Diagnostic::from(nia_ice::Ice::new(
                                "pending declaration membership named an already published module",
                            ))],
                        });
                        continue;
                    }
                    self.pending
                        .insert(partition.id, partition, pending.modules())?;
                }
                CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                    ready.push(CodegenPartitionPreparation::Invalid {
                        partition,
                        diagnostics,
                    });
                }
            }
        }
        Ok(ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::ModuleIdAllocator;

    #[test]
    fn pending_partitions_are_retried_only_for_indexed_modules() {
        let ids = ModuleIdAllocator::new().expect("create module ids");
        let owner = ids.allocate().expect("allocate owner");
        let first_dependency = ids.allocate().expect("allocate first dependency");
        let second_dependency = ids.allocate().expect("allocate second dependency");
        let unrelated_module = ids.allocate().expect("allocate unrelated module");
        let mut pending = PendingPartitions::default();
        let first = CodegenUnitId::SourceModule {
            module_id: owner,
            ordinal: 0,
        };
        let second = CodegenUnitId::SourceModule {
            module_id: owner,
            ordinal: 1,
        };
        let third = CodegenUnitId::SourceModule {
            module_id: owner,
            ordinal: 2,
        };

        pending
            .insert(first, "first", &[first_dependency, second_dependency])
            .expect("index first partition dependencies");
        pending
            .insert(second, "second", &[second_dependency])
            .expect("index second partition dependency");
        pending
            .insert(third, "third", &[first_dependency])
            .expect("index third partition dependency");

        assert!(
            pending
                .take_waiting_for(unrelated_module)
                .expect("unrelated module publication has no waiters")
                .is_empty()
        );
        assert_eq!(pending.entries.len(), 3);
        assert_eq!(
            pending
                .take_waiting_for(first_dependency)
                .expect("wake first dependency"),
            [(first, "first"), (third, "third")]
        );
        assert_eq!(pending.entries.len(), 1);
        pending
            .insert(first, "first", &[second_dependency])
            .expect("re-index partition for its remaining dependency");
        assert_eq!(
            pending
                .take_waiting_for(second_dependency)
                .expect("wake remaining first dependency"),
            [(first, "first"), (second, "second")]
        );
        assert!(pending.is_empty());
        assert!(pending.by_module.is_empty());
    }
}
