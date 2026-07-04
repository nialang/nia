use crate::used_paths::{UsedModulePath, host_segments, module_using_aliases, using_host_path};
use nia_ast::{UsingGroupItem, UsingSelector};
use nia_imports::{ModuleMap, Visibility};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleFacadeFacts {
    public_type_names: HashSet<String>,
    public_reexports: Vec<PublicReexportSource>,
    provider_source_paths: Vec<UsedModulePath>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PublicReexportSource {
    exposed_name: Option<String>,
    source: UsedModulePath,
}

impl ModuleFacadeFacts {
    pub(crate) fn from_active_item_tree(
        item_tree: &ActiveModuleItemTree,
        module_map: &ModuleMap,
    ) -> Self {
        let local_module_names = local_module_names(item_tree);
        let aliases = module_using_aliases(item_tree, module_map, &local_module_names);
        let mut public_type_names = HashSet::new();
        let mut public_reexports = Vec::new();
        let mut provider_source_paths = Vec::new();

        for item in &item_tree.items {
            if item.visibility == Visibility::Public
                && let Some(name) = public_type_name(item)
            {
                public_type_names.insert(name.to_string());
            }

            let ItemTreeNodeKind::Using(using) = &item.kind else {
                continue;
            };
            let Some(host_path) =
                using_host_path(&using.host, module_map, &local_module_names, &aliases)
            else {
                continue;
            };

            if item.visibility == Visibility::Public {
                collect_public_reexport_sources(&host_path, &using.selector, &mut public_reexports);
            }
            collect_provider_source_paths(&host_path, &using.selector, &mut provider_source_paths);
        }

        public_reexports.sort();
        public_reexports.dedup();
        provider_source_paths.sort();
        provider_source_paths.dedup();

        Self {
            public_type_names,
            public_reexports,
            provider_source_paths,
        }
    }

    pub(crate) fn public_reexport_exposes_name(&self, name: &str) -> bool {
        self.public_reexports
            .iter()
            .any(|reexport| reexport.exposes_name(name))
    }

    pub(crate) fn public_type_exposes_name(&self, name: &str) -> bool {
        self.public_type_names.contains(name) || self.public_reexport_exposes_name(name)
    }

    pub(crate) fn reexport_source_paths<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a UsedModulePath> + 'a {
        self.public_reexports
            .iter()
            .filter(move |reexport| reexport.exposes_name(name))
            .map(|reexport| &reexport.source)
    }

    pub(crate) fn provider_source_paths(&self) -> &[UsedModulePath] {
        &self.provider_source_paths
    }
}

impl PublicReexportSource {
    fn exact(exposed_name: String, source: UsedModulePath) -> Self {
        Self {
            exposed_name: Some(exposed_name),
            source,
        }
    }

    fn wildcard(source: UsedModulePath) -> Self {
        Self {
            exposed_name: None,
            source,
        }
    }

    fn exposes_name(&self, name: &str) -> bool {
        self.exposed_name
            .as_deref()
            .is_none_or(|exposed| exposed == name)
    }
}

fn local_module_names(item_tree: &ActiveModuleItemTree) -> Vec<String> {
    item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Module(module) => Some(module.name.clone()),
            _ => None,
        })
        .collect()
}

fn public_type_name(item: &nia_item_tree::ItemTreeNode) -> Option<&str> {
    match &item.kind {
        ItemTreeNodeKind::Struct(item) => Some(&item.name),
        ItemTreeNodeKind::Union(item) => Some(&item.name),
        ItemTreeNodeKind::Trait(item) => Some(&item.name),
        ItemTreeNodeKind::Enum(item) => Some(&item.name),
        ItemTreeNodeKind::TypeAlias(item) => Some(&item.name),
        _ => None,
    }
}

fn collect_public_reexport_sources(
    host_path: &UsedModulePath,
    selector: &UsingSelector,
    sources: &mut Vec<PublicReexportSource>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(name) = host_path.last_segment_name() {
                sources.push(PublicReexportSource::exact(
                    name.to_string(),
                    host_path.clone(),
                ));
            }
        }
        UsingSelector::Wildcard { .. } => {
            sources.push(PublicReexportSource::wildcard(host_path.clone()));
        }
        UsingSelector::Single(name) => {
            sources.push(PublicReexportSource::exact(
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
                host_path.clone(),
            ));
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_public_reexport_sources_for_group_item(host_path, item, sources);
            }
        }
    }
}

fn collect_public_reexport_sources_for_group_item(
    host_path: &UsedModulePath,
    item: &UsingGroupItem,
    sources: &mut Vec<PublicReexportSource>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            sources.push(PublicReexportSource::exact(
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
                host_path.clone(),
            ));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested = host_path.with_appended_segments(&host_segments(host), false);
            collect_public_reexport_sources(&nested, selector, sources);
        }
    }
}

fn collect_provider_source_paths(
    host_path: &UsedModulePath,
    selector: &UsingSelector,
    paths: &mut Vec<UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName | UsingSelector::Wildcard { .. } => {
            paths.push(host_path.with_declared_children_and_processing(false, false));
        }
        UsingSelector::Single(name) => {
            paths.push(host_path.with_appended_segments_with_processing(
                std::slice::from_ref(&name.name),
                false,
                false,
            ));
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_provider_source_paths_for_group_item(host_path, item, paths);
            }
        }
    }
}

fn collect_provider_source_paths_for_group_item(
    host_path: &UsedModulePath,
    item: &UsingGroupItem,
    paths: &mut Vec<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            paths.push(host_path.with_appended_segments_with_processing(
                std::slice::from_ref(&name.name),
                false,
                false,
            ));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested = host_path.with_appended_segments_with_processing(
                &host_segments(host),
                false,
                false,
            );
            collect_provider_source_paths(&nested, selector, paths);
        }
    }
}
