// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use nia_target_config::TargetConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerFlavor {
    Gnu,
    Lld,
    SelfHostedElf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    Static,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicLinker {
    Auto,
    None,
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableLinker {
    pub program: String,
    pub flavor: LinkerFlavor,
}

impl ExecutableLinker {
    pub fn native() -> Self {
        if let Ok(program) = env::var("NIA_LINKER")
            && !program.is_empty()
        {
            return Self::with_program(program);
        }
        Self::with_program("ld")
    }

    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            flavor: LinkerFlavor::Gnu,
        }
    }

    pub fn with_program_and_flavor(program: impl Into<String>, flavor: LinkerFlavor) -> Self {
        Self {
            program: program.into(),
            flavor,
        }
    }

    pub fn lld() -> Self {
        Self {
            program: String::new(),
            flavor: LinkerFlavor::Lld,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    pub arch: String,
    pub os: String,
    pub abi: String,
}

impl LinkTarget {
    pub fn host() -> Self {
        Self {
            arch: env::consts::ARCH.to_string(),
            os: env::consts::OS.to_string(),
            abi: default_host_abi(),
        }
    }

    pub fn from_target_config(config: &TargetConfig) -> Self {
        Self {
            arch: config.arch.clone(),
            os: config.os.clone(),
            abi: if config.abi.is_empty() {
                default_abi_for_os(&config.os)
            } else {
                config.abi.clone()
            },
        }
    }

    pub fn is_host(&self) -> bool {
        let host = Self::host();
        self.arch == host.arch && self.os == host.os && self.abi == host.abi
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkOptions {
    pub target: LinkTarget,
    pub linker: ExecutableLinker,
    pub entry: Option<String>,
    pub mode: LinkMode,
    pub dynamic_linker: DynamicLinker,
    pub sysroot: Option<String>,
    pub library_paths: Vec<String>,
    pub rpaths: Vec<String>,
    pub libraries: Vec<String>,
    pub raw_args: Vec<String>,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            linker: ExecutableLinker::native(),
            target: LinkTarget::host(),
            entry: Some("_start".to_string()),
            mode: LinkMode::Static,
            dynamic_linker: DynamicLinker::None,
            sysroot: None,
            library_paths: Vec::new(),
            rpaths: Vec::new(),
            libraries: Vec::new(),
            raw_args: Vec::new(),
        }
    }
}

impl LinkOptions {
    pub fn with_raw_args(mut self, args: Vec<String>) -> Self {
        self.raw_args = args;
        self
    }

    pub fn with_linker(mut self, linker: ExecutableLinker) -> Self {
        self.linker = linker;
        self
    }

    pub fn with_target(mut self, target: LinkTarget) -> Self {
        self.target = target;
        self
    }

    pub fn with_dynamic_linker(mut self, dynamic_linker: DynamicLinker) -> Self {
        self.dynamic_linker = dynamic_linker;
        self
    }

    pub fn with_dynamic_mode(mut self) -> Self {
        self.mode = LinkMode::Dynamic;
        if self.dynamic_linker == DynamicLinker::None {
            self.dynamic_linker = DynamicLinker::Auto;
        }
        self
    }

    pub fn add_library_path(mut self, path: impl Into<String>) -> Self {
        self.library_paths.push(path.into());
        self
    }

    pub fn add_rpath(mut self, path: impl Into<String>) -> Self {
        self.rpaths.push(path.into());
        self
    }

    pub fn add_library(mut self, library: impl Into<String>) -> Self {
        self.libraries.push(library.into());
        self
    }

    pub fn invocation(
        &self,
        objects: &[PathBuf],
        output: PathBuf,
    ) -> Result<LinkerInvocation, LinkerConfigError> {
        let linker = self.linker.resolve()?;
        match self.linker.flavor {
            LinkerFlavor::Gnu | LinkerFlavor::Lld => {
                self.gnu_like_invocation(&linker, objects, output)
            }
            LinkerFlavor::SelfHostedElf => {
                Err(LinkerConfigError::UnsupportedFlavor(self.linker.flavor))
            }
        }
    }

    fn gnu_like_invocation(
        &self,
        linker: &ResolvedLinker,
        objects: &[PathBuf],
        output: PathBuf,
    ) -> Result<LinkerInvocation, LinkerConfigError> {
        let mut args = Vec::new();
        if let Some(sysroot) = &self.sysroot {
            args.push(format!("--sysroot={sysroot}"));
        }
        if let Some(entry) = &self.entry {
            args.push("-e".to_string());
            args.push(entry.clone());
        }
        args.extend(
            objects
                .iter()
                .map(|path| path.to_string_lossy().into_owned()),
        );
        match self.mode {
            LinkMode::Static => {
                args.push("-static".to_string());
            }
            LinkMode::Dynamic => {}
        }
        for path in self.default_library_paths_for_linker(linker) {
            args.push("-L".to_string());
            args.push(path);
        }
        for path in &self.library_paths {
            args.push("-L".to_string());
            args.push(path.clone());
        }
        for rpath in &self.rpaths {
            args.push("-rpath".to_string());
            args.push(rpath.clone());
        }
        for library in &self.libraries {
            args.push("-l".to_string());
            args.push(library.clone());
        }
        match self.mode {
            LinkMode::Static => {}
            LinkMode::Dynamic => match &self.dynamic_linker {
                DynamicLinker::Auto => {
                    if let Some(path) = dynamic_linker_for_target(&self.target)? {
                        args.push("--dynamic-linker".to_string());
                        args.push(path);
                    } else {
                        args.push("--no-dynamic-linker".to_string());
                    }
                }
                DynamicLinker::None => {
                    args.push("--no-dynamic-linker".to_string());
                }
                DynamicLinker::Path(path) => {
                    args.push("--dynamic-linker".to_string());
                    args.push(path.clone());
                }
            },
        }
        args.extend(self.raw_args.iter().cloned());
        args.push("-o".to_string());
        args.push(output.to_string_lossy().into_owned());
        Ok(LinkerInvocation {
            program: linker.program.clone(),
            args,
        })
    }

    fn default_library_paths_for_linker(&self, linker: &ResolvedLinker) -> Vec<String> {
        if linker.flavor != LinkerFlavor::Lld || self.sysroot.is_some() || !self.target.is_host() {
            return Vec::new();
        }
        native_linux_library_paths()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedLinker {
    program: String,
    flavor: LinkerFlavor,
}

impl ExecutableLinker {
    fn resolve(&self) -> Result<ResolvedLinker, LinkerConfigError> {
        match self.flavor {
            LinkerFlavor::Gnu => Ok(ResolvedLinker {
                program: if self.program.is_empty() {
                    "ld".to_string()
                } else {
                    self.program.clone()
                },
                flavor: self.flavor,
            }),
            LinkerFlavor::Lld => Ok(ResolvedLinker {
                program: resolve_lld_program(&self.program)?,
                flavor: self.flavor,
            }),
            LinkerFlavor::SelfHostedElf => Err(LinkerConfigError::UnsupportedFlavor(self.flavor)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkerInvocation {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug)]
pub enum LinkerConfigError {
    Io {
        path: PathBuf,
        error: io::Error,
    },
    InvalidElf {
        path: PathBuf,
    },
    LinkerNotFound {
        flavor: LinkerFlavor,
        program: String,
    },
    UnsupportedFlavor(LinkerFlavor),
}

impl std::fmt::Display for LinkerConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, error } => {
                write!(f, "failed to inspect `{}`: {error}", path.display())
            }
            Self::InvalidElf { path } => write!(f, "`{}` is not a valid ELF file", path.display()),
            Self::LinkerNotFound { flavor, program } => {
                write!(
                    f,
                    "linker `{program}` for flavor `{flavor:?}` was not found"
                )
            }
            Self::UnsupportedFlavor(flavor) => {
                write!(f, "linker flavor `{flavor:?}` is not implemented")
            }
        }
    }
}

impl std::error::Error for LinkerConfigError {}

fn resolve_lld_program(program: &str) -> Result<String, LinkerConfigError> {
    if !program.is_empty() {
        return Ok(program.to_string());
    }
    if let Ok(program) = env::var("NIA_LLD")
        && !program.is_empty()
    {
        return Ok(program);
    }
    find_program_on_path("ld.lld").ok_or_else(|| LinkerConfigError::LinkerNotFound {
        flavor: LinkerFlavor::Lld,
        program: "ld.lld".to_string(),
    })
}

fn find_program_on_path(program: &str) -> Option<String> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return is_executable_file(program_path).then(|| program.to_string());
    }
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn native_linux_library_paths() -> Vec<String> {
    if env::consts::OS != "linux" {
        return Vec::new();
    }
    let mut paths = Vec::new();
    for path in [
        "/usr/local/lib64",
        "/usr/lib64",
        "/lib64",
        "/usr/local/lib",
        "/usr/lib",
        "/lib",
    ] {
        insert_existing_library_path(&mut paths, path);
    }
    read_ld_so_conf(&mut paths, Path::new("/etc/ld.so.conf"), 0);
    paths
}

fn insert_existing_library_path(paths: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() && Path::new(path).is_dir() {
        let path = path.to_string();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
}

fn read_ld_so_conf(paths: &mut Vec<String>, path: &Path, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    let base = path.parent().unwrap_or_else(|| Path::new("/"));
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(pattern) = line.strip_prefix("include ") {
            read_ld_so_conf_include(paths, base, pattern.trim(), depth + 1);
        } else {
            insert_existing_library_path(paths, line);
        }
    }
}

fn read_ld_so_conf_include(paths: &mut Vec<String>, base: &Path, pattern: &str, depth: usize) {
    let pattern_path = Path::new(pattern);
    let pattern_path = if pattern_path.is_absolute() {
        pattern_path.to_path_buf()
    } else {
        base.join(pattern_path)
    };
    let Some(parent) = pattern_path.parent() else {
        return;
    };
    let Some(file_pattern) = pattern_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if glob_file_name_matches(file_pattern, file_name) {
            read_ld_so_conf(paths, &path, depth);
        }
    }
}

