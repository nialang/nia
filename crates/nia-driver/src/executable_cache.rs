// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_imports::ModuleMap;
use nia_linker::LinkOptions;
use nia_loader_query::{EntryRuntime, LoadRequest};
use nia_opt::NiaOptimizationLevel;
use nia_source::{SourceDatabase, SourcePath};
use nia_symbol::symbol_identity_key;
use nia_target_config::TargetConfig;

use crate::Runtime;

static EXECUTABLE_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);
const EXECUTABLE_ARTIFACT_FINGERPRINT_VERSION: &str = "nia-emit-exe-artifact-v1";
const EXECUTABLE_ARTIFACT_MANIFEST_VERSION: &str = "nia-emit-exe-artifact-manifest-v1";

#[derive(Debug, Clone)]
pub struct ExecutableArtifactCacheRequest {
    pub entry_path: String,
    pub module_map: ModuleMap,
    pub optimization: NiaOptimizationLevel,
    pub runtime: Runtime,
    pub target: TargetConfig,
    pub link_options: LinkOptions,
    pub sources: SourceDatabase,
}

impl ExecutableArtifactCacheRequest {
    pub fn new(entry_path: impl Into<String>) -> Self {
        Self {
            entry_path: entry_path.into(),
            module_map: ModuleMap::default(),
            optimization: NiaOptimizationLevel::default(),
            runtime: Runtime::default(),
            target: TargetConfig::host(),
            link_options: LinkOptions::default(),
            sources: SourceDatabase::new(),
        }
    }

    pub fn with_module_map(mut self, module_map: ModuleMap) -> Self {
        self.module_map = module_map;
        self
    }

    pub fn with_optimization(mut self, optimization: NiaOptimizationLevel) -> Self {
        self.optimization = optimization;
        self
    }

    pub fn with_runtime(mut self, runtime: Runtime) -> Self {
        self.runtime = runtime;
        self
    }

    pub fn with_target(mut self, target: TargetConfig) -> Self {
        self.target = target;
        self
    }

    pub fn with_link_options(mut self, link_options: LinkOptions) -> Self {
        self.link_options = link_options;
        self
    }

