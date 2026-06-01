// SPDX-License-Identifier: GPL-3.0-or-later

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NiaOptimizationLevel {
    #[default]
    O0,
    O1,
    O2,
    O3,
    Os,
    Oz,
}

impl NiaOptimizationLevel {
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
                dedup_monomorphized_instances: false,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptimizationPolicy {
    pub level: NiaOptimizationLevel,
    pub simplify_cfg: OptimizationDepth,
    pub const_fold: OptimizationDepth,
    pub dead_code_elim: OptimizationDepth,
    pub local_copy_prop: OptimizationDepth,
    pub inline_threshold: InlineThreshold,
    pub specialize_generics: SpecializationPolicy,
    pub dedup_monomorphized_instances: bool,
    pub prefer_size: bool,
}

impl Default for OptimizationPolicy {
    fn default() -> Self {
        NiaOptimizationLevel::default().policy()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptimizationDepth {
    Disabled,
    Required,
    Cheap,
    Full,
    Aggressive,
}

impl OptimizationDepth {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineThreshold {
    Never,
    Minimal,
    Size,
    Small,
    Normal,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecializationPolicy {
    RequiredOnly,
    SizeAware,
    Normal,
    Aggressive,
}

#[cfg(test)]
mod tests {
    use super::*;

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
