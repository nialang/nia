// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed compilation targets.
//!
//! This crate is a dependency-free leaf so that build plans, linkers, and
//! code generation share the target model without the frontend.
//!
//! A target is the platform that generated code runs on. Its architecture and
//! operating system are the only independent choices in the maintained
//! matrix; vendor, environment, pointer width, byte order, object format,
//! minimum OS version and LLVM triple all derive from them, so no consumer can
//! combine inconsistent strings. The baseline CPU is derived the same way.
//!
//! Parsing accepts only canonical target names, and `TargetConfig::host`
//! returns `None` on a host outside the maintained matrix. Every persistent
//! identity (build plans, runner configuration, cache namespaces) encodes the
//! canonical name. Conditional compilation is evaluated against this model by
//! `nia-target-config`, which does not own target identity.

use std::fmt;

/// Target instruction-set architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Arch {
    /// 32-bit x86 (i686 baseline).
    X86,
    /// 64-bit x86.
    X86_64,
    /// 64-bit Arm.
    Aarch64,
}

impl Arch {
    /// Name exposed to conditional compilation as `arch`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::X86 => "x86",
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    /// Pointer width in bits.
    pub const fn pointer_width(self) -> u32 {
        match self {
            Self::X86 => 32,
            Self::X86_64 | Self::Aarch64 => 64,
        }
    }
}

/// Target operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Os {
    /// Linux, entered through the kernel process ABI.
    Linux,
    /// Windows, entered through the PE loader.
    Windows,
    /// macOS, entered through `dyld`.
    Macos,
}

impl Os {
    /// Name exposed to conditional compilation as `os`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Macos => "macos",
        }
    }
}

/// Target byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Endian {
    /// Least significant byte first.
    Little,
    /// Most significant byte first.
    Big,
}

impl Endian {
    /// Name exposed to conditional compilation as `endian`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Little => "little",
            Self::Big => "big",
        }
    }
}

/// Object file format produced for a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectFormat {
    /// ELF objects and executables.
    Elf,
    /// COFF objects and PE executables.
    Coff,
    /// Mach-O objects and executables.
    MachO,
}

/// Minimum operating-system release a target's executables run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OsVersion {
    /// Major release number.
    pub major: u32,
    /// Minor release number.
    pub minor: u32,
}

impl fmt::Display for OsVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The oldest macOS release that hosted CI executes; it also fixes one Mach-O
/// fixup format (chained fixups) for every maintained architecture.
const MACOS_MINIMUM: OsVersion = OsVersion {
    major: 15,
    minor: 0,
};

/// A maintained compilation target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TargetConfig {
    arch: Arch,
    os: Os,
}

/// Every maintained target, in display order.
pub const SUPPORTED_TARGETS: [TargetConfig; 6] = [
    TargetConfig::new_unchecked(Arch::X86_64, Os::Linux),
    TargetConfig::new_unchecked(Arch::X86, Os::Linux),
    TargetConfig::new_unchecked(Arch::Aarch64, Os::Linux),
    TargetConfig::new_unchecked(Arch::X86_64, Os::Windows),
    TargetConfig::new_unchecked(Arch::X86_64, Os::Macos),
    TargetConfig::new_unchecked(Arch::Aarch64, Os::Macos),
];

/// A target name outside the maintained matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedTarget {
    /// The rejected spelling.
    pub name: String,
}

impl fmt::Display for UnsupportedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported target `{}`; supported targets are ",
            self.name
        )?;
        for (index, target) in SUPPORTED_TARGETS.iter().enumerate() {
            if index != 0 {
                f.write_str(", ")?;
            }
            write!(f, "`{target}`")?;
        }
        Ok(())
    }
}

impl std::error::Error for UnsupportedTarget {}

impl TargetConfig {
    const fn new_unchecked(arch: Arch, os: Os) -> Self {
        Self { arch, os }
    }

