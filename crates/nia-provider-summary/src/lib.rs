// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{TypeKind, TypeRef};
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTarget {
    pub ty: ProviderTypeRef,
}

impl ProviderSummary {
    pub fn from_active_item_tree(item_tree: &ActiveModuleItemTree) -> Self {
        let providers = item_tree
            .items
            .iter()
            .filter_map(|item| {
                let ItemTreeNodeKind::Extend(extend) = &item.kind else {
                    return None;
                };
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
                        ty: ProviderTypeRef::from_type_ref(&extend.target),
                    },
                    trait_ref: extend
                        .trait_ref
                        .as_ref()
                        .map(ProviderTypeRef::from_type_ref),
                    associated_items,
                })
            })
            .collect();
        Self { providers }
    }

    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    pub fn defines_inherent_associated_item(
        &self,
        target_type_name: &str,
        associated_name: &str,
    ) -> bool {
        self.providers.iter().any(|provider| {
            provider.trait_ref.is_none()
                && provider.target.ty.ends_with_name(target_type_name)
                && provider.has_associated_item(associated_name)
        })
    }

    pub fn defines_trait_impl(&self, trait_name: &str, associated_name: Option<&str>) -> bool {
        self.providers.iter().any(|provider| {
            provider
                .trait_ref
                .as_ref()
                .is_some_and(|trait_ref| trait_ref.ends_with_name(trait_name))
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
                if !provider.target.ty.ends_with_name(target_type_name) {
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
                if !facade_exposes_type(trait_name) {
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
    fn from_type_ref(ty: &TypeRef) -> Self {
        Self {
            last_name: type_ref_last_name(ty).map(ToString::to_string),
            is_generic_or_structural_target: type_ref_is_generic_or_structural_provider_target(ty),
        }
    }

    fn ends_with_name(&self, name: &str) -> bool {
        self.last_name.as_deref().is_some_and(|last| last == name)
    }
}

fn type_ref_last_name(ty: &TypeRef) -> Option<&str> {
    match &ty.kind {
        TypeKind::Path { segments } => segments.last().map(|segment| segment.name.as_str()),
        _ => None,
    }
}

fn type_ref_is_generic_or_structural_provider_target(ty: &TypeRef) -> bool {
    match &ty.kind {
        TypeKind::Path { segments } => segments.len() == 1 && segments[0].args.is_empty(),
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
