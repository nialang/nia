// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct MonomorphizationQuery;

impl QueryKey<DriverContext> for MonomorphizationQuery {
    type Value = nia_monomorphize::Monomorphization;

    fn name() -> &'static str {
        "monomorphization"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.monomorphization)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendLoweringQuery;

impl QueryKey<DriverContext> for BackendLoweringQuery {
    type Value = nia_backend_lower::BackendLowering;

    fn name() -> &'static str {
        "backend_lowering"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.backend_lowering)(db)
    }
}