    #[cfg(all(target_arch = "x86", target_os = "linux"))]
    const COMPILED_HOST: Option<Self> = Some(Self::new_unchecked(Arch::X86, Os::Linux));
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    const COMPILED_HOST: Option<Self> = Some(Self::new_unchecked(Arch::X86_64, Os::Linux));
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    const COMPILED_HOST: Option<Self> = Some(Self::new_unchecked(Arch::Aarch64, Os::Linux));
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    const COMPILED_HOST: Option<Self> = Some(Self::new_unchecked(Arch::X86_64, Os::Windows));
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    const COMPILED_HOST: Option<Self> = Some(Self::new_unchecked(Arch::X86_64, Os::Macos));
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    const COMPILED_HOST: Option<Self> = Some(Self::new_unchecked(Arch::Aarch64, Os::Macos));
    #[cfg(not(any(
        all(target_arch = "x86", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "windows"),
        all(target_arch = "x86_64", target_os = "macos"),
        all(target_arch = "aarch64", target_os = "macos")
    )))]
    const COMPILED_HOST: Option<Self> = None;

    /// Selects a maintained target, or `None` for a combination outside it.
    pub fn new(arch: Arch, os: Os) -> Option<Self> {
        let target = Self { arch, os };
        SUPPORTED_TARGETS.contains(&target).then_some(target)
    }

    /// Parses a canonical target name, such as `aarch64-apple-macos`.
    pub fn parse(name: &str) -> Result<Self, UnsupportedTarget> {
        SUPPORTED_TARGETS
            .iter()
            .copied()
            .find(|target| target.name() == name)
            .ok_or_else(|| UnsupportedTarget {
                name: name.to_string(),
            })
    }

    /// The target the running compiler was built for, when it is maintained.
    ///
    /// A compiler may run on a host it cannot target; such hosts must select
    /// an artifact target explicitly.
    pub fn host() -> Option<Self> {
        // Resolve the compiler host through Rust's target configuration so
        // host selection remains typed and cannot drift through string names.
        Self::COMPILED_HOST
    }

    /// Position of this target in [`SUPPORTED_TARGETS`], for protocols that
    /// transmit the matrix once and refer to its members by index.
    pub fn matrix_index(self) -> usize {
        SUPPORTED_TARGETS
            .iter()
            .position(|target| *target == self)
            .expect("every constructed target is a member of the matrix")
    }

    /// Target architecture.
    pub const fn arch(self) -> Arch {
        self.arch
    }

    /// Target operating system.
    pub const fn os(self) -> Os {
        self.os
    }

    /// Vendor component of the target name.
    pub const fn vendor(self) -> &'static str {
        match self.os {
            Os::Linux => "unknown",
            Os::Windows => "pc",
            Os::Macos => "apple",
        }
    }

    /// Environment component: the platform ABI flavor, not a C library.
    pub const fn env(self) -> &'static str {
        match self.os {
            Os::Windows => "msvc",
            Os::Linux | Os::Macos => "",
        }
    }

    /// Byte order; every maintained architecture is little-endian.
    pub const fn endian(self) -> Endian {
        Endian::Little
    }

    /// Pointer width in bits.
    pub const fn pointer_width(self) -> u32 {
        self.arch.pointer_width()
    }

    /// Object file format.
    pub const fn object_format(self) -> ObjectFormat {
        match self.os {
            Os::Linux => ObjectFormat::Elf,
            Os::Windows => ObjectFormat::Coff,
            Os::Macos => ObjectFormat::MachO,
        }
    }

    /// File extension of an executable, without the dot; empty when the
    /// platform uses none.
    pub const fn executable_extension(self) -> &'static str {
        match self.os {
            Os::Windows => "exe",
            Os::Linux | Os::Macos => "",
        }
    }

    /// Minimum OS release, for platforms whose binaries record one.
    pub const fn minimum_os_version(self) -> Option<OsVersion> {
        match self.os {
            Os::Macos => Some(MACOS_MINIMUM),
            Os::Linux | Os::Windows => None,
        }
    }

    /// Canonical user-facing name, accepted by [`TargetConfig::parse`].
    pub fn name(self) -> String {
        let mut name = format!("{}-{}-{}", self.arch.name(), self.vendor(), self.os.name());
        if !self.env().is_empty() {
            name.push('-');
            name.push_str(self.env());
        }
        name
    }

    /// LLVM target triple.
    pub fn llvm_triple(self) -> String {
        match self.os {
            Os::Linux => {
                let arch = match self.arch {
                    Arch::X86 => "i686",
                    Arch::X86_64 => "x86_64",
                    Arch::Aarch64 => "aarch64",
                };
                format!("{arch}-unknown-linux")
            }
            Os::Windows => format!("{}-pc-windows-msvc", self.arch.name()),
            Os::Macos => {
                let arch = match self.arch {
                    Arch::Aarch64 => "arm64",
                    Arch::X86 | Arch::X86_64 => self.arch.name(),
                };
                format!("{arch}-apple-macosx{MACOS_MINIMUM}.0")
            }
        }
    }

    /// Baseline CPU for code that must run on every machine of the target.
    pub const fn baseline_cpu(self) -> &'static str {
        match (self.arch, self.os) {
            (Arch::X86, _) => "i686",
            (Arch::X86_64, Os::Macos) => "core2",
            (Arch::X86_64, _) => "x86-64",
            (Arch::Aarch64, Os::Macos) => "apple-m1",
            (Arch::Aarch64, _) => "generic",
        }
    }
}

