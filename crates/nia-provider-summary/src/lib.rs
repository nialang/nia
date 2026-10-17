// SPDX-License-Identifier: GPL-3.0-or-later
//! Conservative summaries and indexes for extension-method providers.
//!
//! A [`ProviderSummary`] lists a module's `extend` blocks from its active item
//! tree: the target type, the optional trait, and the declared method and
//! associated-value names. Type references are classified syntactically only;
//! local alias chains are followed, while unresolved imports and structural
//! targets remain conservative. [`NominalProviderCandidateIndex`] maps a definite
//! nominal name to the modules that may provide extensions for it, so
//! extension discovery can skip modules that cannot match while still
//! visiting every conservative provider.
//!
//! [`ProviderDemand`] records one cross-module fact requested during semantic
//! analysis; [`ProviderFactRevision`] is a monotonic revision token for those
//! facts.

mod demand;

pub use demand::{
    ProviderDemand, ProviderFactRevision, ProviderFactRevisionTransition, ProviderRequest,
};

use std::collections::HashSet;

use nia_ast::{GenericParam, PathSegmentKind, TypeArg, TypeKind, TypeRef};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use nia_symbol::{SymbolId, SymbolMap, SymbolSet, ToSymbolId};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// Module-local summary of extension providers.
pub struct ProviderSummary {
    providers: Vec<Provider>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One extension block and its associated items.
pub struct Provider {
    /// Target type classification.
    pub target: ProviderTarget,
    /// Optional trait implemented by the extension.
    pub trait_ref: Option<ProviderTypeRef>,
    /// Method names declared by the extension.
    pub associated_methods: Vec<SymbolId>,
    /// Associated-value names declared by the extension.
    pub associated_values: Vec<SymbolId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Lightweight classification of a provider type reference.
pub struct ProviderTypeRef {
    /// Final path segment, when one is available.
    pub last_name: Option<SymbolId>,
    /// Nominal identities of positional type arguments, when known.
    pub type_argument_names: Vec<Option<SymbolId>>,
    /// Whether the target is generic or structurally shaped.
    pub is_generic_or_structural_target: bool,
    /// Qualified module path segments before the target name, when explicit.
    pub target_module_segments: Vec<SymbolId>,
    /// Whether an unqualified target resolves to a nominal type in this module.
    pub target_is_local_nominal: bool,
    /// Whether matching must conservatively account for aliases or imports.
    pub semantic_is_conservative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Target portion of an extension provider summary.
pub struct ProviderTarget {
    /// Classified target type reference.
    pub ty: ProviderTypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Candidate category used by nominal provider discovery.
pub enum NominalProviderCandidate {
    /// Provider has a definite nominal name.
    Named(SymbolId),
    /// Provider requires conservative fallback matching.
    Conservative,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// Stable module index keyed by definite nominal provider names.
pub struct NominalProviderCandidateIndex<M> {
    named: SymbolMap<Vec<M>>,
}

impl<M> NominalProviderCandidateIndex<M>
where
    M: Copy + Ord + Eq,
{
    /// Builds an index from module summaries, sorting and deduplicating modules.
    pub fn from_summaries(
        summaries: impl IntoIterator<Item = (M, ProviderSummary)>,
    ) -> NominalProviderCandidateIndex<M> {
        let mut named: SymbolMap<Vec<M>> = SymbolMap::default();
        for (module, summary) in summaries {
            for provider in &summary.providers {
                if let Some(name) = provider.target.ty.nominal_provider_index_name() {
                    named.entry(*name).or_default().push(module);
                }
            }
        }
        for modules in named.values_mut() {
            modules.sort();
            modules.dedup();
        }
        NominalProviderCandidateIndex { named }
    }

    /// Returns modules that may provide the requested nominal name.
    pub fn named(&self, name: &SymbolId) -> &[M] {
        self.named.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Iterates all indexed modules in key order.
    pub fn all_named(&self) -> impl Iterator<Item = M> + '_ {
        self.named
            .values()
            .flat_map(|modules| modules.iter().copied())
    }
}

impl ProviderSummary {
    /// Creates a summary from already collected providers.
    pub fn from_providers(providers: Vec<Provider>) -> Self {
        Self { providers }
    }