    pub fn with_sources(mut self, sources: SourceDatabase) -> Self {
        self.sources = sources;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ExecutableArtifactCacheEntry {
    pub executable: PathBuf,
    pub cache_dir: PathBuf,
    pub snapshot: Option<ExecutableArtifactCacheSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableArtifactCacheSnapshot {
    pub request_hash: String,
    pub fingerprint: String,
    pub inputs: Vec<ExecutableArtifactCacheInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableArtifactCacheInput {
    pub path: String,
    pub generated: bool,
    pub content_len: u64,
    pub content_hash: String,
}

pub fn executable_artifact_cache_entry(
    request: ExecutableArtifactCacheRequest,
    cache_dir: &Path,
) -> Result<ExecutableArtifactCacheEntry, String> {
    let request_hash = executable_artifact_request_hash(&request);
    let generated = generated_source_inputs(&request.sources);
    if let Some(snapshot) =
        restore_executable_artifact_manifest(cache_dir, &request_hash, &generated)?
    {
        let executable = executable_artifact_cache_path(cache_dir, &snapshot.fingerprint);
        return Ok(ExecutableArtifactCacheEntry {
            executable,
            cache_dir: cache_dir.to_path_buf(),
            snapshot: Some(snapshot),
        });
    }
    let Some(inputs) = loaded_executable_module_inputs(&request)? else {
        let fingerprint = executable_artifact_fingerprint(&request_hash, &[]);
        return Ok(ExecutableArtifactCacheEntry {
            executable: executable_artifact_cache_path(cache_dir, &fingerprint),
            cache_dir: cache_dir.to_path_buf(),
            snapshot: None,
        });
    };
    let fingerprint = executable_artifact_fingerprint(&request_hash, &inputs);
    let snapshot = ExecutableArtifactCacheSnapshot {
        request_hash,
        fingerprint: fingerprint.clone(),
        inputs,
    };
    Ok(ExecutableArtifactCacheEntry {
        executable: executable_artifact_cache_path(cache_dir, &fingerprint),
        cache_dir: cache_dir.to_path_buf(),
        snapshot: Some(snapshot),
    })
}

pub fn restore_executable_artifact_cache(
    cache: &ExecutableArtifactCacheEntry,
    output: &Path,
) -> bool {
    if !cache.executable.is_file() {
        return false;
    }
    let Some(parent) = output.parent() else {
        return fs::copy(&cache.executable, output).is_ok();
    };
    if !parent.as_os_str().is_empty() && fs::create_dir_all(parent).is_err() {
        return false;
    }
    let staged = output.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        EXECUTABLE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    if fs::copy(&cache.executable, &staged).is_err() {
        let _ = fs::remove_file(&staged);
        return false;
    }
    if make_executable_like(&staged, &cache.executable).is_err() {
        let _ = fs::remove_file(&staged);
        return false;
    }
    match fs::rename(&staged, output) {
        Ok(()) => true,
        Err(_) => {
            let _ = fs::remove_file(&staged);
            false
        }
    }
}

pub fn publish_executable_artifact_cache(
    output: &Path,
    cache: &ExecutableArtifactCacheEntry,
) -> Result<(), String> {
    if cache.executable.is_file() {
        return Ok(());
    }
    let parent = cache
        .executable
        .parent()
        .ok_or_else(|| "invalid executable cache path".to_string())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create executable cache directory `{}`: {error}",
            parent.display()
        )
    })?;
    let staged = cache.executable.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        EXECUTABLE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::copy(output, &staged).map_err(|error| {
        format!(
            "failed to copy executable `{}` into cache `{}`: {error}",
            output.display(),
            staged.display()
        )
    })?;
    make_executable_like(&staged, output).map_err(|error| {
        format!(
            "failed to set executable cache permissions `{}`: {error}",
            staged.display()
        )
    })?;
    match fs::rename(&staged, &cache.executable) {
        Ok(()) => Ok(()),
        Err(error) if cache.executable.is_file() => {
            let _ = fs::remove_file(&staged);
            let _ = error;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&staged);
            Err(format!(
                "failed to publish executable cache `{}`: {error}",
                cache.executable.display()
            ))
        }
    }?;
    if let Some(snapshot) = &cache.snapshot {
        save_executable_artifact_manifest(cache, snapshot)?;
    }
    Ok(())
}

fn executable_artifact_request_hash(request: &ExecutableArtifactCacheRequest) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(EXECUTABLE_ARTIFACT_FINGERPRINT_VERSION);
    hash.string(env!("CARGO_PKG_VERSION"));
    hash.string(&request.entry_path);
    hash.string(&format!("{:?}", request.optimization));
    hash.string(&format!("{:?}", request.runtime));
    hash.string(&format!("{:?}", request.target));
    hash.string(&format!("{:?}", request.link_options));
    let mut module_entries = request
        .module_map
        .entries()
        .map(|(name, path)| (symbol_identity_key(name), path.as_str().to_string()))
        .collect::<Vec<_>>();
    module_entries.sort();
    for (name, path) in module_entries {
        hash.string(&name);
        hash.string(&path);
    }
    hash.finish()
}

fn executable_artifact_fingerprint(
    request_hash: &str,
    inputs: &[ExecutableArtifactCacheInput],
) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(request_hash);
    for input in inputs {
        hash.string(&input.path);
        hash.string(if input.generated { "generated" } else { "file" });
        hash.string(&input.content_len.to_string());
        hash.string(&input.content_hash);
    }
    hash.finish()
}

fn executable_artifact_cache_path(cache_dir: &Path, fingerprint: &str) -> PathBuf {
    cache_dir
        .join("artifacts")
        .join("executables")
        .join(fingerprint)
        .join("app")
}