impl fmt::Display for TargetConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// The oldest processor that generated code may run on.
///
/// Code built for a level uses every instruction that level guarantees, so it
/// fails with an illegal-instruction fault on an older processor. A level is
/// not part of the target: one target accepts several levels, and the target
/// name never mentions one. Persistent identities that depend on generated
/// code must therefore record the level beside the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CpuLevel {
    /// Every processor of the target.
    #[default]
    Baseline,
    /// x86-64 psABI level 2: adds SSE3, SSSE3, SSE4.1, SSE4.2, POPCNT and
    /// CMPXCHG16B (Nehalem, 2009, and later).
    X86_64V2,
    /// x86-64 psABI level 3: adds AVX, AVX2, FMA, BMI1, BMI2, LZCNT, MOVBE
    /// and F16C (Haswell, 2013, Zen 1, and later).
    X86_64V3,
    /// x86-64 psABI level 4: adds AVX-512 F, BW, CD, DQ and VL (Skylake-X and
    /// Zen 4; many consumer Intel processors lack it).
    X86_64V4,
    /// The processor of the machine running the compiler, with every feature
    /// it reports. Only valid when the target is that machine's.
    Native,
}

/// Every CPU level, in display order.
pub const CPU_LEVELS: [CpuLevel; 5] = [
    CpuLevel::Baseline,
    CpuLevel::X86_64V2,
    CpuLevel::X86_64V3,
    CpuLevel::X86_64V4,
    CpuLevel::Native,
];

impl CpuLevel {
    /// Canonical name, accepted by [`CpuLevel::parse`].
    pub const fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::X86_64V2 => "x86_64-v2",
            Self::X86_64V3 => "x86_64-v3",
            Self::X86_64V4 => "x86_64-v4",
            Self::Native => "native",
        }
    }

    /// Parses a canonical level name, such as `x86_64-v3`.
    pub fn parse(name: &str) -> Result<Self, CpuLevelError> {
        CPU_LEVELS
            .into_iter()
            .find(|level| level.name() == name)
            .ok_or_else(|| CpuLevelError::Unknown {
                name: name.to_string(),
            })
    }

    /// Checks that this level can describe a processor of `target` when the
    /// compiler runs on `host`, or `None` on a host outside the maintained
    /// matrix. A named level must belong to the target's architecture, and
    /// `Native` needs the target to be the host because it describes the
    /// host's processor.
    pub fn check(
        self,
        target: TargetConfig,
        host: Option<TargetConfig>,
    ) -> Result<(), CpuLevelError> {
        match self {
            Self::Baseline => Ok(()),
            Self::X86_64V2 | Self::X86_64V3 | Self::X86_64V4 => {
                if target.arch() == Arch::X86_64 {
                    Ok(())
                } else {
                    Err(CpuLevelError::WrongArchitecture {
                        level: self,
                        target,
                    })
                }
            }
            Self::Native => {
                if host == Some(target) {
                    Ok(())
                } else {
                    Err(CpuLevelError::NativeForeignTarget { target, host })
                }
            }
        }
    }

    /// The LLVM CPU model for this level on `target`, or `None` for
    /// [`CpuLevel::Native`], which only the running host can name.
    pub const fn llvm_cpu(self, target: TargetConfig) -> Option<&'static str> {
        match self {
            Self::Baseline => Some(target.baseline_cpu()),
            Self::X86_64V2 => Some("x86-64-v2"),
            Self::X86_64V3 => Some("x86-64-v3"),
            Self::X86_64V4 => Some("x86-64-v4"),
            Self::Native => None,
        }
    }
}

impl fmt::Display for CpuLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A CPU level that does not exist or does not fit the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuLevelError {
    /// No level has this name.
    Unknown {
        /// The rejected spelling.
        name: String,
    },
    /// A named level of another architecture.
    WrongArchitecture {
        /// The requested level.
        level: CpuLevel,
        /// The target it does not belong to.
        target: TargetConfig,
    },
    /// `native` for a target other than the host.
    NativeForeignTarget {
        /// The requested target.
        target: TargetConfig,
        /// The host, when it is a maintained target.
        host: Option<TargetConfig>,
    },
}

