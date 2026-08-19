// SPDX-License-Identifier: GPL-3.0-or-later
//! Lazy signature callbacks used by reachability classification.
use super::*;

#[derive(Clone, Copy)]
/// Signature lookup boundary kept independent from the query engine.
pub struct ExecutableSignatureIndex<'a> {
    /// Looks up a source function signature.
    pub function: &'a dyn Fn(GlobalDefId) -> Option<Arc<ProgramFunctionSignature>>,
    /// Looks up a struct signature for type matching.
    pub struct_: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    /// Looks up a union signature for type matching.
    pub union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    /// Looks up a source trait signature.
    pub trait_: &'a dyn Fn(GlobalDefId) -> Option<ProgramTraitSignature>,
    /// Looks up a default trait method and its owner trait.
    pub trait_default_method:
        &'a dyn Fn(GlobalDefId) -> Option<(GlobalDefId, ProgramTraitSignature)>,
}

impl std::fmt::Debug for ExecutableSignatureIndex<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutableSignatureIndex")
            .field("function", &true)
            .field("struct_", &true)
            .field("union", &true)
            .field("trait_", &true)
            .finish()
    }
}
