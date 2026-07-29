// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Clone, Copy)]
pub struct ExecutableRootDefs<'a> {
    pub functions: &'a [GlobalDefId],
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

pub struct ExecutableExtensionSources<'a> {
    pub methods: &'a ExtensionMethods,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

pub struct ExecutableReachabilityInput<'a> {
    pub parse_ok: &'a [ModuleId],
    pub entry_module: ModuleId,
    pub root_defs: ExecutableRootDefs<'a>,
    pub program_signatures: ExecutableSignatureIndex<'a>,
    pub modules: &'a [ReachableModuleInput<'a>],
}

pub struct CheckedModuleReachabilityInput<'a> {
    pub parse_ok: &'a [ModuleId],
    pub program_signatures: ExecutableSignatureIndex<'a>,
    pub module: ReachableModuleInput<'a>,
    pub checked_functions: &'a HashSet<GlobalDefId>,
    pub modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
}
