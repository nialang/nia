// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable package/module identity remapping for the query session.

use super::*;

/// Resolves a session-local definition to its relocation-independent package.
///
/// Implementations are owned by the compiler/loader boundary and may consult
/// package manifests or an installed identity index. The resolver must not
/// infer ownership from declaration spelling or physical source paths.
pub trait StableDefinitionPackageResolver {
    fn package_for_definition(&self, def_id: GlobalDefId) -> QueryResult<PackageId>;
}

/// Resolves a session module to its relocation-independent package identity.
pub trait StableModulePackageResolver {
    fn package_for_module(&self, module_id: ModuleId) -> QueryResult<PackageId>;
}

impl<F> StableModulePackageResolver for F
where
    F: Fn(ModuleId) -> QueryResult<PackageId>,
{
    fn package_for_module(&self, module_id: ModuleId) -> QueryResult<PackageId> {
        self(module_id)
    }
}

/// Session-local remap table for stable package module identities.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StableModuleIndex {
    modules: BTreeMap<StableModuleId, ModuleId>,
}

impl StableModuleIndex {
    /// Creates an empty module remap table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts one identity, returning the previous handle when present.
    pub fn insert(&mut self, identity: StableModuleId, module: ModuleId) -> Option<ModuleId> {
        self.modules.insert(identity, module)
    }

    /// Resolves a stable module identity to a current session handle.
    pub fn module(&self, identity: &StableModuleId) -> Option<ModuleId> {
        self.modules.get(identity).copied()
    }

    /// Returns the number of remapped modules.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Reports whether no modules are installed.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

/// Resolves a stable definition identity into the current compiler
/// session. Implementations are responsible for remapping package/module
/// identities; no physical path lookup is implied by this trait.
pub trait StableDefinitionResolver {
    fn definition_for_identity(&self, definition: &DefinitionId) -> QueryResult<GlobalDefId>;
}

/// Session-local remap table for stable package definition identities.
///
/// The table is built from the current module/definition facts and an
/// explicit package resolver. It is the only supported bridge from immutable
/// package identities to transient `ModuleId`/`DefId` handles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StableDefinitionIndex {
    pub(super) definitions: BTreeMap<DefinitionId, GlobalDefId>,
    pub(super) modules: StableModuleIndex,
}

impl StableDefinitionIndex {
    pub fn definition(&self, identity: &DefinitionId) -> Option<GlobalDefId> {
        self.definitions.get(identity).copied()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Iterates every stable definition remapped in this session.
    pub fn iter(&self) -> impl Iterator<Item = (&DefinitionId, &GlobalDefId)> {
        self.definitions.iter()
    }

    /// Resolves a stable module identity to its current session handle.
    pub fn module(&self, identity: &StableModuleId) -> Option<ModuleId> {
        self.modules.module(identity)
    }

    /// Returns the number of remapped public modules.
    pub fn module_len(&self) -> usize {
        self.modules.len()
    }
}

impl StableDefinitionResolver for StableDefinitionIndex {
    fn definition_for_identity(&self, definition: &DefinitionId) -> QueryResult<GlobalDefId> {
        let resolved = self.definition(definition).or_else(|| {
            (definition.disambiguator == 0)
                .then(|| {
                    self.definitions
                        .iter()
                        .filter(|(candidate, _)| {
                            candidate.module == definition.module
                                && candidate.name == definition.name
                                && candidate.kind == definition.kind
                                && candidate.owner == definition.owner
                        })
                        .map(|(_, resolved)| *resolved)
                        .collect::<Vec<_>>()
                })
                .and_then(|matches| (matches.len() == 1).then_some(matches[0]))
        });
        resolved.ok_or_else(|| QueryError::InvalidInput {
            query: QueryFrame {
                name: "stable_definition_index",
                stats_category: None,
                key: "StableDefinitionIndex".into(),
                description: "stable_definition_index".into(),
            },
            message: format!(
                "compiled definition is not present in the current session: {definition:?}"
            ),
        })
    }
}

impl<F> StableDefinitionResolver for F
where
    F: Fn(&DefinitionId) -> QueryResult<GlobalDefId>,
{
    fn definition_for_identity(&self, definition: &DefinitionId) -> QueryResult<GlobalDefId> {
        self(definition)
    }
}

/// Resolves one loaded source definition through the query graph.
pub(in crate::query) fn resolve_loaded_definition_in_query(
    db: &QueryDb<CompilerContext>,
    definition: &DefinitionId,
    package: &PackageId,
) -> QueryResult<GlobalDefId> {
    if &definition.module.package != package {
        return Err(db.invalid_input(
            &ModuleGraphQuery,
            "stable definition belongs to a different package".to_string(),
        ));
    }
    let graph = db.get(ModuleGraphQuery)?;
    let module_id = graph.module_id_for_path(&definition.module.path);
    let Some(module_id) = module_id else {
        return Err(db.invalid_input(
            &ModuleGraphQuery,
            format!(
                "stable definition module is not loaded: {}",
                definition.module.path
            ),
        ));
    };
    let defs = db.get(FullModuleDefsQuery(module_id))?;
    let symbols = db.context().loader_facts().symbols();
    let matches = defs
        .semantic
        .defs
        .iter()
        .filter_map(|(def_id, def)| {
            if def_kind_tag(def.kind) != definition.kind
                || !symbols
                    .resolve(def.name)
                    .is_some_and(|name| name.as_ref() == definition.name.as_str())
            {
                return None;
            }
            let mut owner_chain = Vec::new();
            let mut parent = def.parent;
            while let Some(parent_id) = parent {
                let parent_def = defs.semantic.defs.get(parent_id)?;
                let parent_name = symbols.resolve(parent_def.name)?;
                owner_chain.push((
                    parent_name.to_string(),
                    def_kind_tag(parent_def.kind),
                    parent_id.0,
                ));
                parent = parent_def.parent;
            }
            let mut owner_identity = None;
            for (owner_name, owner_kind, owner_disambiguator) in owner_chain.into_iter().rev() {
                owner_identity = Some(Box::new(DefinitionId {
                    module: definition.module.clone(),
                    name: owner_name,
                    kind: owner_kind,
                    disambiguator: owner_disambiguator,
                    owner: owner_identity,
                }));
            }
            (definition.disambiguator == 0 || definition.disambiguator == def_id.0)
                .then_some((owner_identity, GlobalDefId { module_id, def_id }))
        })
        .filter(|(owner_identity, _)| owner_identity == &definition.owner)
        .map(|(_, resolved)| resolved)
        .collect::<Vec<_>>();
    let [resolved] = matches.as_slice() else {
        return Err(db.invalid_input(
            &ModuleGraphQuery,
            format!(
                "stable definition is missing or ambiguous: {}::{}",
                definition.module.path, definition.name
            ),
        ));
    };
    Ok(*resolved)
}

impl<F> StableDefinitionPackageResolver for F
where
    F: Fn(GlobalDefId) -> QueryResult<PackageId>,
{
    fn package_for_definition(&self, def_id: GlobalDefId) -> QueryResult<PackageId> {
        self(def_id)
    }
}