    /// Returns providers in source order.
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }

    /// Extracts extension providers from an active item tree.
    pub fn from_active_item_tree(item_tree: &ActiveModuleItemTree) -> Self {
        let mut local_nominal_names = local_nominal_type_names(item_tree);
        local_nominal_names.extend(
            nia_ids::BuiltinTypeAnchor::ALL
                .iter()
                .map(|anchor| anchor.symbol_id()),
        );
        let mut local_trait_names = local_trait_names(item_tree);
        // Imported names can themselves denote aliases or reexports. Only
        // local declarations and language anchors establish an identity
        // without semantic lookup.
        let argument_nominal_names = local_nominal_names.clone();
        local_trait_names.extend(
            nia_ids::BuiltinTrait::ALL
                .iter()
                .map(|trait_id| trait_id.symbol_id()),
        );
        let aliases = item_tree
            .items
            .iter()
            .filter_map(|item| {
                let ItemTreeNodeKind::TypeAlias(alias) = &item.kind else {
                    return None;
                };
                Some((alias.name, alias))
            })
            .collect::<SymbolMap<_>>();
        let providers = item_tree
            .items
            .iter()
            .filter_map(|item| {
                let ItemTreeNodeKind::Extend(extend) = &item.kind else {
                    return None;
                };
                let generic_names = generic_param_names(&extend.generics);
                let associated_methods = extend
                    .methods
                    .iter()
                    .map(|method| method.function.name)
                    .collect();
                let associated_values = extend
                    .associated_values
                    .iter()
                    .map(|value| value.binding.name)
                    .collect();
                Some(Provider {
                    target: ProviderTarget {
                        ty: ProviderTypeRef::from_type_ref(
                            &extend.target,
                            &generic_names,
                            &local_nominal_names,
                            &local_nominal_names,
                            &aliases,
                            &mut SymbolSet::default(),
                        ),
                    },
                    trait_ref: extend.trait_ref.as_ref().map(|trait_ref| {
                        ProviderTypeRef::from_type_ref(
                            trait_ref,
                            &generic_names,
                            &local_trait_names,
                            &argument_nominal_names,
                            &aliases,
                            &mut SymbolSet::default(),
                        )
                    }),
                    associated_methods,
                    associated_values,
                })
            })
            .collect();
        Self { providers }
    }

    /// Reports whether the module contains any extension providers.
    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    /// Reports whether a provider may match a nominal target name.
    pub fn may_define_nominal_provider_for(&self, target_type_name: &SymbolId) -> bool {
        self.providers
            .iter()
            .any(|provider| provider.target.ty.may_match_nominal_name(target_type_name))
    }

    /// Returns deterministic nominal provider candidates for this summary.
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

