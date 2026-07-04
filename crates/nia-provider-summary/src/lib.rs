// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{GenericParam, TypeKind, TypeRef};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderSummary {
    providers: Vec<Provider>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub target: ProviderTarget,
    pub trait_ref: Option<ProviderTypeRef>,
    pub associated_items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTypeRef {
    pub last_name: Option<String>,
    pub is_generic_or_structural_target: bool,
    pub semantic_is_conservative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTarget {
    pub ty: ProviderTypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NominalProviderCandidate {
    Named(String),
    Conservative,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NominalProviderCandidateIndex<M> {
    conservative: Vec<M>,
    named: HashMap<String, Vec<M>>,
}

impl<M> NominalProviderCandidateIndex<M>
where
    M: Copy + Ord + Eq,
{
    pub fn from_summaries(
        summaries: impl IntoIterator<Item = (M, ProviderSummary)>,
    ) -> NominalProviderCandidateIndex<M> {
        let mut conservative = Vec::new();
        let mut named: HashMap<String, Vec<M>> = HashMap::new();
        for (module, summary) in summaries {
            for provider in &summary.providers {
                match provider.target.ty.source_nominal_provider_candidate() {
                    NominalProviderCandidate::Named(name) => {
                        named.entry(name).or_default().push(module);
                    }
                    NominalProviderCandidate::Conservative => {
                        conservative.push(module);
                    }
                }
            }
        }
        conservative.sort();
        conservative.dedup();
        for modules in named.values_mut() {
            modules.sort();
            modules.dedup();
        }
        NominalProviderCandidateIndex {
            conservative,
            named,
        }
    }

    pub fn conservative(&self) -> &[M] {
        &self.conservative
    }

    pub fn named(&self, name: &str) -> &[M] {
        self.named.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn all_named(&self) -> impl Iterator<Item = M> + '_ {
        self.named
            .values()
            .flat_map(|modules| modules.iter().copied())
    }
}

impl ProviderSummary {
    pub fn from_active_item_tree(item_tree: &ActiveModuleItemTree) -> Self {
        let local_nominal_names = local_nominal_type_names(item_tree);
        let local_trait_names = local_trait_names(item_tree);
        let providers = item_tree
            .items
            .iter()
            .filter_map(|item| {
                let ItemTreeNodeKind::Extend(extend) = &item.kind else {
                    return None;
                };
                let generic_names = generic_param_names(&extend.generics);
                let associated_items = extend
                    .methods
                    .iter()
                    .map(|method| method.function.name.clone())
                    .chain(
                        extend
                            .associated_values
                            .iter()
                            .map(|value| value.binding.name.clone()),
                    )
                    .collect();
                Some(Provider {
                    target: ProviderTarget {
                        ty: ProviderTypeRef::from_type_ref(
                            &extend.target,
                            &generic_names,
                            &local_nominal_names,
                        ),
                    },
                    trait_ref: extend.trait_ref.as_ref().map(|trait_ref| {
                        ProviderTypeRef::from_type_ref(
                            trait_ref,
                            &generic_names,
                            &local_trait_names,
                        )
                    }),
                    associated_items,
                })
            })
            .collect();
        Self { providers }
    }

    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    pub fn may_define_nominal_provider_for(&self, target_type_name: &str) -> bool {
        self.providers
            .iter()
            .any(|provider| provider.target.ty.may_match_nominal_name(target_type_name))
    }

    pub fn nominal_provider_candidates(&self) -> Vec<NominalProviderCandidate> {
        let mut candidates = HashSet::new();
        for provider in &self.providers {
            candidates.insert(provider.target.ty.semantic_nominal_provider_candidate());
        }
        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        candidates.sort_by(|lhs, rhs| match (lhs, rhs) {
            (NominalProviderCandidate::Conservative, NominalProviderCandidate::Conservative) => {
                std::cmp::Ordering::Equal
            }
            (NominalProviderCandidate::Conservative, NominalProviderCandidate::Named(_)) => {
                std::cmp::Ordering::Less
            }
            (NominalProviderCandidate::Named(_), NominalProviderCandidate::Conservative) => {
                std::cmp::Ordering::Greater
            }
            (NominalProviderCandidate::Named(lhs), NominalProviderCandidate::Named(rhs)) => {
                lhs.cmp(rhs)
            }
        });
        candidates
    }

    pub fn defines_inherent_associated_item(
        &self,
        target_type_name: &str,
        associated_name: &str,
    ) -> bool {
        self.providers.iter().any(|provider| {
            provider.trait_ref.is_none()
                && provider.target.ty.may_match_nominal_name(target_type_name)
                && provider.has_associated_item(associated_name)
        })
    }

    pub fn defines_trait_impl(&self, trait_name: &str, associated_name: Option<&str>) -> bool {
        self.providers.iter().any(|provider| {
            provider
                .trait_ref
                .as_ref()
                .is_some_and(|trait_ref| trait_ref.may_match_nominal_name(trait_name))
                && associated_name.is_none_or(|name| provider.has_associated_item(name))
        })
    }

    pub fn defines_public_extension_method_for_facade(
        &self,
        facade_exposes_type: impl Fn(&str) -> bool,
        target_type_name: Option<&str>,
        associated_name: &str,
    ) -> bool {
        self.providers.iter().any(|provider| {
            if let Some(target_type_name) = target_type_name {
                if !provider.target.ty.may_match_nominal_name(target_type_name) {
                    return false;
                }
            } else if provider.trait_ref.is_none()
                && !provider.target.ty.is_generic_or_structural_target
            {
                return false;
            }
            if let Some(trait_ref) = &provider.trait_ref {
                let Some(trait_name) = trait_ref.last_name.as_deref() else {
                    return false;
                };
                if !facade_exposes_type(trait_name) && trait_ref.is_definite_semantic_name() {
                    return false;
                }
            }
            provider.has_associated_item(associated_name)
        })
    }
}

impl Provider {
    fn has_associated_item(&self, name: &str) -> bool {
        self.associated_items
            .iter()
            .any(|associated| associated == name)
    }
}

impl ProviderTypeRef {
    fn from_type_ref(
        ty: &TypeRef,
        generic_names: &HashSet<&str>,
        definite_names: &HashSet<&str>,
    ) -> Self {
        let last_name = type_ref_last_name(ty).map(ToString::to_string);
        let is_generic_or_structural_target =
            type_ref_is_generic_or_structural_provider_target(ty, generic_names);
        let semantic_is_conservative =
            !type_ref_is_definite_local_nominal_name(ty, generic_names, definite_names);
        Self {
            last_name,
            is_generic_or_structural_target,
            semantic_is_conservative,
        }
    }

    fn may_match_nominal_name(&self, name: &str) -> bool {
        self.is_generic_or_structural_target
            || self.semantic_is_conservative
            || self.last_name.as_deref().is_none_or(|last| last == name)
    }

    fn semantic_nominal_provider_candidate(&self) -> NominalProviderCandidate {
        if self.semantic_is_conservative {
            return NominalProviderCandidate::Conservative;
        }
        self.last_name
            .clone()
            .map(NominalProviderCandidate::Named)
            .unwrap_or(NominalProviderCandidate::Conservative)
    }

    fn source_nominal_provider_candidate(&self) -> NominalProviderCandidate {
        if self.is_generic_or_structural_target {
            return NominalProviderCandidate::Conservative;
        }
        self.last_name
            .clone()
            .map(NominalProviderCandidate::Named)
            .unwrap_or(NominalProviderCandidate::Conservative)
    }

    fn is_definite_semantic_name(&self) -> bool {
        !self.semantic_is_conservative
    }
}

fn local_nominal_type_names(item_tree: &ActiveModuleItemTree) -> HashSet<&str> {
    item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Struct(item) => Some(item.name.as_str()),
            ItemTreeNodeKind::Union(item) => Some(item.name.as_str()),
            ItemTreeNodeKind::Enum(item) => Some(item.name.as_str()),
            _ => None,
        })
        .collect()
}

fn local_trait_names(item_tree: &ActiveModuleItemTree) -> HashSet<&str> {
    item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Trait(item) => Some(item.name.as_str()),
            _ => None,
        })
        .collect()
}

