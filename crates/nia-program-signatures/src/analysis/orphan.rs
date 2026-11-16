// SPDX-License-Identifier: GPL-3.0-or-later

//! The orphan rule: which package may declare a trait implementation.
//!
//! An implementation `extend[P..] T0 : Trait[T1, .., Tn]` is allowed in the
//! package that defines `Trait`. Otherwise one of `T0, .., Tn` must be
//! local, owned by the implementing package as [`coherence_owner`] defines
//! ownership, and no type parameter that no type covers may appear before
//! the first local type. This is the rule of Rust's RFC 2451.
//!
//! The rule lets lookup find every implementation that could apply to a
//! goal by searching the trait's package and the packages owning the goal's
//! types, and it keeps two packages that do not see each other from
//! declaring implementations for the same goal: an uncovered parameter
//! before the local type would let another package that owns an earlier
//! type implement it too.

use nia_ids::InternedTyId;
use nia_symbol::{SymbolId, known};
use nia_ty::coherence::{CoherenceOwner, coherence_owner};
use nia_ty::{TraitId, TypeStore};

/// Whether a package may declare an implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrphanVerdict {
    Allowed,
    /// The package owns neither the trait nor any type of the header.
    NoLocalType,
    /// The type at `position` (0 for the extended type, `i` for the trait's
    /// `i`th type argument) may be any type, and precedes the first local
    /// type: an uncovered type parameter `param`, or a projection when
    /// `param` is `None`.
    Uncovered {
        param: Option<SymbolId>,
        position: usize,
    },
}

/// The orphan rule's verdict on an implementation of `trait_id` for
/// `self_ty` with `trait_args`, declared in `package`.
pub(crate) fn orphan_verdict(
    type_store: &TypeStore,
    graph: &dyn nia_imports::ModuleGraphLookup,
    package: SymbolId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: &[InternedTyId],
) -> nia_ice::IceResult<OrphanVerdict> {
    if trait_package(graph, trait_id)? == package {
        return Ok(OrphanVerdict::Allowed);
    }
    let kind = |ty| type_store.get(ty).cloned();
    for (position, ty) in std::iter::once(self_ty)
        .chain(trait_args.iter().copied())
        .enumerate()
    {
        let owner = match coherence_owner(kind, ty) {
            CoherenceOwner::Nominal(def_id) => Some(module_package(graph, def_id.module_id)?),
            CoherenceOwner::Primitive(_) => Some(known::STD),
            CoherenceOwner::TraitObject(trait_id) => Some(trait_package(graph, trait_id)?),
            CoherenceOwner::UncoveredParam(name) => {
                return Ok(OrphanVerdict::Uncovered {
                    param: Some(name),
                    position,
                });
            }
            CoherenceOwner::Projection => {
                return Ok(OrphanVerdict::Uncovered {
                    param: None,
                    position,
                });
            }
            CoherenceOwner::None => None,
        };
        if owner == Some(package) {
            return Ok(OrphanVerdict::Allowed);
        }
    }
    Ok(OrphanVerdict::NoLocalType)
}

/// The package that defines a trait. Builtin traits are std's.
pub(crate) fn trait_package(
    graph: &dyn nia_imports::ModuleGraphLookup,
    trait_id: TraitId,
) -> nia_ice::IceResult<SymbolId> {
    match trait_id {
        TraitId::Source(def_id) => module_package(graph, def_id.module_id),
        TraitId::Builtin(_) => Ok(known::STD),
    }
}

/// The package that contains a module.
pub(crate) fn module_package(
    graph: &dyn nia_imports::ModuleGraphLookup,
    module_id: nia_ids::ModuleId,
) -> nia_ice::IceResult<SymbolId> {
    let package = graph
        .module_path(module_id)
        .map(|path| path.package)
        .ok_or_else(|| {
            nia_ice::Ice::new(format!("missing module path for module {module_id:?}"))
        })?;
    // A program that is std, as when std itself is checked, is std's tree:
    // its root is bound as std's package root, and it owns what std owns.
    let std_root = graph.package_root_module(&known::STD);
    if std_root.is_some() && graph.current_package_root_module(module_id) == std_root {
        return Ok(known::STD);
    }
    Ok(package)
}
