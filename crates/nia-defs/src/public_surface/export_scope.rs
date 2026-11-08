// SPDX-License-Identifier: GPL-3.0-or-later
use super::ExportDenial;
use nia_ids::{AccessRestriction, ModuleId, Visibility};

/// Alternative export routes, each bounded by the selected name and its re-exports.
/// A re-export can narrow access but cannot erase an earlier restriction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportScope {
    routes: Vec<Vec<AccessRestriction>>,
}

/// Rule identifiers shared by the Rust contract tests and the Lean model.
/// Keeping this inventory beside the implementation makes a new algebra rule
/// require an explicit update to both verification sides.
pub const SCOPED_EXPORT_RULE_NAMES: &[&str] = &[
    "alternate_routes",
    "restriction_preserved",
    "intersection_requires_both",
    "subsumed_route",
    "enum_variant_scope",
    "namespace_member_separation",
    "provider_publication_separation",
    "declaration_access_kind",
    "guarded_provider_reachability",
    "export_access_classification",
    "provider_scope_projection",
];

impl Default for ExportScope {
    fn default() -> Self {
        Self {
            routes: vec![Vec::new()],
        }
    }
}

impl ExportScope {
    /// A directly declared export with its original visibility owner.
    pub fn declaration(module: ModuleId, visibility: Visibility) -> Self {
        Self::default().restricted(module, visibility)
    }

    /// A directly declared module name uses the module access rule.
    pub fn module_declaration(module: ModuleId, visibility: Visibility) -> Self {
        Self::default().with_restriction(AccessRestriction::Module(module, visibility))
    }

    /// Adds an item or directive restriction while retaining its declaring module.
    pub fn restricted(self, module: ModuleId, visibility: Visibility) -> Self {
        self.with_restriction(AccessRestriction::Item(module, visibility))
    }

    fn with_restriction(self, restriction: AccessRestriction) -> Self {
        let visibility = match restriction {
            AccessRestriction::Item(_, visibility) | AccessRestriction::Module(_, visibility) => {
                visibility
            }
        };
        if visibility == Visibility::Public {
            return self;
        }
        let mut result = Self { routes: Vec::new() };
        for mut route in self.routes {
            if !route.contains(&restriction) {
                route.push(restriction);
                route.sort_by_key(|restriction| restriction.ordering_key());
            }
            result.merge(Self {
                routes: vec![route],
            });
        }
        result
    }

    /// Whether any complete route permits access by the current caller.
    pub fn allows(&self, allows: &dyn Fn(AccessRestriction) -> bool) -> bool {
        self.routes
            .iter()
            .any(|route| route.iter().all(|&restriction| allows(restriction)))
    }

    /// Checks every route, retaining the failed conditions only on denial.
    pub fn check_access(
        &self,
        allows: &dyn Fn(AccessRestriction) -> bool,
    ) -> Result<(), ExportDenial> {
        if self.allows(allows) {
            return Ok(());
        }
        Err(ExportDenial {
            blocked_routes: self
                .routes
                .iter()
                .map(|route| {
                    route
                        .iter()
                        .copied()
                        .filter(|&restriction| !allows(restriction))
                        .collect()
                })
                .collect(),
        })
    }

    /// Publishing a module's extensions does not publish its namespace.
    /// Namespace declaration restrictions therefore do not constrain this
    /// capability, but every earlier re-export directive still does.
    pub fn provider_publication(&self) -> Self {
        let mut result = Self { routes: Vec::new() };
        for route in &self.routes {
            result.merge(Self {
                routes: vec![
                    route
                        .iter()
                        .copied()
                        .filter(|restriction| matches!(restriction, AccessRestriction::Item(..)))
                        .collect(),
                ],
            });
        }
        result
    }

    /// Requires both scopes, combining their alternative routes.
    pub fn intersect(&self, other: &Self) -> Self {
        let mut result = Self { routes: Vec::new() };
        for left in &self.routes {
            for right in &other.routes {
                let mut route = Self {
                    routes: vec![left.clone()],
                };
                for &restriction in right {
                    route = route.with_restriction(restriction);
                }
                result.merge(route);
            }
        }
        result
    }