impl fmt::Display for CpuLevelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { name } => {
                write!(f, "unknown CPU level `{name}`; CPU levels are ")?;
                for (index, level) in CPU_LEVELS.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "`{level}`")?;
                }
                Ok(())
            }
            Self::WrongArchitecture { level, target } => write!(
                f,
                "CPU level `{level}` is for x86_64 targets, not `{target}`"
            ),
            Self::NativeForeignTarget {
                target,
                host: Some(host),
            } => write!(
                f,
                "CPU level `native` describes this machine (`{host}`), not target `{target}`; \
                 name a CPU level for `{target}` instead"
            ),
            Self::NativeForeignTarget { target, host: None } => write!(
                f,
                "CPU level `native` describes this machine, which is not a maintained target; \
                 name a CPU level for `{target}` instead"
            ),
        }
    }
}

impl std::error::Error for CpuLevelError {}

/// A processor feature that conditional compilation can test with
/// `hasFeature("name")`.
///
/// The table holds the features the x86-64 levels are defined by and the
/// AArch64 vector unit. Names are Nia's own, so a future LLVM rename cannot
/// change what a condition means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CpuFeature {
    /// SSE2; every x86_64 processor has it.
    Sse2,
    /// SSE3.
    Sse3,
    /// Supplemental SSE3, which adds the byte shuffle `pshufb`.
    Ssse3,
    /// SSE4.1.
    Sse4_1,
    /// SSE4.2.
    Sse4_2,
    /// The population-count instruction.
    Popcnt,
    /// The 16-byte compare-and-exchange instruction.
    Cmpxchg16b,
    /// AVX.
    Avx,
    /// AVX2.
    Avx2,
    /// Fused multiply-add.
    Fma,
    /// Bit manipulation instructions, set 1.
    Bmi1,
    /// Bit manipulation instructions, set 2.
    Bmi2,
    /// The leading-zero-count instruction.
    Lzcnt,
    /// The byte-swapping move instruction.
    Movbe,
    /// Half-precision conversion instructions.
    F16c,
    /// AVX-512 foundation.
    Avx512f,
    /// AVX-512 byte and word instructions.
    Avx512bw,
    /// AVX-512 conflict detection.
    Avx512cd,
    /// AVX-512 doubleword and quadword instructions.
    Avx512dq,
    /// AVX-512 vector length extensions.
    Avx512vl,
    /// The AArch64 Advanced SIMD unit; every AArch64 processor has it.
    Neon,
}

/// Every CPU feature, in display order. A feature appears after every
/// feature it implies, so a scan from the end meets an implying feature
/// before the features it implies (the runtime checker relies on this).
pub const CPU_FEATURES: [CpuFeature; 21] = [
    CpuFeature::Sse2,
    CpuFeature::Sse3,
    CpuFeature::Ssse3,
    CpuFeature::Sse4_1,
    CpuFeature::Sse4_2,
    CpuFeature::Popcnt,
    CpuFeature::Cmpxchg16b,
    CpuFeature::Avx,
    CpuFeature::Avx2,
    CpuFeature::Fma,
    CpuFeature::Bmi1,
    CpuFeature::Bmi2,
    CpuFeature::Lzcnt,
    CpuFeature::Movbe,
    CpuFeature::F16c,
    CpuFeature::Avx512f,
    CpuFeature::Avx512bw,
    CpuFeature::Avx512cd,
    CpuFeature::Avx512dq,
    CpuFeature::Avx512vl,
    CpuFeature::Neon,
];

