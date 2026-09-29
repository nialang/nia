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
//! combine inconsistent strings.

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
        let arch = match std::env::consts::ARCH {
            "x86" => Arch::X86,
            "x86_64" => Arch::X86_64,
            "aarch64" => Arch::Aarch64,
            _ => return None,
        };
        let os = match std::env::consts::OS {
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            "macos" => Os::Macos,
            _ => return None,
        };
        Self::new(arch, os)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_target_round_trips_its_name() {
        for target in SUPPORTED_TARGETS {
            assert_eq!(TargetConfig::parse(&target.name()), Ok(target));
            assert_eq!(TargetConfig::new(target.arch(), target.os()), Some(target));
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
}