    /// Adds an alternative route, discarding routes subsumed by broader ones.
    pub fn merge(&mut self, other: Self) -> bool {
        let mut changed = false;
        for route in other.routes {
            if self.routes.iter().any(|existing| {
                existing
                    .iter()
                    .all(|restriction| route.contains(restriction))
            }) {
                continue;
            }
            self.routes.retain(|existing| {
                !route
                    .iter()
                    .all(|restriction| existing.contains(restriction))
            });
            self.routes.push(route);
            self.routes.sort_by(|left, right| {
                left.iter()
                    .map(|restriction| restriction.ordering_key())
                    .cmp(right.iter().map(|restriction| restriction.ordering_key()))
            });
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_two_restriction_scopes_preserve_the_access_algebra() {
        let allocator = nia_ids::ModuleIdAllocator::new().unwrap();
        let owners = [allocator.allocate().unwrap(), allocator.allocate().unwrap()];
        let scopes = (0u8..16)
            .map(|routes| {
                let mut scope = ExportScope { routes: Vec::new() };
                for mask in 0u8..4 {
                    if routes & (1 << mask) == 0 {
                        continue;
                    }
                    let mut route = ExportScope::default();
                    for (index, owner) in owners.iter().enumerate() {
                        if mask & (1 << index) != 0 {
                            route = route.restricted(*owner, Visibility::PublicRoot);
                        }
                    }
                    scope.merge(route);
                }
                scope
            })
            .collect::<Vec<_>>();
        for left in &scopes {
            for right in &scopes {
                let mut union = left.clone();
                union.merge(right.clone());
                let mut reverse = right.clone();
                reverse.merge(left.clone());
                assert_eq!(union, reverse);
                let intersection = left.intersect(right);
                assert_eq!(intersection, right.intersect(left));
                for grants in 0u8..4 {
                    let grant = |restriction: AccessRestriction| {
                        let (owner, _, _) = restriction.ordering_key();
                        owners.iter().enumerate().any(|(index, candidate)| {
                            *candidate == owner && grants & (1 << index) != 0
                        })
                    };
                    assert_eq!(
                        union.allows(&grant),
                        left.allows(&grant) || right.allows(&grant)
                    );
                    assert_eq!(
                        intersection.allows(&grant),
                        left.allows(&grant) && right.allows(&grant)
                    );
                    for present in [false, true] {
                        let lookup = super::super::ExportLookup::with_access(
                            present.then_some(7),
                            || intersection.clone(),
                            &grant,
                        );
                        match lookup {
                            super::super::ExportLookup::Missing => assert!(!present),
                            super::super::ExportLookup::Visible(value) => {
                                assert_eq!(value, 7);
                                assert!(present && intersection.allows(&grant));
                            }
                            super::super::ExportLookup::Inaccessible(denial) => {
                                assert!(present && !intersection.allows(&grant));
                                assert_eq!(denial.blocked_routes.len(), intersection.routes.len());
                                for (blocked, route) in
                                    denial.blocked_routes.iter().zip(&intersection.routes)
                                {
                                    assert!(!blocked.is_empty());
                                    assert_eq!(
                                        *blocked,
                                        route
                                            .iter()
                                            .copied()
                                            .filter(|&r| !grant(r))
                                            .collect::<Vec<_>>()
                                    );
                                }
                            }
                        }
                    }
                    let restricted = union.clone().restricted(owners[0], Visibility::PublicRoot);
                    assert_eq!(
                        restricted.allows(&grant),
                        union.allows(&grant) && grants & 1 != 0
                    );
                    assert_eq!(
                        restricted,
                        restricted
                            .clone()
                            .restricted(owners[0], Visibility::PublicRoot)
                    );
                }
            }
        }
    }

    #[test]
    fn export_scope_routes_are_canonical_and_preserve_union_and_intersection() {
        let allocator = nia_ids::ModuleIdAllocator::new().unwrap();
        let owner = allocator.allocate().unwrap();
        let other = allocator.allocate().unwrap();
        let a = ExportScope::declaration(owner, Visibility::PublicRoot);
        let b = ExportScope::declaration(other, Visibility::PublicSuper);
        let ab = a.clone().restricted(other, Visibility::PublicSuper);
        let ba = b.clone().restricted(owner, Visibility::PublicRoot);
        assert_eq!(ab, ba);
        let mut union = a.clone();
        assert!(union.merge(b.clone()));
        let mut reverse = b;
        assert!(reverse.merge(a.clone()));
        assert_eq!(union, reverse);
        assert!(!union.merge(ab.clone()));
        assert!(!union.merge(union.clone()));
        for mask in 0u8..4 {
            let grants = |restriction| {
                let AccessRestriction::Item(module, visibility) = restriction else {
                    return false;
                };
                (module == owner && visibility == Visibility::PublicRoot && mask & 1 != 0)
                    || (module == other && visibility == Visibility::PublicSuper && mask & 2 != 0)
            };
            assert_eq!(union.allows(&grants), mask != 0);
            assert_eq!(ab.allows(&grants), mask == 3);
        }
        assert!(union.merge(ExportScope::default()));
        assert_eq!(union, ExportScope::default());
        assert!(!union.merge(a));
    }

    #[test]
    fn provider_projection_preserves_directive_limits_and_drops_namespace_limits() {
        let allocator = nia_ids::ModuleIdAllocator::new().unwrap();
        let owner = allocator.allocate().unwrap();
        let relay = allocator.allocate().unwrap();
        let scope = ExportScope::module_declaration(owner, Visibility::Private)
            .restricted(relay, Visibility::PublicRoot);
        assert!(!scope.allows(&|r| matches!(r, AccessRestriction::Item(..))));
        assert_eq!(
            scope.provider_publication(),
            ExportScope::declaration(relay, Visibility::PublicRoot)
        );
        assert!(!scope.provider_publication().allows(&|_| false));
        assert!(
            ExportScope::module_declaration(owner, Visibility::Private)
                .provider_publication()
                .allows(&|_| false)
        );
        assert!(matches!(
            super::super::ExportLookup::<()>::with_access(
                None,
                || panic!("missing name must not query scope"),
                &|_| panic!("missing name must not check access")
            ),
            super::super::ExportLookup::Missing
        ));
    }
}