fn type_ref_last_name(ty: &TypeRef) -> Option<&str> {
    match &ty.kind {
        TypeKind::Path { segments } => segments.last().map(|segment| segment.name.as_str()),
        _ => None,
    }
}

fn type_ref_is_definite_local_nominal_name(
    ty: &TypeRef,
    generic_names: &HashSet<&str>,
    definite_names: &HashSet<&str>,
) -> bool {
    match &ty.kind {
        TypeKind::Path { segments } => {
            segments.len() == 1
                && segments[0].args.is_empty()
                && !generic_names.contains(segments[0].name.as_str())
                && definite_names.contains(segments[0].name.as_str())
        }
        _ => false,
    }
}

fn generic_param_names(generics: &[GenericParam]) -> HashSet<&str> {
    generics
        .iter()
        .map(|generic| generic.name.as_str())
        .collect()
}

fn type_ref_is_generic_or_structural_provider_target(
    ty: &TypeRef,
    generic_names: &HashSet<&str>,
) -> bool {
    match &ty.kind {
        TypeKind::Path { segments } => {
            segments.len() == 1
                && segments[0].args.is_empty()
                && generic_names.contains(segments[0].name.as_str())
        }
        TypeKind::Pointer { .. }
        | TypeKind::VolatilePointer { .. }
        | TypeKind::Slice { .. }
        | TypeKind::SlicePointee { .. }
        | TypeKind::Array { .. }
        | TypeKind::Range { .. }
        | TypeKind::FunctionPointer { .. }
        | TypeKind::Optional { .. }
        | TypeKind::ErrorUnion { .. }
        | TypeKind::SelfType
        | TypeKind::Infer => true,
        TypeKind::Error | TypeKind::Projection { .. } | TypeKind::Void | TypeKind::Never => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;

    #[test]
    fn summarizes_inherent_associated_items() {
        let summary = summary_for(
            r#"
struct Widget {}

extend Widget {
    pub fn score(&self) i32 { 1 }
    pub comptime Kind: i32 = 0;
}
"#,
        );

        assert!(summary.has_providers());
        assert!(summary.defines_inherent_associated_item("Widget", "score"));
        assert!(summary.defines_inherent_associated_item("Widget", "Kind"));
        assert!(!summary.defines_inherent_associated_item("Widget", "missing"));
    }

    #[test]
    fn summarizes_trait_impls_by_trait_and_associated_item() {
        let summary = summary_for(
            r#"
trait Hash {
    fn hash(&self) u64;
}
struct Widget {}

extend Widget : Hash {
    pub fn hash(&self) u64 { 1u64 }
}
"#,
        );

        assert!(summary.defines_trait_impl("Hash", None));
        assert!(summary.defines_trait_impl("Hash", Some("hash")));
        assert!(!summary.defines_trait_impl("Hash", Some("finish")));
    }

    #[test]
    fn nominal_provider_summary_keeps_generic_targets_conservative() {
        let summary = summary_for(
            r#"
trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}
"#,
        );

        assert!(summary.may_define_nominal_provider_for("Error"));
    }

    #[test]
    fn nominal_provider_summary_filters_unrelated_plain_targets() {
        let summary = summary_for(
            r#"
struct Used {}
struct Unused {}

extend Used {
    pub fn len(&self) i32 { 1 }
}
"#,
        );

        assert!(summary.may_define_nominal_provider_for("Used"));
        assert!(!summary.may_define_nominal_provider_for("Other"));
        assert_eq!(
            summary.nominal_provider_candidates(),
            vec![NominalProviderCandidate::Named("Used".to_string())]
        );
    }

    #[test]
    fn nominal_provider_summary_keeps_alias_targets_conservative() {
        let summary = summary_for(
            r#"
struct Used {}
type Alias = Used;

extend Alias {
    pub fn len(&self) i32 { 1 }
}
"#,
        );

        assert!(summary.may_define_nominal_provider_for("Used"));
        assert!(summary.may_define_nominal_provider_for("Other"));
        assert_eq!(
            summary.nominal_provider_candidates(),
            vec![NominalProviderCandidate::Conservative]
        );
        let index = NominalProviderCandidateIndex::from_summaries([(0usize, summary)]);
        assert_eq!(index.named("Alias"), &[0usize]);
        assert!(index.named("Used").is_empty());
    }

    #[test]
    fn filters_trait_extension_methods_by_facade_surface() {
        let summary = summary_for(
            r#"
trait Hash {
    fn hash(&self) u64;
}
struct Widget {}

extend Widget : Hash {
    pub fn hash(&self) u64 { 1u64 }
}
"#,
        );

        assert!(summary.defines_public_extension_method_for_facade(
            |name| name == "Hash",
            Some("Widget"),
            "hash",
        ));
        assert!(!summary.defines_public_extension_method_for_facade(
            |_| false,
            Some("Widget"),
            "hash",
        ));
    }

    fn summary_for(source: &str) -> ProviderSummary {
        let (module, errors) = parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        let item_tree = ModuleItemTree::from_module(&module);
        let active = nia_item_tree::ActiveModuleItemTree::new(
            item_tree.active_items_without_comptime(),
            Default::default(),
        );
        ProviderSummary::from_active_item_tree(&active)
    }
}
