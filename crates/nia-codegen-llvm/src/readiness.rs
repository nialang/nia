use std::collections::HashSet;
use std::sync::Arc;

use nia_backend_ir::{
    BackendModuleOwnerDirectory, BackendModuleStore, CodegenPartition, CodegenPartitionPlan,
    CodegenUnitKey,
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
    pending: Vec<CodegenPartition>,
    unit_keys: HashSet<CodegenUnitKey>,
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
            pending: Vec::new(),
            unit_keys: HashSet::new(),
        }
    }

    pub(super) fn publish(
        &mut self,
        module_id: ModuleId,
    ) -> nia_ice::IceResult<Vec<CodegenPartitionPreparation>> {
        self.publisher.publish(module_id)?;
        let module = self.index.module(module_id).ok_or_else(|| {
            nia_ice::Ice::new("published backend module is missing from codegen index")
        })?;
        let plan = CodegenPartitionPlan::for_ready_module(module);
        for partition in plan.partitions() {
            if !self.unit_keys.insert(partition.key.clone()) {
                return Err(nia_ice::Ice::new(
                    "incremental codegen planning produced a duplicate stable unit key",
                ));
            }
        }
        self.pending.extend(plan.partitions().iter().cloned());
        Ok(self.retry_pending())
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

    fn retry_pending(&mut self) -> Vec<CodegenPartitionPreparation> {
        let mut unresolved = Vec::new();
        let mut ready = Vec::new();
        let all_modules_published = self
            .index
            .module_ids()
            .iter()
            .all(|module_id| self.index.is_published(*module_id));
        for partition in self.pending.drain(..) {
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
            match CodegenDeclarationMembership::build(&partition, &self.index, &self.owners) {
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
                    unresolved.push(partition);
                }
                CodegenDeclarationMembershipBuild::Invalid { diagnostics } => {
                    ready.push(CodegenPartitionPreparation::Invalid {
                        partition,
                        diagnostics,
                    });
                }
            }
        }
        self.pending = unresolved;
        ready
    }
}
