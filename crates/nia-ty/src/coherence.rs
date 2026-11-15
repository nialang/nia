// SPDX-License-Identifier: GPL-3.0-or-later

//! Which definition owns a type for coherence.
//!
//! A trait implementation may be declared only in a package that owns the
//! trait or a type of its header, as the orphan rule states, and lookup
//! searches exactly the packages that could own an implementation for a
//! goal. Both ask the question answered here, so that the rule and the
//! lookup cannot disagree about where an implementation can live.
//!
//! Pointers `&T` and `&mut T` are fundamental: they own nothing, a pointer
//! to a type is owned where the type is, and a pointer to a type parameter
//! leaves the parameter uncovered. A trait object is owned where its trait
//! is. Every other structural type, such as a slice, array, tuple, optional,
//! or error union, is owned by no package, and covers any parameter inside
//! it, as a nominal type does with its arguments. A projection that has not
//! been normalized may stand for any type, so it is treated like an
//! uncovered parameter.

use crate::{InternedTyId, PrimitiveTy, TraitId, TyKind};
use nia_ids::GlobalDefId;
use nia_symbol::SymbolId;

/// The owner of a type in the header of a trait implementation or goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoherenceOwner {
    /// A struct, union, or enum, owned by the package that defines it.
    Nominal(GlobalDefId),
    /// A primitive, owned by the standard package.
    Primitive(PrimitiveTy),
    /// A trait object, owned by the package that defines its trait.
    TraitObject(TraitId),
    /// A type parameter that no type covers: the type may be any type.
    UncoveredParam(SymbolId),
    /// An associated type projection that has not been normalized: it may
    /// be any type, as an uncovered parameter may.
    Projection,
    /// A structural type: owned by no package.
    None,
}

/// How implementation lookup finds the owner of a type: the name that
/// filters candidate implementations, the package that owns the type, and
/// the path of the owning module below that package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerIdentity {
    pub name: Option<SymbolId>,
    pub package: Option<SymbolId>,
    pub module_segments: Vec<SymbolId>,
}

impl CoherenceOwner {
    /// This owner as lookup identifies it. `def_name` names a definition,
    /// and `module_path` gives a module's package and its path below it.
    pub fn identity(
        self,
        def_name: impl Fn(GlobalDefId) -> Option<SymbolId>,
        module_path: impl Fn(nia_ids::ModuleId) -> Option<(SymbolId, Vec<SymbolId>)>,
    ) -> OwnerIdentity {
        match self {
            Self::Nominal(def_id) => {
                let (package, module_segments) = module_path(def_id.module_id)
                    .map_or((None, Vec::new()), |(package, segments)| {
                        (Some(package), segments)
                    });
                OwnerIdentity {
                    name: def_name(def_id),
                    package,
                    module_segments,
                }
            }
            Self::Primitive(primitive) => OwnerIdentity {
                name: Some(primitive.symbol_id()),
                package: Some(nia_symbol::known::STD),
                module_segments: Vec::new(),
            },
            Self::TraitObject(TraitId::Source(def_id)) => OwnerIdentity {
                name: None,
                package: module_path(def_id.module_id).map(|(package, _)| package),
                module_segments: Vec::new(),
            },
            Self::TraitObject(TraitId::Builtin(_)) => OwnerIdentity {
                name: None,
                package: Some(nia_symbol::known::STD),
                module_segments: Vec::new(),
            },
            Self::UncoveredParam(_) | Self::Projection | Self::None => OwnerIdentity::default(),
        }
    }
}

/// The owner of `ty`, looking through fundamental pointers. `kind` gives a
/// type's kind; callers pass the lookup of their own type store.
pub fn coherence_owner(
    kind: impl Fn(InternedTyId) -> Option<TyKind>,
    ty: InternedTyId,
) -> CoherenceOwner {
    let mut current = ty;
    loop {
        match kind(current) {
            Some(TyKind::Pointer { elem, .. }) => current = elem,
            Some(TyKind::Nominal { def_id, .. }) => return CoherenceOwner::Nominal(def_id),
            Some(TyKind::Primitive(primitive)) => return CoherenceOwner::Primitive(primitive),
            Some(
                TyKind::TraitObject { trait_id, .. } | TyKind::TraitObjectPointee { trait_id, .. },
            ) => return CoherenceOwner::TraitObject(trait_id),
            Some(TyKind::GenericParam(name)) => return CoherenceOwner::UncoveredParam(name),
            Some(TyKind::Projection { .. }) => return CoherenceOwner::Projection,
            _ => return CoherenceOwner::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TypeStore;
    use nia_ids::{DefId, ModuleIdAllocator};

    #[test]
    fn pointers_are_fundamental() {
        let store = TypeStore::new().expect("create type store");
        let module = ModuleIdAllocator::new()
            .expect("create module allocator")
            .allocate()
            .expect("allocate module");
        let append = store.append_for_module(module);
        let intern = |kind| append.intern(kind).expect("intern test type");
        let def_id = GlobalDefId {
            module_id: module,
            def_id: DefId(2),
        };
        let nominal = intern(TyKind::Nominal {
            def_id,
            args: Vec::new(),
            const_args: Vec::new(),
        });
        let pointer = intern(TyKind::Pointer {
            is_readonly: true,
            elem: nominal,
        });
        let name = SymbolId::from_stable_hash(7);
        let param = intern(TyKind::GenericParam(name));
        let param_pointer = intern(TyKind::Pointer {
            is_readonly: false,
            elem: param,
        });
        let slice = intern(TyKind::Slice {
            is_readonly: true,
            elem: nominal,
        });
        let owner = |ty| coherence_owner(|ty| store.get(ty).cloned(), ty);
        assert_eq!(owner(pointer), CoherenceOwner::Nominal(def_id));
        assert_eq!(owner(param_pointer), CoherenceOwner::UncoveredParam(name));
        assert_eq!(owner(slice), CoherenceOwner::None);
    }
}