impl CpuFeature {
    /// Canonical name, accepted by [`CpuFeature::parse`].
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sse2 => "sse2",
            Self::Sse3 => "sse3",
            Self::Ssse3 => "ssse3",
            Self::Sse4_1 => "sse4_1",
            Self::Sse4_2 => "sse4_2",
            Self::Popcnt => "popcnt",
            Self::Cmpxchg16b => "cmpxchg16b",
            Self::Avx => "avx",
            Self::Avx2 => "avx2",
            Self::Fma => "fma",
            Self::Bmi1 => "bmi1",
            Self::Bmi2 => "bmi2",
            Self::Lzcnt => "lzcnt",
            Self::Movbe => "movbe",
            Self::F16c => "f16c",
            Self::Avx512f => "avx512f",
            Self::Avx512bw => "avx512bw",
            Self::Avx512cd => "avx512cd",
            Self::Avx512dq => "avx512dq",
            Self::Avx512vl => "avx512vl",
            Self::Neon => "neon",
        }
    }

    /// Parses a canonical feature name, such as `avx2`.
    pub fn parse(name: &str) -> Option<Self> {
        CPU_FEATURES
            .into_iter()
            .find(|feature| feature.name() == name)
    }

    /// LLVM's name for the feature in a target feature string.
    pub const fn llvm_name(self) -> &'static str {
        match self {
            Self::Sse4_1 => "sse4.1",
            Self::Sse4_2 => "sse4.2",
            Self::Cmpxchg16b => "cx16",
            Self::Bmi1 => "bmi",
            _ => self.name(),
        }
    }

    /// The x86-64 psABI level that first guarantees the feature; `None` for
    /// a feature of another architecture.
    const fn x86_64_level(self) -> Option<CpuLevel> {
        match self {
            Self::Sse2 => Some(CpuLevel::Baseline),
            Self::Sse3
            | Self::Ssse3
            | Self::Sse4_1
            | Self::Sse4_2
            | Self::Popcnt
            | Self::Cmpxchg16b => Some(CpuLevel::X86_64V2),
            Self::Avx
            | Self::Avx2
            | Self::Fma
            | Self::Bmi1
            | Self::Bmi2
            | Self::Lzcnt
            | Self::Movbe
            | Self::F16c => Some(CpuLevel::X86_64V3),
            Self::Avx512f | Self::Avx512bw | Self::Avx512cd | Self::Avx512dq | Self::Avx512vl => {
                Some(CpuLevel::X86_64V4)
            }
            Self::Neon => None,
        }
    }

    const fn bit(self) -> u32 {
        1 << (self as u32)
    }

    /// Whether this instruction-set feature belongs to `arch`.
    pub const fn supports_arch(self, arch: Arch) -> bool {
        matches!((self, arch), (Self::Neon, Arch::Aarch64))
            || (!matches!(self, Self::Neon) && matches!(arch, Arch::X86 | Arch::X86_64))
    }
}

impl fmt::Display for CpuFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The CPU features generated code may rely on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CpuFeatures(u32);

impl CpuFeatures {
    /// Reads a stable set, rejecting bits outside the feature registry.
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !((1 << CPU_FEATURES.len()) - 1) == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Union of two feature sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every feature in this set is available in `other`.
    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    /// The features of this set that `other` lacks.
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The features of this set that no other member implies, in display
    /// order: the names a programmer writes. `{sse3, ..., avx, avx2}` gives
    /// `avx2`.
    pub fn outermost(self) -> impl Iterator<Item = CpuFeature> {
        self.iter().filter(move |feature| {
            !self.iter().any(|other| {
                other != *feature && Self::default().enabling(other).contains(*feature)
            })
        })
    }

    /// Enables one feature and the instruction sets it requires. These are
    /// the same implications used by LLVM's target-feature model.
    pub fn enabling(self, feature: CpuFeature) -> Self {
        use CpuFeature::*;
        let features = self.with(feature);
        match feature {
            Sse3 => features.enabling(Sse2),
            Ssse3 => features.enabling(Sse3),
            Sse4_1 => features.enabling(Ssse3),
            Sse4_2 => features.enabling(Sse4_1),
            Avx => features.enabling(Sse4_2),
            Avx2 | Fma | F16c => features.enabling(Avx),
            Avx512f => features.enabling(Avx2).enabling(Fma).enabling(F16c),
            Avx512bw | Avx512cd | Avx512dq | Avx512vl => features.enabling(Avx512f),
            _ => features,
        }
    }

    /// Whether code may rely on `feature`.
    pub const fn contains(self, feature: CpuFeature) -> bool {
        self.0 & feature.bit() != 0
    }

    /// The set as stable bits for persistent identities: bit `i` is feature
    /// `CPU_FEATURES[i]`. Appending a feature keeps every existing bit.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The features in display order.
    pub fn iter(self) -> impl Iterator<Item = CpuFeature> {
        CPU_FEATURES
            .into_iter()
            .filter(move |feature| self.contains(*feature))
    }

    fn with(self, feature: CpuFeature) -> Self {
        Self(self.0 | feature.bit())
    }

    /// The features every processor of `target` has.
    pub fn baseline(target: TargetConfig) -> Self {
        let mut features = Self::default();
        match (target.arch(), target.os()) {
            (Arch::X86, _) => {}
            // The macOS baseline is `core2`, which adds SSE3 and SSSE3.
            (Arch::X86_64, Os::Macos) => {
                features = features
                    .with(CpuFeature::Sse2)
                    .with(CpuFeature::Sse3)
                    .with(CpuFeature::Ssse3);
            }
            (Arch::X86_64, _) => features = features.with(CpuFeature::Sse2),
            (Arch::Aarch64, _) => features = features.with(CpuFeature::Neon),
        }
        features
    }