fn loaded_executable_module_inputs(
    request: &ExecutableArtifactCacheRequest,
) -> Result<Option<Vec<ExecutableArtifactCacheInput>>, String> {
    let generated = generated_source_inputs(&request.sources);
    let loaded = nia_loader_query::load_program_request(
        LoadRequest::new(request.entry_path.clone())
            .with_module_map(request.module_map.clone())
            .with_sources(request.sources.clone())
            .with_target(request.target.clone())
            .with_entry_runtime(entry_runtime(request.runtime))
            .with_package_root_used_paths(true),
    );
    if !loaded.diagnostics.is_empty() {
        return Ok(None);
    }
    let mut modules = loaded
        .graph
        .modules()
        .map(|module| module.path.as_str().to_string())
        .collect::<Vec<_>>();
    modules.sort();
    modules.dedup();
    let mut inputs = Vec::with_capacity(modules.len());
    for module_path in modules {
        if let Some(input) = generated.get(&SourcePath::new(module_path.clone())) {
            inputs.push(input.clone());
        } else if module_path.starts_with("<nia:") {
            inputs.push(ExecutableArtifactCacheInput {
                path: module_path,
                generated: true,
                content_len: 0,
                content_hash: content_hash("<generated>"),
            });
        } else {
            match fs::read_to_string(&module_path) {
                Ok(source) => inputs.push(ExecutableArtifactCacheInput {
                    path: module_path,
                    generated: false,
                    content_len: source.len() as u64,
                    content_hash: content_hash(&source),
                }),
                Err(error) => {
                    return Err(format!(
                        "failed to read `{module_path}` for executable cache fingerprint: {error}"
                    ));
                }
            }
        }
    }
    Ok(Some(inputs))
}

fn generated_source_inputs(
    sources: &SourceDatabase,
) -> HashMap<SourcePath, ExecutableArtifactCacheInput> {
    sources
        .source_files()
        .into_iter()
        .map(|file| {
            let input = ExecutableArtifactCacheInput {
                path: file.path.as_str().to_string(),
                generated: true,
                content_len: file.text.len() as u64,
                content_hash: content_hash(&file.text),
            };
            (file.path, input)
        })
        .collect()
}

fn restore_executable_artifact_manifest(
    cache_dir: &Path,
    request_hash: &str,
    generated: &HashMap<SourcePath, ExecutableArtifactCacheInput>,
) -> Result<Option<ExecutableArtifactCacheSnapshot>, String> {
    let Some(snapshot) = read_executable_artifact_manifest(cache_dir, request_hash)? else {
        return Ok(None);
    };
    for input in &snapshot.inputs {
        let current = current_executable_artifact_input(input, generated)?;
        if current.content_len != input.content_len || current.content_hash != input.content_hash {
            return Ok(None);
        }
    }
    let fingerprint = executable_artifact_fingerprint(&snapshot.request_hash, &snapshot.inputs);
    if fingerprint == snapshot.fingerprint {
        Ok(Some(snapshot))
    } else {
        Ok(None)
    }
}

fn current_executable_artifact_input(
    input: &ExecutableArtifactCacheInput,
    generated: &HashMap<SourcePath, ExecutableArtifactCacheInput>,
) -> Result<ExecutableArtifactCacheInput, String> {
    if input.generated {
        if let Some(current) = generated.get(&SourcePath::new(input.path.clone())) {
            return Ok(current.clone());
        }
        if !input.path.starts_with("<nia:") {
            return Ok(ExecutableArtifactCacheInput {
                path: input.path.clone(),
                generated: true,
                content_len: 0,
                content_hash: content_hash("<missing-generated>"),
            });
        }
        return Ok(ExecutableArtifactCacheInput {
            path: input.path.clone(),
            generated: true,
            content_len: input.content_len,
            content_hash: input.content_hash.clone(),
        });
    }
    let source = fs::read_to_string(&input.path).map_err(|error| {
        format!(
            "failed to read `{}` for executable cache manifest: {error}",
            input.path
        )
    })?;
    Ok(ExecutableArtifactCacheInput {
        path: input.path.clone(),
        generated: false,
        content_len: source.len() as u64,
        content_hash: content_hash(&source),
    })
}

