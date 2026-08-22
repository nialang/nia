// SPDX-License-Identifier: GPL-3.0-or-later
//! Declarative optimization policies shared by compiler and backend passes.
//!
//! This crate selects pass depth, inlining thresholds, generic specialization,
//! and size preference. It does not implement an optimization pass itself.

/// User-facing optimization level mapped to a complete pass policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NiaOptimizationLevel {
    #[default]
    /// Required correctness work only.
    O0,
    /// Cheap local improvements.
    O1,
    /// Full normal optimization.
    O2,
    /// Aggressive performance optimization.
    O3,
    /// Full optimization biased toward code size.
    Os,
    /// Minimal code-size-oriented optimization.
    Oz,
}

impl NiaOptimizationLevel {
    /// Expands this level into the policy consumed by optimization passes.
    pub fn policy(self) -> OptimizationPolicy {
        match self {
            Self::O0 => OptimizationPolicy {
                level: self,
                simplify_cfg: OptimizationDepth::Required,
                const_fold: OptimizationDepth::Required,
                dead_code_elim: OptimizationDepth::Disabled,
                local_copy_prop: OptimizationDepth::Disabled,
                inline_threshold: InlineThreshold::Never,
                specialize_generics: SpecializationPolicy::RequiredOnly,
                dedup_monomorphized_instances: true,
                prefer_size: false,
            },
            Self::O1 => OptimizationPolicy {
                level: self,
                simplify_cfg: OptimizationDepth::Cheap,
                const_fold: OptimizationDepth::Cheap,
                dead_code_elim: OptimizationDepth::Cheap,
                local_copy_prop: OptimizationDepth::Cheap,
                inline_threshold: InlineThreshold::Small,
                specialize_generics: SpecializationPolicy::RequiredOnly,
                dedup_monomorphized_instances: true,
                prefer_size: false,
            },
            Self::O2 => OptimizationPolicy {
                level: self,
                simplify_cfg: OptimizationDepth::Full,
                const_fold: OptimizationDepth::Full,
                dead_code_elim: OptimizationDepth::Full,
                local_copy_prop: OptimizationDepth::Full,
                inline_threshold: InlineThreshold::Normal,
                specialize_generics: SpecializationPolicy::Normal,
                dedup_monomorphized_instances: true,
                prefer_size: false,
            },
            Self::O3 => OptimizationPolicy {
                level: self,
                simplify_cfg: OptimizationDepth::Aggressive,
                const_fold: OptimizationDepth::Aggressive,
                dead_code_elim: OptimizationDepth::Aggressive,
                local_copy_prop: OptimizationDepth::Aggressive,
                inline_threshold: InlineThreshold::Aggressive,
                specialize_generics: SpecializationPolicy::Aggressive,
                dedup_monomorphized_instances: true,
                prefer_size: false,
            },
            Self::Os => OptimizationPolicy {
                level: self,
                simplify_cfg: OptimizationDepth::Full,
                const_fold: OptimizationDepth::Full,
                dead_code_elim: OptimizationDepth::Full,
                local_copy_prop: OptimizationDepth::Full,
                inline_threshold: InlineThreshold::Size,
                specialize_generics: SpecializationPolicy::SizeAware,
                dedup_monomorphized_instances: true,
                prefer_size: true,
            },
            Self::Oz => OptimizationPolicy {
                level: self,
                simplify_cfg: OptimizationDepth::Full,
                const_fold: OptimizationDepth::Full,
                dead_code_elim: OptimizationDepth::Full,
                local_copy_prop: OptimizationDepth::Cheap,
                inline_threshold: InlineThreshold::Minimal,
                specialize_generics: SpecializationPolicy::RequiredOnly,
                dedup_monomorphized_instances: true,
                prefer_size: true,
            },
        }
    }
}

/// Complete optimization policy for one compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptimizationPolicy {
    /// Source optimization level.
    pub level: NiaOptimizationLevel,
    /// Control-flow simplification depth.
    pub simplify_cfg: OptimizationDepth,
    /// Constant-folding depth.
    pub const_fold: OptimizationDepth,
    /// Dead-code elimination depth.
    pub dead_code_elim: OptimizationDepth,
    /// Local copy-propagation depth.
    pub local_copy_prop: OptimizationDepth,
    /// Function inlining threshold.
    pub inline_threshold: InlineThreshold,
    /// Generic specialization policy.
    pub specialize_generics: SpecializationPolicy,
    /// Whether equivalent monomorphized instances are deduplicated.
    pub dedup_monomorphized_instances: bool,
    /// Whether size should be preferred over speed.
    pub prefer_size: bool,
}

impl Default for OptimizationPolicy {
    fn default() -> Self {
        NiaOptimizationLevel::default().policy()
    }
}

/// Depth of an individual optimization family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptimizationDepth {
    /// Pass is disabled.
    Disabled,
    /// Passes required for correctness.
    Required,
    /// Bounded low-cost pass work.
    Cheap,
    /// Complete normal pass work.
    Full,
    /// Maximum pass work.
    Aggressive,
}

impl OptimizationDepth {
    /// Returns whether this depth meets `minimum`.
    pub fn at_least(self, minimum: Self) -> bool {
        self.rank() >= minimum.rank()
    }

