// SPDX-License-Identifier: GPL-3.0-or-later
//! Input products for clean and incremental executable reachability.
use super::*;

#[derive(Clone, Copy)]
/// Explicit executable roots selected by the driver/runtime policy.
pub struct ExecutableRootDefs<'a> {
    /// Source functions that must be retained even without a caller edge.
    pub functions: &'a [GlobalDefId],
    /// Runtime globals that must be retained as initialization roots.
    pub globals: &'a [GlobalDefId],
}

impl std::fmt::Debug for ExecutableRootDefs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutableRootDefs")
            .field("functions", &self.functions)
            .field("globals", &self.globals)
            .finish()
    }
}

/// Eager extension and trait-implementation sources for clean reachability.
pub struct ExecutableExtensionSources<'a> {
    /// Extension methods indexed by the caller.
    pub methods: &'a ExtensionMethods,
    /// Program trait implementation signatures.
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

/// Complete input snapshot for the clean or incremental fixed point.
pub struct ExecutableReachabilityInput<'a> {
    /// Modules whose parsing/checking products are valid for this revision.
    pub parse_ok: &'a [ModuleId],
    /// Module containing the entry point or selected root item.
    pub entry_module: ModuleId,
    /// Driver-selected function and global roots.
    pub root_defs: ExecutableRootDefs<'a>,
    /// Lazy program signature callbacks used to classify runtime bodies.
    pub program_signatures: ExecutableSignatureIndex<'a>,
    /// Reachable-module products available to dependency extraction.
    pub modules: &'a [ReachableModuleInput<'a>],
}

/// One checked module product supplied to incremental extension.
pub struct CheckedModuleReachabilityInput<'a> {
    /// Modules whose parsing/checking products are valid for this revision.
    pub parse_ok: &'a [ModuleId],
    /// Lazy program signature callbacks used by the fixed point.
    pub program_signatures: ExecutableSignatureIndex<'a>,
    /// Newly checked module dependency input.
    pub module: ReachableModuleInput<'a>,
    /// Functions whose body facts were refreshed in this batch.
    pub checked_functions: &'a HashSet<GlobalDefId>,
    /// All currently available module inputs keyed by module identity.
    pub modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
}