fn executable_artifact_manifest_path(cache_dir: &Path, request_hash: &str) -> PathBuf {
    cache_dir
        .join("artifacts")
        .join("executables")
        .join("manifests")
        .join(request_hash)
}

fn read_executable_artifact_manifest(
    cache_dir: &Path,
    request_hash: &str,
) -> Result<Option<ExecutableArtifactCacheSnapshot>, String> {
    let path = executable_artifact_manifest_path(cache_dir, request_hash);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read executable cache manifest `{}`: {error}",
                path.display()
            ));
        }
    };
    Ok(parse_executable_artifact_manifest(&text)
        .filter(|snapshot| snapshot.request_hash == request_hash))
}

fn save_executable_artifact_manifest(
    cache: &ExecutableArtifactCacheEntry,
    snapshot: &ExecutableArtifactCacheSnapshot,
) -> Result<(), String> {
    let path = executable_artifact_manifest_path(&cache.cache_dir, &snapshot.request_hash);
    let parent = path
        .parent()
        .ok_or_else(|| "invalid executable cache manifest path".to_string())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create executable cache manifest directory `{}`: {error}",
            parent.display()
        )
    })?;
    let staged = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        EXECUTABLE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&staged, format_executable_artifact_manifest(snapshot)).map_err(|error| {
        format!(
            "failed to write executable cache manifest `{}`: {error}",
            staged.display()
        )
    })?;
    match fs::rename(&staged, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&staged);
            Err(format!(
                "failed to publish executable cache manifest `{}`: {error}",
                path.display()
            ))
        }
    }
}

fn format_executable_artifact_manifest(snapshot: &ExecutableArtifactCacheSnapshot) -> String {
    let mut text = String::new();
    text.push_str(EXECUTABLE_ARTIFACT_MANIFEST_VERSION);
    text.push('\n');
    text.push_str("request\t");
    text.push_str(&snapshot.request_hash);
    text.push('\n');
    text.push_str("fingerprint\t");
    text.push_str(&snapshot.fingerprint);
    text.push('\n');
    for input in &snapshot.inputs {
        text.push_str("input\t");
        text.push_str(if input.generated { "generated" } else { "file" });
        text.push('\t');
        text.push_str(&input.content_len.to_string());
        text.push('\t');
        text.push_str(&input.content_hash);
        text.push('\t');
        text.push_str(&input.path);
        text.push('\n');
    }
    text
}

fn parse_executable_artifact_manifest(text: &str) -> Option<ExecutableArtifactCacheSnapshot> {
    let mut lines = text.lines();
    (lines.next()? == EXECUTABLE_ARTIFACT_MANIFEST_VERSION).then_some(())?;
    let request_hash = lines.next()?.strip_prefix("request\t")?.to_string();
    let fingerprint = lines.next()?.strip_prefix("fingerprint\t")?.to_string();
    if request_hash.is_empty() || fingerprint.is_empty() {
        return None;
    }
    let mut inputs = Vec::new();
    for line in lines {
        let mut fields = line.splitn(5, '\t');
        (fields.next()? == "input").then_some(())?;
        let generated = match fields.next()? {
            "generated" => true,
            "file" => false,
            _ => return None,
        };
        let content_len = fields.next()?.parse().ok()?;
        let content_hash = fields.next()?.to_string();
        let path = fields.next()?.to_string();
        if content_hash.is_empty() || path.is_empty() {
            return None;
        }
        inputs.push(ExecutableArtifactCacheInput {
            path,
            generated,
            content_len,
            content_hash,
        });
    }
    Some(ExecutableArtifactCacheSnapshot {
        request_hash,
        fingerprint,
        inputs,
    })
}

fn entry_runtime(runtime: Runtime) -> EntryRuntime {
    match runtime {
        Runtime::Bare => EntryRuntime::None,
        Runtime::Freestanding => EntryRuntime::Freestanding,
    }
}

fn make_executable_like(path: &Path, source: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::metadata(source)?.permissions())?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = source;
    }
    Ok(())
}

