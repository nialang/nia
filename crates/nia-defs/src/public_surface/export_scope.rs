// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{AccessRestriction, ModuleId, Visibility};

/// Alternative export routes, each bounded by the selected name and its re-exports.
/// A re-export can narrow access but cannot erase an earlier restriction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportScope {
    routes: Vec<Vec<AccessRestriction>>,
}

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
}