    fn rank(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::Required => 1,
            Self::Cheap => 2,
            Self::Full => 3,
            Self::Aggressive => 4,
        }
    }
}

/// Relative threshold used by function inlining.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineThreshold {
    /// Never inline.
    Never,
    /// Inline only the smallest bodies.
    Minimal,
    /// Inline with code-size preference.
    Size,
    /// Inline small bodies.
    Small,
    /// Inline normal candidates.
    Normal,
    /// Inline aggressively.
    Aggressive,
}

/// Generic specialization aggressiveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecializationPolicy {
    /// Specialize only semantically required instances.
    RequiredOnly,
    /// Specialize with code-size awareness.
    SizeAware,
    /// Normal specialization.
    Normal,
    /// Aggressive specialization.
    Aggressive,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimization_levels_expand_to_expected_policy_matrix() {
        let expected = [
            (
                NiaOptimizationLevel::O0,
                OptimizationPolicy {
                    level: NiaOptimizationLevel::O0,
                    simplify_cfg: OptimizationDepth::Required,
                    const_fold: OptimizationDepth::Required,
                    dead_code_elim: OptimizationDepth::Disabled,
                    local_copy_prop: OptimizationDepth::Disabled,
                    inline_threshold: InlineThreshold::Never,
                    specialize_generics: SpecializationPolicy::RequiredOnly,
                    dedup_monomorphized_instances: true,
                    prefer_size: false,
                },
            ),
            (
                NiaOptimizationLevel::O1,
                OptimizationPolicy {
                    level: NiaOptimizationLevel::O1,
                    simplify_cfg: OptimizationDepth::Cheap,
                    const_fold: OptimizationDepth::Cheap,
                    dead_code_elim: OptimizationDepth::Cheap,
                    local_copy_prop: OptimizationDepth::Cheap,
                    inline_threshold: InlineThreshold::Small,
                    specialize_generics: SpecializationPolicy::RequiredOnly,
                    dedup_monomorphized_instances: true,
                    prefer_size: false,
                },
            ),
            (
                NiaOptimizationLevel::O2,
                OptimizationPolicy {
                    level: NiaOptimizationLevel::O2,
                    simplify_cfg: OptimizationDepth::Full,
                    const_fold: OptimizationDepth::Full,
                    dead_code_elim: OptimizationDepth::Full,
                    local_copy_prop: OptimizationDepth::Full,
                    inline_threshold: InlineThreshold::Normal,
                    specialize_generics: SpecializationPolicy::Normal,
                    dedup_monomorphized_instances: true,
                    prefer_size: false,
                },
            ),
            (
                NiaOptimizationLevel::O3,
                OptimizationPolicy {
                    level: NiaOptimizationLevel::O3,
                    simplify_cfg: OptimizationDepth::Aggressive,
                    const_fold: OptimizationDepth::Aggressive,
                    dead_code_elim: OptimizationDepth::Aggressive,
                    local_copy_prop: OptimizationDepth::Aggressive,
                    inline_threshold: InlineThreshold::Aggressive,
                    specialize_generics: SpecializationPolicy::Aggressive,
                    dedup_monomorphized_instances: true,
                    prefer_size: false,
                },
            ),
            (
                NiaOptimizationLevel::Os,
                OptimizationPolicy {
                    level: NiaOptimizationLevel::Os,
                    simplify_cfg: OptimizationDepth::Full,
                    const_fold: OptimizationDepth::Full,
                    dead_code_elim: OptimizationDepth::Full,
                    local_copy_prop: OptimizationDepth::Full,
                    inline_threshold: InlineThreshold::Size,
                    specialize_generics: SpecializationPolicy::SizeAware,
                    dedup_monomorphized_instances: true,
                    prefer_size: true,
                },
            ),
            (
                NiaOptimizationLevel::Oz,
                OptimizationPolicy {
                    level: NiaOptimizationLevel::Oz,
                    simplify_cfg: OptimizationDepth::Full,
                    const_fold: OptimizationDepth::Full,
                    dead_code_elim: OptimizationDepth::Full,
                    local_copy_prop: OptimizationDepth::Cheap,
                    inline_threshold: InlineThreshold::Minimal,
                    specialize_generics: SpecializationPolicy::RequiredOnly,
                    dedup_monomorphized_instances: true,
                    prefer_size: true,
                },
            ),
        ];

        for (level, policy) in expected {
            assert_eq!(level.policy(), policy, "{level:?}");
        }
    }

    #[test]
    fn size_levels_prefer_size_without_aggressive_inlining() {
        let os = NiaOptimizationLevel::Os.policy();
        let oz = NiaOptimizationLevel::Oz.policy();

        assert!(os.prefer_size);
        assert!(oz.prefer_size);
        assert_eq!(os.inline_threshold, InlineThreshold::Size);
        assert_eq!(oz.inline_threshold, InlineThreshold::Minimal);
        assert_eq!(oz.specialize_generics, SpecializationPolicy::RequiredOnly);
    }

    #[test]
    fn o3_enables_aggressive_policy() {
        let policy = NiaOptimizationLevel::O3.policy();

        assert_eq!(policy.simplify_cfg, OptimizationDepth::Aggressive);
        assert_eq!(policy.inline_threshold, InlineThreshold::Aggressive);
        assert_eq!(policy.specialize_generics, SpecializationPolicy::Aggressive);
    }
}