struct StableFingerprint {
    state: u64,
}

impl StableFingerprint {
    fn new() -> Self {
        Self {
            state: 0xcbf29ce484222325,
        }
    }

    fn string(&mut self, text: &str) {
        self.bytes(&(text.len() as u64).to_le_bytes());
        self.bytes(text.as_bytes());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(self) -> String {
        format!("{:016x}", self.state)
    }
}

fn content_hash(text: &str) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(text);
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_artifact_cache_fingerprint_tracks_loaded_source_graph() {
        let root = temp_root("executable_artifact_cache_fingerprint_tracks_loaded_source_graph");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
module helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    _ = helper::value();
    !{}
}
"#,
        )
        .expect("write main");
        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write helper");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");

        let before = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint before");
        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit helper");
        let after = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint after");

        assert_ne!(before.executable, after.executable);
    }

    #[test]
    fn executable_artifact_cache_fingerprint_ignores_unloaded_package_sources() {
        let root =
            temp_root("executable_artifact_cache_fingerprint_ignores_unloaded_package_sources");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
        )
        .expect("write main");
        std::fs::write(root.join("src/unused.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write unused");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");

        let before = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint before");
        std::fs::write(root.join("src/unused.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit unused");
        let after = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint after");

        assert_eq!(before.executable, after.executable);
    }

    #[test]
    fn executable_artifact_manifest_restores_unchanged_fingerprint() {
        let root = temp_root("executable_artifact_manifest_restores_unchanged_fingerprint");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
        )
        .expect("write main");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");
        let before = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint before");
        let snapshot = before.snapshot.clone().expect("snapshot");
        save_executable_artifact_manifest(&before, &snapshot).expect("save manifest");

        let after = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint after");

        assert_eq!(before.executable, after.executable);
    }

    #[test]
    fn executable_artifact_manifest_rejects_changed_loaded_source() {
        let root = temp_root("executable_artifact_manifest_rejects_changed_loaded_source");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
module helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    _ = helper::value();
    !{}
}
"#,
        )
        .expect("write main");
        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write helper");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");
        let before = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint before");
        let snapshot = before.snapshot.clone().expect("snapshot");
        save_executable_artifact_manifest(&before, &snapshot).expect("save manifest");

        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit helper");
        let after = executable_artifact_cache_entry(cache_request(&source), &cache_dir)
            .expect("fingerprint after");

        assert_ne!(before.executable, after.executable);
    }

    #[test]
    fn executable_artifact_cache_tracks_generated_entry_source() {
        let root = temp_root("executable_artifact_cache_tracks_generated_entry_source");
        std::fs::write(root.join("build.nia"), "pub fn build() void {}\n").expect("write build");
        let entry = root.join(".nia-build/runner/root.nia");
        let entry_text = "using build_script::*;\nfn main() void {}\n";
        let mut module_map = ModuleMap::new();
        module_map.insert(
            "build_script",
            nia_source::SourcePath::new(root.join("build.nia").to_string_lossy().into_owned()),
        );
        let sources = SourceDatabase::new();
        sources.set_source(
            SourcePath::new(entry.to_string_lossy().into_owned()),
            entry_text,
        );
        let request = ExecutableArtifactCacheRequest::new(entry.to_string_lossy().into_owned())
            .with_module_map(module_map)
            .with_sources(sources);
        let cache_dir = root.join(".nia-cache");

        let entry = executable_artifact_cache_entry(request, &cache_dir).expect("fingerprint");
        let snapshot = entry.snapshot.expect("snapshot");

        assert!(
            snapshot
                .inputs
                .iter()
                .any(|input| input.generated && input.content_hash == content_hash(entry_text))
        );
    }

    fn cache_request(source: &str) -> ExecutableArtifactCacheRequest {
        ExecutableArtifactCacheRequest::new(source)
            .with_runtime(Runtime::Freestanding)
            .with_optimization(NiaOptimizationLevel::default())
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nia-driver-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp root");
        root
    }
}