    /// The features `level` guarantees on `target`, or `None` for
    /// [`CpuLevel::Native`], which only the processor itself can report.
    /// The level must fit the target, as [`CpuLevel::check`] requires.
    pub fn for_level(target: TargetConfig, level: CpuLevel) -> Option<Self> {
        if level == CpuLevel::Native {
            return None;
        }
        let mut features = Self::baseline(target);
        if target.arch() == Arch::X86_64 {
            for feature in CPU_FEATURES {
                if feature.x86_64_level().is_some_and(|first| first <= level) {
                    features = features.with(feature);
                }
            }
        }
        Some(features)
    }

    /// The features an LLVM target feature string such as
    /// `+sse2,+avx2,-avx512f` enables on `target`, together with the
    /// target's baseline. Features of another architecture are ignored.
    pub fn from_llvm_features(target: TargetConfig, llvm_features: &str) -> Self {
        let mut features = Self::baseline(target);
        for entry in llvm_features.split(',') {
            let Some(name) = entry.strip_prefix('+') else {
                continue;
            };
            if let Some(feature) = CPU_FEATURES
                .into_iter()
                .find(|feature| feature.supports_arch(target.arch()) && feature.llvm_name() == name)
            {
                features = features.with(feature);
            }
        }
        features
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_follow_the_features_they_imply() {
        for (index, feature) in CPU_FEATURES.into_iter().enumerate() {
            for implied in CpuFeatures::default().enabling(feature).iter() {
                let implied_index = CPU_FEATURES.iter().position(|f| *f == implied);
                assert!(implied_index <= Some(index), "{feature} implies {implied}");
            }
        }
    }

    #[test]
    fn outermost_features_name_what_the_programmer_wrote() {
        use CpuFeature::*;
        let enable = |features: &[CpuFeature]| {
            features
                .iter()
                .fold(CpuFeatures::default(), |set, feature| {
                    set.enabling(*feature)
                })
        };
        let outermost = |set: CpuFeatures| set.outermost().collect::<Vec<_>>();
        assert_eq!(outermost(enable(&[Avx2])), [Avx2]);
        assert_eq!(outermost(enable(&[Avx2, Bmi2, Fma])), [Avx2, Fma, Bmi2]);
        assert_eq!(
            outermost(enable(&[Avx512bw, Avx512vl])),
            [Avx512bw, Avx512vl]
        );
        let missing = enable(&[Avx2]).difference(enable(&[Sse4_2]));
        assert_eq!(missing.iter().collect::<Vec<_>>(), [Avx, Avx2]);
        assert_eq!(outermost(missing), [Avx2]);
        assert_eq!(outermost(CpuFeatures::default()), []);
    }

    #[test]
    fn every_supported_target_round_trips_its_name() {
        for target in SUPPORTED_TARGETS {
            assert_eq!(TargetConfig::parse(&target.name()), Ok(target));
            assert_eq!(TargetConfig::new(target.arch(), target.os()), Some(target));
            assert_eq!(SUPPORTED_TARGETS[target.matrix_index()], target);
        }
        assert_eq!(
            SUPPORTED_TARGETS.map(TargetConfig::name),
            [
                "x86_64-unknown-linux",
                "x86-unknown-linux",
                "aarch64-unknown-linux",
                "x86_64-pc-windows-msvc",
                "x86_64-apple-macos",
                "aarch64-apple-macos",
            ]
        );
    }

    #[test]
    fn derived_properties_follow_the_platform() {
        let macos = TargetConfig::parse("aarch64-apple-macos").unwrap();
        assert_eq!(macos.object_format(), ObjectFormat::MachO);
        assert_eq!(macos.executable_extension(), "");
        assert_eq!(macos.llvm_triple(), "arm64-apple-macosx15.0.0");
        assert_eq!(
            macos.minimum_os_version(),
            Some(OsVersion {
                major: 15,
                minor: 0
            })
        );
        let x86 = TargetConfig::parse("x86-unknown-linux").unwrap();
        assert_eq!(x86.pointer_width(), 32);
        assert_eq!(x86.llvm_triple(), "i686-unknown-linux");
        let windows = TargetConfig::parse("x86_64-pc-windows-msvc").unwrap();
        assert_eq!(windows.object_format(), ObjectFormat::Coff);
        assert_eq!(windows.executable_extension(), "exe");
        assert_eq!(windows.env(), "msvc");
        assert_eq!(TargetConfig::new(Arch::Aarch64, Os::Windows), None);
    }

    #[test]
    fn unsupported_names_list_the_matrix() {
        let error = TargetConfig::parse("x86_64-unknown-linux-gnu").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("`x86_64-unknown-linux-gnu`"), "{message}");
        assert!(message.contains("`aarch64-apple-macos`"), "{message}");
    }

