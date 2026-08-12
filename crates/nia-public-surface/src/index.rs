use std::{borrow::Borrow, collections::HashMap};

use nia_defs::{DefCollection, DefKind, ModuleUsingScope, PublicSurfaces};
use nia_ids::{GlobalDefId, ModuleId};
use nia_symbol::SymbolId;

/// Reverse index from a type definition to every source-level name exposing it.
///
/// The compiler uses this as a semantic product for diagnostics and extension
/// lookup. Names are collected from direct definitions, exported surfaces, and
/// local `using` scopes, then sorted and deduplicated so the result is stable
/// regardless of module traversal order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypeExposureIndex {
    names_by_target: HashMap<GlobalDefId, Vec<SymbolId>>,
}

impl TypeExposureIndex {
    pub fn from_defs_surfaces_and_using_scopes<D: Borrow<DefCollection>>(
        defs_by_module: &[D],
        surfaces: &PublicSurfaces,
        using_scopes: &HashMap<ModuleId, ModuleUsingScope>,
    ) -> Self {
        let mut names_by_target: HashMap<GlobalDefId, Vec<SymbolId>> = HashMap::new();

        for defs in defs_by_module {
            let defs = defs.borrow();
            for (def_id, def) in defs.defs.iter() {
                if !matches!(
                    def.kind,
                    DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias
                ) {
                    continue;
                }
                names_by_target
                    .entry(GlobalDefId {
                        module_id: defs.module_id,
                        def_id,
                    })
                    .or_default()
                    .push(def.name);
            }
        }

        for surface in surfaces.iter().map(|(_, surface)| surface) {
            for (name, item) in &surface.types {
                names_by_target
                    .entry(GlobalDefId {
                        module_id: item.target_module,
                        def_id: item.target_def_id,
                    })
                    .or_default()
                    .push(*name);
            }
        }

        for using_scope in using_scopes.values() {
            for (name, entry) in &using_scope.types {
                names_by_target
                    .entry(GlobalDefId {
                        module_id: entry.target_module,
                        def_id: entry.target_def_id,
                    })
                    .or_default()
                    .push(*name);
            }
        }

        for names in names_by_target.values_mut() {
            names.sort();
            names.dedup();
        }
        Self { names_by_target }
    }

    pub fn names_for(&self, target: GlobalDefId) -> &[SymbolId] {
        self.names_by_target
            .get(&target)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}