    /// Returns sorted definite names used by the nominal provider index.
    pub fn nominal_provider_index_names(&self) -> Vec<SymbolId> {
        let mut names = self
            .providers
            .iter()
            .filter_map(|provider| provider.target.ty.nominal_provider_index_name().cloned())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    /// Returns sorted method names declared by providers.
    pub fn method_index_names(&self) -> Vec<SymbolId> {
        let mut names = self
            .providers
            .iter()
            .flat_map(|provider| provider.associated_methods.iter().cloned())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    /// Returns sorted trait names implemented by providers.
    pub fn trait_impl_index_names(&self) -> Vec<SymbolId> {
        let mut names = self
            .providers
            .iter()
            .filter_map(|provider| {
                provider
                    .trait_ref
                    .as_ref()
                    .and_then(|trait_ref| trait_ref.last_name)
            })
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    /// Reports whether an inherent provider may define an associated item.
    pub fn defines_inherent_associated_item(
        &self,
        target_type_name: &SymbolId,
        associated_name: &SymbolId,
    ) -> bool {
        self.providers.iter().any(|provider| {
            provider.trait_ref.is_none()
                && provider.target.ty.may_match_nominal_name(target_type_name)
                && provider.has_associated_item(associated_name)
        })
    }

    /// Reports whether a provider may define a matching trait implementation.
    pub fn defines_trait_impl(
        &self,
        target_type_name: Option<&SymbolId>,
        trait_name: &SymbolId,
        trait_type_argument_names: &[Option<SymbolId>],
        associated_name: Option<&SymbolId>,
    ) -> bool {
        self.defines_trait_impl_at_module(
            target_type_name,
            &[],
            &[],
            trait_name,
            trait_type_argument_names,
            associated_name,
        )
    }

    /// Reports whether a matching implementation may target a type defined in
    /// the given module path. Unqualified summary paths remain conservative.
    pub fn defines_trait_impl_at_module(
        &self,
        target_type_name: Option<&SymbolId>,
        target_module_segments: &[SymbolId],
        provider_module_segments: &[SymbolId],
        trait_name: &SymbolId,
        trait_type_argument_names: &[Option<SymbolId>],
        associated_name: Option<&SymbolId>,
    ) -> bool {
        self.providers.iter().any(|provider| {
            target_type_name.map_or(
                provider.target.ty.is_generic_or_structural_target
                    || provider.target.ty.semantic_is_conservative,
                |target_type_name| {
                    provider
                        .target
                        .ty
                        .may_match_demand_nominal_name(target_type_name)
                },
            ) && {
                let provider_target_module_segments = if provider.target.ty.target_is_local_nominal
                {
                    provider_module_segments
                } else {
                    &provider.target.ty.target_module_segments
                };
                target_module_segments.is_empty()
                    || provider_target_module_segments.is_empty()
                    || provider_target_module_segments == target_module_segments
            } && provider
                .trait_ref
                .as_ref()
                .is_some_and(|trait_ref| trait_ref.may_match_trait_name(trait_name))
                && provider.trait_ref.as_ref().is_some_and(|trait_ref| {
                    trait_ref.may_match_type_arguments(trait_type_argument_names)
                })
                && associated_name.is_none_or(|name| provider.has_associated_item(name))
        })
    }

    /// Reports whether a public extension item is visible through a facade.
    pub fn defines_public_extension_method_for_facade(
        &self,
        facade_exposes_type: impl Fn(&SymbolId) -> bool,
        target_type_name: Option<&SymbolId>,
        associated_name: &SymbolId,
    ) -> bool {
        self.providers.iter().any(|provider| {
            if let Some(target_type_name) = target_type_name {
                if !provider
                    .target
                    .ty
                    .may_match_demand_nominal_name(target_type_name)
                {
                    return false;
                }
            } else if provider.trait_ref.is_none()
                && !provider.target.ty.is_generic_or_structural_target
                && provider.target.ty.is_definite_semantic_name()
            {
                return false;
            }
            if let Some(trait_ref) = &provider.trait_ref {
                let Some(trait_name) = trait_ref.last_name.as_ref() else {
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
    fn has_associated_item(&self, name: &SymbolId) -> bool {
        self.associated_methods
            .iter()
            .chain(self.associated_values.iter())
            .any(|associated| associated == name)
    }
}

impl ProviderTypeRef {
    fn from_type_ref(
        ty: &TypeRef,
        generic_names: &SymbolSet,
        definite_names: &SymbolSet,
        argument_definite_names: &SymbolSet,
        aliases: &SymbolMap<&nia_ast::TypeAliasItem>,
        visiting: &mut SymbolSet,
    ) -> Self {
        if let TypeKind::Path { segments } = &ty.kind
            && segments.len() == 1
            && let PathSegmentKind::Name(name) = segments[0].kind
            && !generic_names.contains(&name)
            && let Some(alias) = aliases.get(&name)
        {
            if visiting.insert(name)
                && let Some(target) = &alias.ty
            {
                let mut generic_names = generic_names.clone();
                generic_names.extend(generic_param_names(&alias.generics));
                let resolved = Self::from_type_ref(
                    target,
                    &generic_names,
                    definite_names,
                    argument_definite_names,
                    aliases,
                    visiting,
                );
                visiting.remove(&name);
                return resolved;
            }
            return Self {
                last_name: None,
                type_argument_names: Vec::new(),
                is_generic_or_structural_target: true,
                semantic_is_conservative: true,
                target_module_segments: Vec::new(),
                target_is_local_nominal: false,
            };
        }
        let last_name = type_ref_last_name(ty);
        let target_module_segments = match &ty.kind {
            TypeKind::Path { segments } => segments
                .iter()
                .take(segments.len().saturating_sub(1))
                .filter_map(|segment| match segment.kind {
                    PathSegmentKind::Name(name) => Some(name),
                    PathSegmentKind::Package
                    | PathSegmentKind::Super
                    | PathSegmentKind::SelfValue => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let is_generic_or_structural_target =
            type_ref_is_generic_or_structural_provider_target(ty, generic_names);
        let target_is_local_nominal = matches!(
            &ty.kind,
            TypeKind::Path { segments }
                if segments.len() == 1
                    && matches!(segments[0].kind, PathSegmentKind::Name(name)
                        if definite_names.contains(&name) && !generic_names.contains(&name))
        );
        let semantic_is_conservative =
            !type_ref_is_definite_local_nominal_name(ty, generic_names, definite_names);
        Self {
            last_name,
            type_argument_names: type_ref_argument_names(
                ty,
                generic_names,
                argument_definite_names,
            ),
            is_generic_or_structural_target,
            semantic_is_conservative,
            target_module_segments,
            target_is_local_nominal,
        }
    }

    fn may_match_nominal_name(&self, name: &SymbolId) -> bool {
        self.is_generic_or_structural_target
            || self.semantic_is_conservative
            || self.last_name.as_ref().is_none_or(|last| last == name)
    }

    fn may_match_demand_nominal_name(&self, name: &SymbolId) -> bool {
        self.is_generic_or_structural_target
            || self.semantic_is_conservative
            || self.last_name.as_ref().is_none_or(|last| last == name)
    }

    fn may_match_trait_name(&self, name: &SymbolId) -> bool {
        self.semantic_is_conservative || self.last_name.as_ref().is_none_or(|last| last == name)
    }

    fn may_match_type_arguments(&self, names: &[Option<SymbolId>]) -> bool {
        if self.type_argument_names.len() != names.len() {
            return true;
        }
        self.type_argument_names
            .iter()
            .zip(names)
            .all(|(provider, demand)| match (provider, demand) {
                (Some(provider), Some(demand)) => provider == demand,
                _ => true,
            })
    }

    fn semantic_nominal_provider_candidate(&self) -> NominalProviderCandidate {
        if self.semantic_is_conservative {
            return NominalProviderCandidate::Conservative;
        }
        self.last_name
            .map(NominalProviderCandidate::Named)
            .unwrap_or(NominalProviderCandidate::Conservative)
    }

    fn nominal_provider_index_name(&self) -> Option<&SymbolId> {
        if self.is_generic_or_structural_target {
            return None;
        }
        self.last_name.as_ref()
    }

    fn is_definite_semantic_name(&self) -> bool {
        !self.semantic_is_conservative
    }
}

fn local_nominal_type_names(item_tree: &ActiveModuleItemTree) -> SymbolSet {
    item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Struct(item) => Some(item.name),
            ItemTreeNodeKind::Union(item) => Some(item.name),
            ItemTreeNodeKind::Enum(item) => Some(item.name),
            _ => None,
        })
        .collect()
}

fn local_trait_names(item_tree: &ActiveModuleItemTree) -> SymbolSet {
    item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Trait(item) => Some(item.name),
            _ => None,
        })
        .collect()
}

fn type_ref_last_name(ty: &TypeRef) -> Option<SymbolId> {
    match &ty.kind {
        TypeKind::Path { segments } => segments.last().and_then(|segment| match segment.kind {
            PathSegmentKind::Name(name) => Some(name),
            PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
        }),
        _ => None,
    }
}

fn type_ref_argument_names(
    ty: &TypeRef,
    generic_names: &SymbolSet,
    definite_names: &SymbolSet,
) -> Vec<Option<SymbolId>> {
    let TypeKind::Path { segments } = &ty.kind else {
        return Vec::new();
    };
    let Some(segment) = segments.last() else {
        return Vec::new();
    };
    segment
        .args
        .iter()
        .filter_map(|argument| match argument {
            TypeArg::Type(ty) | TypeArg::TypeOrConst { ty, .. } => Some(
                if type_ref_is_definite_local_nominal_name(ty, generic_names, definite_names) {
                    type_ref_last_name(ty)
                } else {
                    None
                },
            ),
            TypeArg::Const(_) | TypeArg::AssocBinding { .. } => None,
        })
        .collect()
}

fn type_ref_is_definite_local_nominal_name(
    ty: &TypeRef,
    generic_names: &SymbolSet,
    definite_names: &SymbolSet,
) -> bool {
    match &ty.kind {
        TypeKind::Path { segments } => match segments.as_slice() {
            [segment] => matches!(
                segment.kind,
                PathSegmentKind::Name(name)
                    if !generic_names.contains(&name) && definite_names.contains(&name)
            ),
            [.., segment] => matches!(segment.kind, PathSegmentKind::Name(_)),
            [] => false,
        },
        _ => false,
    }
}

fn generic_param_names(generics: &[GenericParam]) -> SymbolSet {
    generics.iter().map(|generic| generic.name).collect()
}

fn type_ref_is_generic_or_structural_provider_target(
    ty: &TypeRef,
    generic_names: &SymbolSet,
) -> bool {
    match &ty.kind {
        TypeKind::Path { segments } => {
            segments.len() == 1
                && segments[0].args.is_empty()
                && matches!(
                    segments[0].kind,
                    PathSegmentKind::Name(name) if generic_names.contains(&name)
                )
        }
        TypeKind::Pointer { .. }
        | TypeKind::VolatilePointer { .. }
        | TypeKind::Slice { .. }
        | TypeKind::SlicePointee { .. }
        | TypeKind::Array { .. }
        | TypeKind::Tuple { .. }
        | TypeKind::Range { .. }
        | TypeKind::FunctionPointer { .. }
        | TypeKind::Callable { .. }
        | TypeKind::Optional { .. }
        | TypeKind::ErrorUnion { .. }
        | TypeKind::SelfType
        | TypeKind::Infer => true,
        TypeKind::Error | TypeKind::Projection { .. } | TypeKind::Opaque | TypeKind::Never => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;
    use nia_symbol::stable_hash;

    fn sym(text: &str) -> SymbolId {
        SymbolId::from_stable_hash(stable_hash(text))
    }

    #[test]
    fn summarizes_inherent_associated_items() {
        let summary = summary_for(
            r#"
struct Widget {}

extend Widget {
    pub fn score(&self) i32 { 1 }
    pub const Kind: i32 = 0;
}
"#,
        );

        assert!(summary.has_providers());
        assert!(summary.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
        assert!(summary.defines_inherent_associated_item(&sym("Widget"), &sym("Kind")));
        assert!(!summary.defines_inherent_associated_item(&sym("Widget"), &sym("missing")));
        assert_eq!(summary.method_index_names(), vec![sym("score")]);
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

        assert!(summary.defines_trait_impl(Some(&sym("Widget")), &sym("Hash"), &[], None));
        assert!(summary.defines_trait_impl(
            Some(&sym("Widget")),
            &sym("Hash"),
            &[],
            Some(&sym("hash"))
        ));
        assert!(!summary.defines_trait_impl(Some(&sym("Other")), &sym("Hash"), &[], None));
        assert!(!summary.defines_trait_impl(
            Some(&sym("Widget")),
            &sym("Hash"),
            &[],
            Some(&sym("finish")),
        ));
        assert_eq!(summary.trait_impl_index_names(), vec![sym("Hash")]);
    }

    #[test]
    fn nominal_provider_summary_keeps_generic_targets_conservative() {
        let summary = summary_for(
            r#"
trait IntoError[Target] {
    fn intoError(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        match self {
            !ok => {
                !ok
            },
            error! => {
                error.intoError()!
            },
        }
    }
}
"#,
        );

        assert!(summary.may_define_nominal_provider_for(&sym("Error")));
    }

    #[test]
    fn trait_provider_summary_matches_structural_target_without_a_name() {
        let summary = summary_for(
            r#"
trait Iterable {}
extend[T] T..T : Iterable {}
"#,
        );
        assert!(summary.defines_trait_impl(None, &sym("Iterable"), &[], None));
    }

    #[test]
    fn trait_provider_summary_resolves_local_alias_targets() {
        let summary = summary_for(
            r#"
struct Widget {}
type Alias = Widget;
trait Show {}
extend Alias : Show {}
"#,
        );
        assert!(summary.defines_trait_impl(Some(&sym("Widget")), &sym("Show"), &[], None));
        assert!(!summary.defines_trait_impl(Some(&sym("Other")), &sym("Show"), &[], None));
        assert!(!summary.defines_trait_impl(None, &sym("Show"), &[], None));
    }

    #[test]
    fn trait_provider_summary_resolves_local_trait_aliases() {
        let summary = summary_for(
            r#"
struct Widget {}
trait Show {}
type Alias = Show;
extend Widget : Alias {}
"#,
        );
        assert!(summary.defines_trait_impl(Some(&sym("Widget")), &sym("Show"), &[], None));
        assert!(!summary.defines_trait_impl(Some(&sym("Widget")), &sym("Other"), &[], None));
    }

    #[test]
    fn trait_provider_summary_follows_alias_chains_and_structural_aliases() {
        let summary = summary_for(
            r#"
struct Widget[T] {}
trait Show {}
type First[T] = Widget[T];
type Second[T] = First[T];
type TraitAlias = Show;
type TraitChain = TraitAlias;
type Values[T] = [T];
extend[T] Second[T] : TraitChain {}
extend[T] Values[T] : Show {}
"#,
        );
        assert!(summary.defines_trait_impl(Some(&sym("Widget")), &sym("Show"), &[], None));
        assert!(summary.defines_trait_impl(None, &sym("Show"), &[], None));
        assert!(!summary.defines_trait_impl(Some(&sym("Widget")), &sym("Other"), &[], None));
        assert_eq!(
            summary.providers()[0].target.ty.last_name,
            Some(sym("Widget"))
        );
    }

    #[test]
    fn trait_provider_summary_alias_cycles_terminate_conservatively() {
        let summary = summary_for("type A = B; type B = A; trait Show {} extend A : Show {}");
        assert!(summary.defines_trait_impl(None, &sym("Show"), &[], None));
        assert!(summary.defines_trait_impl(Some(&sym("Widget")), &sym("Show"), &[], None));
    }

    #[test]
    fn trait_provider_summary_keeps_qualified_alias_candidates() {
        let summary = summary_for(
            "using dep::types; using dep::traits; extend types::Widget : traits::Show {}",
        );
        assert!(summary.defines_trait_impl(Some(&sym("Widget")), &sym("Show"), &[], None));
        // Qualified paths retain their terminal identity; only imported
        // single-segment aliases remain conservative.
        assert!(!summary.defines_trait_impl(None, &sym("Show"), &[], None));
        assert!(!summary.defines_trait_impl(Some(&sym("Other")), &sym("Show"), &[], None));
        assert!(!summary.defines_trait_impl(Some(&sym("Widget")), &sym("Other"), &[], None));
    }

    #[test]
    fn unnamed_structural_demand_does_not_select_nominal_generic_providers() {
        let summary = summary_for(
            r#"
trait Iterator {}
struct Cursor[T] {}
extend[T] Cursor[T] : Iterator {}
"#,
        );

        assert!(!summary.defines_trait_impl(None, &sym("Iterator"), &[], None));
        assert!(summary.defines_trait_impl(Some(&sym("Cursor")), &sym("Iterator"), &[], None));
        assert!(!summary.defines_trait_impl(Some(&sym("Other")), &sym("Iterator"), &[], None));
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

        assert!(summary.may_define_nominal_provider_for(&sym("Used")));
        assert!(!summary.may_define_nominal_provider_for(&sym("Other")));
        assert_eq!(
            summary.nominal_provider_candidates(),
            vec![NominalProviderCandidate::Named(sym("Used"))]
        );
    }

    #[test]
    fn nominal_provider_summary_keeps_imported_alias_targets() {
        let summary = summary_for(
            r#"
module types;
using types::{Used};

extend Used {
    pub fn init() Used {
        Used {}
    }
}
"#,
        );

        assert!(summary.defines_inherent_associated_item(&sym("Used"), &sym("init")));
        assert!(summary.defines_inherent_associated_item(&sym("Other"), &sym("init")));
    }

    #[test]
    fn public_extension_summary_keeps_qualified_inherent_targets_for_unknown_receivers() {
        let summary = summary_for(
            r#"
module fs;
using fs;

extend fs::File {
    pub fn writer(&self) () {}
}
"#,
        );

        assert!(!summary.defines_public_extension_method_for_facade(
            |_| false,
            None,
            &sym("writer")
        ));
        assert!(!summary.defines_public_extension_method_for_facade(
            |_| false,
            None,
            &sym("reader")
        ));
    }

    #[test]
    fn nominal_provider_summary_indexes_resolved_local_alias_targets() {
        let summary = summary_for(
            r#"
struct Used {}
type Alias = Used;

extend Alias {
    pub fn len(&self) i32 { 1 }
}
"#,
        );

        assert!(summary.may_define_nominal_provider_for(&sym("Used")));
        assert!(!summary.may_define_nominal_provider_for(&sym("Other")));
        assert_eq!(
            summary.nominal_provider_candidates(),
            vec![NominalProviderCandidate::Named(sym("Used"))]
        );
        let index = NominalProviderCandidateIndex::from_summaries([(0usize, summary)]);
        assert!(index.named(&sym("Alias")).is_empty());
        assert_eq!(index.named(&sym("Used")), &[0usize]);
    }

    #[test]
    fn nominal_provider_index_skips_structural_targets() {
        let summary = summary_for(
            r#"
extend[T] [T] {
    fn size(&self) usize { 0usize }
}

extend[T] &T {
    fn ptr(self) bool { true }
}

extend () {
    fn unit(self) i32 { 1 }
}
"#,
        );

        assert!(summary.may_define_nominal_provider_for(&sym("Used")));
        assert_eq!(
            summary.nominal_provider_candidates(),
            vec![NominalProviderCandidate::Conservative]
        );
        let index = NominalProviderCandidateIndex::from_summaries([(0usize, summary)]);
        assert!(index.all_named().next().is_none());
    }

    #[test]
    fn nominal_provider_index_skips_generic_parameter_targets() {
        let summary = summary_for(
            r#"
extend[T] T {
    fn rank(self) i32 { 1 }
}
"#,
        );

        assert!(summary.may_define_nominal_provider_for(&sym("Used")));
        assert_eq!(
            summary.nominal_provider_candidates(),
            vec![NominalProviderCandidate::Conservative]
        );
        let index = NominalProviderCandidateIndex::from_summaries([(0usize, summary)]);
        assert!(index.named(&sym("T")).is_empty());
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
            |name| *name == sym("Hash"),
            Some(&sym("Widget")),
            &sym("hash"),
        ));
        assert!(!summary.defines_public_extension_method_for_facade(
            |_| false,
            Some(&sym("Widget")),
            &sym("hash"),
        ));
    }

    #[test]
    fn trait_impl_provider_summary_uses_qualified_trait_last_name() {
        let summary = summary_for(
            r#"
module error;
using error;

struct SpawnError {}

extend SpawnError : error::IntoError {
    fn intoError(self) () {}
}
"#,
        );

        assert!(
            summary.defines_trait_impl(Some(&sym("SpawnError")), &sym("IntoError"), &[], None,)
        );
        assert!(!summary.defines_trait_impl(None, &sym("Iterable"), &[], None));
    }

    #[test]
    fn local_nominal_trait_targets_are_anchored_to_the_provider_module() {
        let summary = summary_for(
            r#"
struct Error {}
trait IntoError {}
extend Error : IntoError {}
"#,
        );
        assert!(summary.defines_trait_impl_at_module(
            Some(&sym("Error")),
            &[sym("process"), sym("types")],
            &[sym("process"), sym("types")],
            &sym("IntoError"),
            &[],
            None,
        ));
        assert!(!summary.defines_trait_impl_at_module(
            Some(&sym("Error")),
            &[sym("os")],
            &[sym("process"), sym("types")],
            &sym("IntoError"),
            &[],
            None,
        ));
    }

    #[test]
    fn trait_impl_provider_summary_distinguishes_nominal_trait_arguments() {
        let summary = summary_for(
            r#"
struct SourceError {}
struct ExitCode {}
trait IntoError[Target] {}

extend SourceError : IntoError[ExitCode] {}
"#,
        );

        assert_eq!(
            summary.providers()[0]
                .trait_ref
                .as_ref()
                .map(|trait_ref| trait_ref.type_argument_names.as_slice()),
            Some([Some(sym("ExitCode"))].as_slice()),
        );
        assert!(summary.defines_trait_impl(
            Some(&sym("SourceError")),
            &sym("IntoError"),
            &[Some(sym("ExitCode"))],
            None,
        ));
        assert!(!summary.defines_trait_impl(
            Some(&sym("SourceError")),
            &sym("IntoError"),
            &[Some(sym("OtherError"))],
            None,
        ));
        assert!(summary.defines_trait_impl(
            Some(&sym("SourceError")),
            &sym("IntoError"),
            &[None],
            None,
        ));

        let generic = summary_for(
            r#"
struct SourceError {}
trait IntoError[Target] {}

extend[Target] SourceError : IntoError[Target] {}
"#,
        );
        assert!(generic.defines_trait_impl(
            Some(&sym("SourceError")),
            &sym("IntoError"),
            &[Some(sym("AnyConcreteError"))],
            None,
        ));

        let aliased = summary_for(
            r#"
module types;
using types::{ExitCode as Code};
struct SourceError {}
trait IntoError[Target] {}

extend SourceError : IntoError[Code] {}
"#,
        );
        assert!(aliased.defines_trait_impl(
            Some(&sym("SourceError")),
            &sym("IntoError"),
            &[Some(sym("ExitCode"))],
            None,
        ));
    }

    fn summary_for(source: &str) -> ProviderSummary {
        let (module, errors) = parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        let item_tree = ModuleItemTree::from_module(&module);
        let active = item_tree.all_items_active();
        ProviderSummary::from_active_item_tree(&active)
    }
}