fn glob_file_name_matches(pattern: &str, file_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let Some(star) = pattern.find('*') else {
        return pattern == file_name;
    };
    let prefix = &pattern[..star];
    let suffix = &pattern[star + 1..];
    file_name.starts_with(prefix) && file_name.ends_with(suffix)
}

pub fn native_dynamic_linker() -> Result<Option<String>, LinkerConfigError> {
    #[cfg(target_os = "linux")]
    {
        let path = PathBuf::from("/usr/bin/env");
        match elf_interpreter(&path) {
            Ok(Some(path)) => Ok(Some(path)),
            Ok(None) | Err(LinkerConfigError::InvalidElf { .. }) => Ok(standard_dynamic_linker()),
            Err(error) => Err(error),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

pub fn standard_dynamic_linker() -> Option<String> {
    standard_dynamic_linker_for(&LinkTarget::host())
}

pub fn standard_dynamic_linker_for(target: &LinkTarget) -> Option<String> {
    if target.os != "linux" {
        return None;
    }
    if is_musl_abi(&target.abi) {
        return musl_dynamic_linker(target);
    }
    if is_gnu_abi(&target.abi) {
        return gnu_dynamic_linker(target);
    }
    None
}

fn dynamic_linker_for_target(target: &LinkTarget) -> Result<Option<String>, LinkerConfigError> {
    if target.is_host() {
        native_dynamic_linker()
    } else {
        Ok(standard_dynamic_linker_for(target))
    }
}

fn gnu_dynamic_linker(target: &LinkTarget) -> Option<String> {
    match (target.arch.as_str(), target.abi.as_str()) {
        ("x86_64", "gnu") => Some("/lib64/ld-linux-x86-64.so.2".to_string()),
        ("x86_64", "gnux32") => Some("/libx32/ld-linux-x32.so.2".to_string()),
        ("x86", "gnu") | ("i386", "gnu") | ("i586", "gnu") | ("i686", "gnu") => {
            Some("/lib/ld-linux.so.2".to_string())
        }
        ("aarch64", "gnu") => Some("/lib/ld-linux-aarch64.so.1".to_string()),
        ("aarch64_be", "gnu") => Some("/lib/ld-linux-aarch64_be.so.1".to_string()),
        ("arm", "gnueabi") | ("armeb", "gnueabi") | ("thumb", "gnueabi") => {
            Some("/lib/ld-linux.so.3".to_string())
        }
        ("arm", "gnueabihf") | ("armeb", "gnueabihf") | ("thumb", "gnueabihf") => {
            Some("/lib/ld-linux-armhf.so.3".to_string())
        }
        ("riscv64", "gnu") => Some("/lib/ld-linux-riscv64-lp64d.so.1".to_string()),
        ("riscv32", "gnu") => Some("/lib/ld-linux-riscv32-ilp32d.so.1".to_string()),
        ("powerpc64", "gnu") | ("powerpc64le", "gnu") => Some("/lib64/ld64.so.2".to_string()),
        ("s390x", "gnu") => Some("/lib/ld64.so.1".to_string()),
        ("mips", "gnueabi")
        | ("mipsel", "gnueabi")
        | ("mips", "gnueabihf")
        | ("mipsel", "gnueabihf") => Some("/lib/ld.so.1".to_string()),
        ("mips64", "gnuabi64") | ("mips64el", "gnuabi64") => Some("/lib64/ld.so.1".to_string()),
        ("mips64", "gnuabin32") | ("mips64el", "gnuabin32") => Some("/lib32/ld.so.1".to_string()),
        ("loongarch64", "gnu") => Some("/lib64/ld-linux-loongarch-lp64d.so.1".to_string()),
        ("loongarch64", "gnuf32") => Some("/lib64/ld-linux-loongarch-lp64f.so.1".to_string()),
        ("loongarch64", "gnusf") => Some("/lib64/ld-linux-loongarch-lp64s.so.1".to_string()),
        ("sparc64", "gnu") => Some("/lib64/ld-linux.so.2".to_string()),
        ("sparc", "gnu") | ("alpha", "gnu") => Some("/lib/ld-linux.so.2".to_string()),
        ("hppa", "gnu") | ("m68k", "gnu") | ("microblaze", "gnu") | ("microblazeel", "gnu") => {
            Some("/lib/ld.so.1".to_string())
        }
        _ => None,
    }
}

fn musl_dynamic_linker(target: &LinkTarget) -> Option<String> {
    match (target.arch.as_str(), target.abi.as_str()) {
        ("x86_64", "musl") => Some("/lib/ld-musl-x86_64.so.1".to_string()),
        ("x86_64", "muslx32") => Some("/lib/ld-musl-x32.so.1".to_string()),
        ("x86", "musl") | ("i386", "musl") | ("i586", "musl") | ("i686", "musl") => {
            Some("/lib/ld-musl-i386.so.1".to_string())
        }
        ("aarch64", "musl") => Some("/lib/ld-musl-aarch64.so.1".to_string()),
        ("aarch64_be", "musl") => Some("/lib/ld-musl-aarch64_be.so.1".to_string()),
        ("arm", "musleabi") | ("thumb", "musleabi") => Some("/lib/ld-musl-arm.so.1".to_string()),
        ("arm", "musleabihf") | ("thumb", "musleabihf") => {
            Some("/lib/ld-musl-armhf.so.1".to_string())
        }
        ("armeb", "musleabi") | ("thumbeb", "musleabi") => {
            Some("/lib/ld-musl-armeb.so.1".to_string())
        }
        ("armeb", "musleabihf") | ("thumbeb", "musleabihf") => {
            Some("/lib/ld-musl-armebhf.so.1".to_string())
        }
        ("riscv64", "musl") => Some("/lib/ld-musl-riscv64.so.1".to_string()),
        ("riscv32", "musl") => Some("/lib/ld-musl-riscv32.so.1".to_string()),
        ("powerpc64", "musl") => Some("/lib/ld-musl-powerpc64.so.1".to_string()),
        ("powerpc64le", "musl") => Some("/lib/ld-musl-powerpc64le.so.1".to_string()),
        ("powerpc", "musleabi") => Some("/lib/ld-musl-powerpc-sf.so.1".to_string()),
        ("powerpc", "musleabihf") => Some("/lib/ld-musl-powerpc.so.1".to_string()),
        ("mips", "musleabi") => Some("/lib/ld-musl-mips-sf.so.1".to_string()),
        ("mips", "musleabihf") => Some("/lib/ld-musl-mips.so.1".to_string()),
        ("mipsel", "musleabi") => Some("/lib/ld-musl-mipsel-sf.so.1".to_string()),
        ("mipsel", "musleabihf") => Some("/lib/ld-musl-mipsel.so.1".to_string()),
        ("mips64", "muslabi64") => Some("/lib/ld-musl-mips64.so.1".to_string()),
        ("mips64el", "muslabi64") => Some("/lib/ld-musl-mips64el.so.1".to_string()),
        ("mips64", "muslabin32") => Some("/lib/ld-musl-mipsn32.so.1".to_string()),
        ("mips64el", "muslabin32") => Some("/lib/ld-musl-mipsn32el.so.1".to_string()),
        ("s390x", "musl") => Some("/lib/ld-musl-s390x.so.1".to_string()),
        ("loongarch64", "musl") => Some("/lib/ld-musl-loongarch64.so.1".to_string()),
        ("m68k", "musl") => Some("/lib/ld-musl-m68k.so.1".to_string()),
        ("microblaze", "musl") => Some("/lib/ld-musl-microblaze.so.1".to_string()),
        ("microblazeel", "musl") => Some("/lib/ld-musl-microblazeel.so.1".to_string()),
        _ => None,
    }
}

fn is_gnu_abi(abi: &str) -> bool {
    matches!(
        abi,
        "gnu" | "gnux32" | "gnueabi" | "gnueabihf" | "gnuabi64" | "gnuabin32" | "gnuf32" | "gnusf"
    )
}

fn is_musl_abi(abi: &str) -> bool {
    matches!(
        abi,
        "musl"
            | "muslx32"
            | "musleabi"
            | "musleabihf"
            | "muslabi64"
            | "muslabin32"
            | "muslf32"
            | "muslsf"
    )
}

fn default_host_abi() -> String {
    default_abi_for_os(env::consts::OS)
}

fn default_abi_for_os(os: &str) -> String {
    match os {
        "linux" => "gnu".to_string(),
        _ => String::new(),
    }
}

fn elf_interpreter(path: &PathBuf) -> Result<Option<String>, LinkerConfigError> {
    const EI_CLASS: usize = 4;
    const EI_DATA: usize = 5;
    const ELFCLASS64: u8 = 2;
    const ELFDATA2LSB: u8 = 1;
    const PT_INTERP: u32 = 3;
    const ELF_HEADER_LEN: usize = 64;
    const PROGRAM_HEADER_LEN_64: usize = 56;

    let bytes = fs::read(path).map_err(|error| LinkerConfigError::Io {
        path: path.clone(),
        error,
    })?;
    if bytes.len() < ELF_HEADER_LEN
        || &bytes[0..4] != b"\x7fELF"
        || bytes[EI_CLASS] != ELFCLASS64
        || bytes[EI_DATA] != ELFDATA2LSB
    {
        return Err(LinkerConfigError::InvalidElf { path: path.clone() });
    }

    let phoff = read_u64(&bytes, 32)
        .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
        as usize;
    let phentsize = read_u16(&bytes, 54)
        .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
        as usize;
    let phnum = read_u16(&bytes, 56)
        .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
        as usize;
    if phentsize < PROGRAM_HEADER_LEN_64 {
        return Err(LinkerConfigError::InvalidElf { path: path.clone() });
    }

    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        let Some(p_type) = read_u32(&bytes, offset) else {
            return Err(LinkerConfigError::InvalidElf { path: path.clone() });
        };
        if p_type != PT_INTERP {
            continue;
        }
        let p_offset = read_u64(&bytes, offset + 8)
            .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
            as usize;
        let p_filesz = read_u64(&bytes, offset + 32)
            .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
            as usize;
        let Some(slice) = bytes.get(p_offset..p_offset + p_filesz) else {
            return Err(LinkerConfigError::InvalidElf { path: path.clone() });
        };
        let nul = slice
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(slice.len());
        return String::from_utf8(slice[..nul].to_vec())
            .map(Some)
            .map_err(|_| LinkerConfigError::InvalidElf { path: path.clone() });
    }
    Ok(None)
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let array = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(array))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let array = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let array = bytes.get(offset..offset + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_static_gnu_invocation_keeps_freestanding_shape() {
        let options = LinkOptions {
            linker: ExecutableLinker::with_program("ld"),
            ..LinkOptions::default()
        };
        let invocation = options
            .invocation(&[PathBuf::from("main.o")], PathBuf::from("main"))
            .expect("link invocation");
        assert_eq!(invocation.program, "ld");
        assert_eq!(
            invocation.args,
            vec!["-e", "_start", "main.o", "-static", "-o", "main"]
        );
    }

    #[test]
    fn dynamic_gnu_invocation_accepts_structured_options() {
        let options = LinkOptions {
            linker: ExecutableLinker::with_program("ld"),
            ..LinkOptions::default()
        }
        .with_dynamic_mode()
        .with_dynamic_linker(DynamicLinker::Path("/loader".to_string()))
        .add_library_path("/lib")
        .add_rpath("$ORIGIN")
        .add_library("nia_capi")
        .with_raw_args(vec!["-z".to_string(), "now".to_string()]);
        let invocation = options
            .invocation(&[PathBuf::from("main.o")], PathBuf::from("main"))
            .expect("link invocation");
        assert_eq!(
            invocation.args,
            vec![
                "-e",
                "_start",
                "main.o",
                "-L",
                "/lib",
                "-rpath",
                "$ORIGIN",
                "-l",
                "nia_capi",
                "--dynamic-linker",
                "/loader",
                "-z",
                "now",
                "-o",
                "main"
            ]
        );
    }

    #[test]
    fn static_gnu_invocation_selects_static_libraries_before_library_search() {
        let options = LinkOptions {
            linker: ExecutableLinker::with_program("ld"),
            ..LinkOptions::default()
        }
        .add_library_path("/lib")
        .add_library("nia_capi");
        let invocation = options
            .invocation(&[PathBuf::from("main.o")], PathBuf::from("main"))
            .expect("link invocation");
        let static_index = invocation
            .args
            .iter()
            .position(|arg| arg == "-static")
            .expect("-static argument");
        let library_index = invocation
            .args
            .iter()
            .position(|arg| arg == "-l")
            .expect("-l argument");
        assert!(
            static_index < library_index,
            "static mode must be selected before library lookup: {:?}",
            invocation.args
        );
    }

    #[test]
    fn lld_invocation_uses_gnu_like_arguments() {
        let options = LinkOptions {
            linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
            ..LinkOptions::default()
        }
        .add_library_path("/lib")
        .add_library("m");
        let invocation = options
            .invocation(&[PathBuf::from("main.o")], PathBuf::from("main"))
            .expect("link invocation");
        assert_eq!(invocation.program, "ld.lld");
        assert!(
            invocation
                .args
                .windows(2)
                .any(|args| args == ["-e", "_start"])
        );
        assert!(invocation.args.iter().any(|arg| arg == "main.o"));
        assert!(
            invocation
                .args
                .windows(2)
                .any(|args| args == ["-L", "/lib"])
        );
        assert!(invocation.args.windows(2).any(|args| args == ["-l", "m"]));
        assert!(
            invocation
                .args
                .windows(2)
                .any(|args| args == ["-o", "main"])
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn lld_invocation_adds_native_linux_library_paths() {
        let options = LinkOptions {
            linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
            ..LinkOptions::default()
        };
        let invocation = options
            .invocation(&[PathBuf::from("main.o")], PathBuf::from("main"))
            .expect("link invocation");
        assert!(
            invocation
                .args
                .windows(2)
                .any(|args| args == ["-L", "/usr/lib64"] || args == ["-L", "/lib64"]),
            "{:?}",
            invocation.args
        );
    }

    #[test]
    fn lld_invocation_resolves_program_from_path() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        let root = env::temp_dir().join(format!("nia-linker-lld-path-{}", std::process::id()));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("create bin dir");
        let linker = bin.join("ld.lld");
        fs::write(&linker, "").expect("write mock linker");
        make_executable(&linker);
        let previous_path = env::var_os("PATH");
        let previous_nia_lld = env::var_os("NIA_LLD");
        unsafe {
            env::set_var("PATH", &bin);
            env::remove_var("NIA_LLD");
        }

        let options = LinkOptions {
            linker: ExecutableLinker::lld(),
            ..LinkOptions::default()
        };
        let invocation = options
            .invocation(&[PathBuf::from("main.o")], PathBuf::from("main"))
            .expect("link invocation");
        assert_eq!(invocation.program, linker.to_string_lossy());

        restore_env("PATH", previous_path);
        restore_env("NIA_LLD", previous_nia_lld);
    }

    #[test]
    #[cfg(unix)]
    fn lld_invocation_ignores_non_executable_program_on_path() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        let root = env::temp_dir().join(format!(
            "nia-linker-lld-non-executable-{}",
            std::process::id()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("create bin dir");
        fs::write(bin.join("ld.lld"), "").expect("write mock linker");
        let previous_path = env::var_os("PATH");
        let previous_nia_lld = env::var_os("NIA_LLD");
        unsafe {
            env::set_var("PATH", &bin);
            env::remove_var("NIA_LLD");
        }

        let options = LinkOptions {
            linker: ExecutableLinker::lld(),
            ..LinkOptions::default()
        };
        assert!(matches!(
            options.invocation(&[PathBuf::from("main.o")], PathBuf::from("main")),
            Err(LinkerConfigError::LinkerNotFound {
                flavor: LinkerFlavor::Lld,
                ..
            })
        ));

        restore_env("PATH", previous_path);
        restore_env("NIA_LLD", previous_nia_lld);
    }

    #[test]
    fn lld_invocation_reports_missing_program() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        let previous_path = env::var_os("PATH");
        let previous_nia_lld = env::var_os("NIA_LLD");
        unsafe {
            env::set_var("PATH", "");
            env::remove_var("NIA_LLD");
        }

        let options = LinkOptions {
            linker: ExecutableLinker::lld(),
            ..LinkOptions::default()
        };
        assert!(matches!(
            options.invocation(&[PathBuf::from("main.o")], PathBuf::from("main")),
            Err(LinkerConfigError::LinkerNotFound {
                flavor: LinkerFlavor::Lld,
                ..
            })
        ));

        restore_env("PATH", previous_path);
        restore_env("NIA_LLD", previous_nia_lld);
    }

    #[test]
    fn self_hosted_elf_flavor_is_reserved() {
        let options = LinkOptions {
            linker: ExecutableLinker::with_program_and_flavor(
                "nia-link",
                LinkerFlavor::SelfHostedElf,
            ),
            ..LinkOptions::default()
        };
        assert!(matches!(
            options.invocation(&[PathBuf::from("main.o")], PathBuf::from("main")),
            Err(LinkerConfigError::UnsupportedFlavor(
                LinkerFlavor::SelfHostedElf
            ))
        ));
    }

    #[test]
    fn standard_dynamic_linker_covers_common_linux_gnu_targets() {
        assert_eq!(
            standard_dynamic_linker_for(&target("x86_64", "linux", "gnu")).as_deref(),
            Some("/lib64/ld-linux-x86-64.so.2")
        );
        assert_eq!(
            standard_dynamic_linker_for(&target("aarch64", "linux", "gnu")).as_deref(),
            Some("/lib/ld-linux-aarch64.so.1")
        );
        assert_eq!(
            standard_dynamic_linker_for(&target("riscv64", "linux", "gnu")).as_deref(),
            Some("/lib/ld-linux-riscv64-lp64d.so.1")
        );
    }

    #[test]
    fn standard_dynamic_linker_covers_common_linux_musl_targets() {
        assert_eq!(
            standard_dynamic_linker_for(&target("x86_64", "linux", "musl")).as_deref(),
            Some("/lib/ld-musl-x86_64.so.1")
        );
        assert_eq!(
            standard_dynamic_linker_for(&target("aarch64", "linux", "musl")).as_deref(),
            Some("/lib/ld-musl-aarch64.so.1")
        );
        assert_eq!(
            standard_dynamic_linker_for(&target("arm", "linux", "musleabihf")).as_deref(),
            Some("/lib/ld-musl-armhf.so.1")
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn detects_native_dynamic_linker_from_usr_bin_env() {
        let dynamic_linker = native_dynamic_linker().expect("native dynamic linker");
        assert!(
            dynamic_linker
                .as_deref()
                .is_some_and(|path| path.contains("ld-linux") || path.contains("ld-musl")),
            "{dynamic_linker:?}"
        );
    }

    #[test]
    fn ld_so_conf_reader_follows_simple_include_patterns() {
        let root = env::temp_dir().join(format!("nia-linker-ld-so-conf-{}", std::process::id()));
        let lib = root.join("lib");
        let conf_dir = root.join("conf.d");
        fs::create_dir_all(&lib).expect("create lib dir");
        fs::create_dir_all(&conf_dir).expect("create conf dir");
        fs::write(
            root.join("ld.so.conf"),
            format!("include {}\n", conf_dir.join("*.conf").display()),
        )
        .expect("write root conf");
        fs::write(conf_dir.join("local.conf"), format!("{}\n", lib.display()))
            .expect("write included conf");

        let mut paths = Vec::new();
        read_ld_so_conf(&mut paths, &root.join("ld.so.conf"), 0);

        assert!(
            paths.contains(&lib.to_string_lossy().into_owned()),
            "{paths:?}"
        );
    }

    fn target(arch: &str, os: &str, abi: &str) -> LinkTarget {
        LinkTarget {
            arch: arch.to_string(),
            os: os.to_string(),
            abi: abi.to_string(),
        }
    }

    fn restore_env(name: &str, value: Option<std::ffi::OsString>) {
        unsafe {
            if let Some(value) = value {
                env::set_var(name, value);
            } else {
                env::remove_var(name);
            }
        }
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(path)
                .expect("mock linker metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("make mock linker executable");
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }
}