    #[test]
    fn host_is_derived_from_the_compiler_target() {
        #[cfg(all(target_arch = "x86", target_os = "linux"))]
        let expected = Some(TargetConfig::new_unchecked(Arch::X86, Os::Linux));
        #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
        let expected = Some(TargetConfig::new_unchecked(Arch::X86_64, Os::Linux));
        #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
        let expected = Some(TargetConfig::new_unchecked(Arch::Aarch64, Os::Linux));
        #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
        let expected = Some(TargetConfig::new_unchecked(Arch::X86_64, Os::Windows));
        #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
        let expected = Some(TargetConfig::new_unchecked(Arch::X86_64, Os::Macos));
        #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
        let expected = Some(TargetConfig::new_unchecked(Arch::Aarch64, Os::Macos));
        #[cfg(not(any(
            all(target_arch = "x86", target_os = "linux"),
            all(target_arch = "x86_64", target_os = "linux"),
            all(target_arch = "aarch64", target_os = "linux"),
            all(target_arch = "x86_64", target_os = "windows"),
            all(target_arch = "x86_64", target_os = "macos"),
            all(target_arch = "aarch64", target_os = "macos")
        )))]
        let expected = None;

        assert_eq!(TargetConfig::host(), expected);
    }

    #[test]
    fn cpu_levels_round_trip_their_names() {
        for level in CPU_LEVELS {
            assert_eq!(CpuLevel::parse(level.name()), Ok(level));
        }
        assert_eq!(CpuLevel::default(), CpuLevel::Baseline);
        let error = CpuLevel::parse("x86-64-v3").expect_err("LLVM spelling is not a level name");
        assert_eq!(
            error.to_string(),
            "unknown CPU level `x86-64-v3`; CPU levels are `baseline`, `x86_64-v2`, `x86_64-v3`, `x86_64-v4`, `native`"
        );
    }

    #[test]
    fn cpu_levels_fit_only_their_architecture() {
        let x86_64 = TargetConfig::parse("x86_64-unknown-linux").expect("maintained target");
        let aarch64 = TargetConfig::parse("aarch64-unknown-linux").expect("maintained target");
        let x86 = TargetConfig::parse("x86-unknown-linux").expect("maintained target");
        for target in SUPPORTED_TARGETS {
            assert_eq!(CpuLevel::Baseline.check(target, None), Ok(()));
            assert_eq!(
                CpuLevel::Baseline.llvm_cpu(target),
                Some(target.baseline_cpu())
            );
        }
        for level in [CpuLevel::X86_64V2, CpuLevel::X86_64V3, CpuLevel::X86_64V4] {
            assert_eq!(level.check(x86_64, None), Ok(()));
            for target in [aarch64, x86] {
                assert_eq!(
                    level.check(target, None),
                    Err(CpuLevelError::WrongArchitecture { level, target })
                );
            }
        }
        assert_eq!(CpuLevel::X86_64V3.llvm_cpu(x86_64), Some("x86-64-v3"));
        assert_eq!(
            CpuLevel::X86_64V2
                .check(aarch64, None)
                .expect_err("x86 level")
                .to_string(),
            "CPU level `x86_64-v2` is for x86_64 targets, not `aarch64-unknown-linux`"
        );
    }

    #[test]
    fn native_cpu_level_requires_the_host_target() {
        let linux = TargetConfig::parse("x86_64-unknown-linux").expect("maintained target");
        let windows = TargetConfig::parse("x86_64-pc-windows-msvc").expect("maintained target");
        assert_eq!(CpuLevel::Native.check(windows, Some(windows)), Ok(()));
        assert_eq!(CpuLevel::Native.llvm_cpu(windows), None);
        let foreign = CpuLevel::Native
            .check(linux, Some(windows))
            .expect_err("native names the host");
        assert_eq!(
            foreign.to_string(),
            "CPU level `native` describes this machine (`x86_64-pc-windows-msvc`), not target `x86_64-unknown-linux`; name a CPU level for `x86_64-unknown-linux` instead"
        );
        assert!(matches!(
            CpuLevel::Native.check(linux, None),
            Err(CpuLevelError::NativeForeignTarget { host: None, .. })
        ));
    }

    #[test]
    fn cpu_features_round_trip_their_names_and_keep_stable_bits() {
        for (index, feature) in CPU_FEATURES.into_iter().enumerate() {
            assert_eq!(CpuFeature::parse(feature.name()), Some(feature));
            assert_eq!(feature.bit(), 1 << index, "{feature}");
        }
        // LLVM spellings are not feature names.
        assert_eq!(CpuFeature::parse("sse4.2"), None);
        assert_eq!(CpuFeature::parse("bmi"), None);
    }

    #[test]
    fn function_features_include_dependencies_and_validate_stable_sets() {
        let avx = CpuFeatures::default().enabling(CpuFeature::Avx);
        let avx2 = CpuFeatures::default().enabling(CpuFeature::Avx2);
        assert!(avx.is_subset_of(avx2));
        assert!(!avx2.is_subset_of(avx));
        assert!(avx2.contains(CpuFeature::Sse2) && avx2.contains(CpuFeature::Sse4_2));
        assert!(!avx2.contains(CpuFeature::Fma) && !avx2.contains(CpuFeature::Popcnt));
        let zmm = CpuFeatures::default().enabling(CpuFeature::Avx512bw);
        assert!(avx2.is_subset_of(zmm) && zmm.contains(CpuFeature::Avx512f));
        assert!(zmm.contains(CpuFeature::Fma) && zmm.contains(CpuFeature::F16c));
        assert_eq!(CpuFeatures::from_bits(zmm.bits()), Some(zmm));
        assert_eq!(CpuFeatures::from_bits(1 << CPU_FEATURES.len()), None);
        let i686 = TargetConfig::parse("x86-unknown-linux").unwrap();
        assert!(CpuFeatures::from_llvm_features(i686, "+sse2,+avx2").contains(CpuFeature::Avx2));
        assert!(CpuFeature::Avx2.supports_arch(Arch::X86));
        assert!(!CpuFeature::Avx2.supports_arch(Arch::Aarch64));
    }

    #[test]
    fn cpu_levels_imply_the_psabi_feature_sets() {
        let linux = TargetConfig::parse("x86_64-unknown-linux").expect("maintained target");
        let names = |level| {
            CpuFeatures::for_level(linux, level)
                .expect("named level")
                .iter()
                .map(CpuFeature::name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(CpuLevel::Baseline), ["sse2"]);
        assert_eq!(
            names(CpuLevel::X86_64V2),
            [
                "sse2",
                "sse3",
                "ssse3",
                "sse4_1",
                "sse4_2",
                "popcnt",
                "cmpxchg16b"
            ]
        );
        let v3 = CpuFeatures::for_level(linux, CpuLevel::X86_64V3).expect("named level");
        assert!(v3.contains(CpuFeature::Avx2) && v3.contains(CpuFeature::Bmi2));
        assert!(!v3.contains(CpuFeature::Avx512f));
        let v4 = CpuFeatures::for_level(linux, CpuLevel::X86_64V4).expect("named level");
        assert_eq!(
            v4.iter().count(),
            CPU_FEATURES.len() - 1,
            "v4 has every x86 feature"
        );
        assert_eq!(CpuFeatures::for_level(linux, CpuLevel::Native), None);

        // Baselines differ by target, not only by architecture.
        let macos = TargetConfig::parse("x86_64-apple-macos").expect("maintained target");
        assert!(CpuFeatures::baseline(macos).contains(CpuFeature::Ssse3));
        let i686 = TargetConfig::parse("x86-unknown-linux").expect("maintained target");
        assert_eq!(CpuFeatures::baseline(i686), CpuFeatures::default());
        let aarch64 = TargetConfig::parse("aarch64-unknown-linux").expect("maintained target");
        assert_eq!(
            CpuFeatures::baseline(aarch64).iter().collect::<Vec<_>>(),
            [CpuFeature::Neon]
        );
    }

    #[test]
    fn llvm_feature_strings_map_to_cpu_features_of_the_target() {
        let linux = TargetConfig::parse("x86_64-unknown-linux").expect("maintained target");
        let features =
            CpuFeatures::from_llvm_features(linux, "+sse4.2,+bmi,-avx512f,+cx16,+neon,+unknown");
        assert_eq!(
            features.iter().map(CpuFeature::name).collect::<Vec<_>>(),
            ["sse2", "sse4_2", "cmpxchg16b", "bmi1"],
            "baseline kept, disabled and foreign features dropped"
        );
        let aarch64 = TargetConfig::parse("aarch64-unknown-linux").expect("maintained target");
        assert_eq!(
            CpuFeatures::from_llvm_features(aarch64, "+avx2"),
            CpuFeatures::baseline(aarch64)
        );
    }
}
