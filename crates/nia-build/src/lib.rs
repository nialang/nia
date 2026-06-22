// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use nia_driver::{CheckRequest, Driver, DriverError, LinkExecutableRequest};
use nia_imports::ModuleMap;
use nia_source::SourcePath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub root: Option<PathBuf>,
    pub step: Option<String>,
}

impl BuildRequest {
    pub fn new() -> Self {
        Self {
            root: None,
            step: None,
        }
    }

    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    pub fn with_step(mut self, step: impl Into<String>) -> Self {
        self.step = Some(step.into());
        self
    }
}

impl Default for BuildRequest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub package_root: PathBuf,
    pub build_script: PathBuf,
    pub toolchain_executable: PathBuf,
    pub build_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runner_dir: PathBuf,
    pub runner_executable: PathBuf,
    pub step: BuildStepSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStepSelection {
    Default,
    Named(String),
}

impl BuildStepSelection {
    fn as_runner_arg(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Named(step) => Some(step),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRunnerSource {
    pub path: String,
    pub source: String,
}

#[derive(Debug)]
pub enum BuildError {
    CurrentDirectory {
        error: io::Error,
    },
    CurrentExecutable {
        error: io::Error,
    },
    CreateBuildDirectory {
        path: PathBuf,
        error: io::Error,
    },
    CreateCacheDirectory {
        path: PathBuf,
        error: io::Error,
    },
    NonUtf8Path {
        role: &'static str,
        path: PathBuf,
    },
    CreateRunnerDirectory {
        path: PathBuf,
        error: io::Error,
    },
    CompileRunner {
        path: String,
        source: String,
        error: DriverError,
    },
    RunRunner {
        path: PathBuf,
        error: io::Error,
    },
    RunnerFailed {
        path: PathBuf,
        status: ExitStatus,
    },
    MissingBuildScript {
        start: PathBuf,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory { error } => {
                write!(f, "failed to read current directory: {error}")
            }
            Self::CurrentExecutable { error } => {
                write!(
                    f,
                    "failed to read current toolchain executable path: {error}"
                )
            }
            Self::CreateBuildDirectory { path, error } => {
                write!(
                    f,
                    "failed to create build directory `{}`: {error}",
                    path.display()
                )
            }
            Self::CreateCacheDirectory { path, error } => {
                write!(
                    f,
                    "failed to create build cache directory `{}`: {error}",
                    path.display()
                )
            }
            Self::NonUtf8Path { role, path } => {
                write!(
                    f,
                    "failed to encode {role} path `{}` as Nia source: path is not valid UTF-8",
                    path.display()
                )
            }
            Self::CreateRunnerDirectory { path, error } => {
                write!(
                    f,
                    "failed to create build runner directory `{}`: {error}",
                    path.display()
                )
            }
            Self::CompileRunner {
                path,
                source,
                error,
            } => {
                write!(
                    f,
                    "failed to compile build runner `{path}`\n{}",
                    nia_driver::render_driver_error(error, Some(path), Some(source))
                )
            }
            Self::RunRunner { path, error } => {
                write!(
                    f,
                    "failed to run build runner `{}`: {error}",
                    path.display()
                )
            }
            Self::RunnerFailed { path, status } => {
                write!(
                    f,
                    "build runner `{}` exited with status {status}",
                    path.display()
                )
            }
            Self::MissingBuildScript { start } => write!(
                f,
                "failed to find `build.nia` from `{}` or any parent directory",
                start.display()
            ),
        }
    }
}

impl std::error::Error for BuildError {}

pub fn run_build(request: BuildRequest) -> Result<(), BuildError> {
    let plan = resolve_build_plan(request)?;
    prepare_build_directories(&plan)?;
    compile_build_runner(&plan)?;
    run_build_runner(&plan)
}

pub fn resolve_build_plan(request: BuildRequest) -> Result<BuildPlan, BuildError> {
    let start = match request.root {
        Some(root) => root,
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let toolchain_executable =
        env::current_exe().map_err(|error| BuildError::CurrentExecutable { error })?;
    let package_root = find_package_root(&start)?;
    let build_script = package_root.join("build.nia");
    let build_dir = package_root.join(".nia-build");
    let runner_dir = build_dir.join("runner");
    Ok(BuildPlan {
        cache_dir: package_root.join(".nia-cache"),
        runner_executable: runner_dir.join("nia-build-runner"),
        runner_dir,
        build_dir,
        toolchain_executable,
        package_root,
        build_script,
        step: request
            .step
            .map(BuildStepSelection::Named)
            .unwrap_or(BuildStepSelection::Default),
    })
}

pub fn build_runner_source(plan: &BuildPlan) -> Result<BuildRunnerSource, BuildError> {
    let path = plan
        .runner_dir
        .join("root.nia")
        .to_string_lossy()
        .into_owned();
    build_runner_source_for_path(plan, path)
}

fn build_runner_source_for_path(
    plan: &BuildPlan,
    path: String,
) -> Result<BuildRunnerSource, BuildError> {
    let package_root = nia_path_literal("package root", &plan.package_root)?;
    let build_dir = nia_path_literal("build dir", &plan.build_dir)?;
    let cache_dir = nia_path_literal("cache dir", &plan.cache_dir)?;
    let toolchain_executable =
        nia_path_literal("toolchain executable", &plan.toolchain_executable)?;
    let source = r#"
using std::build;
using std::fs;
using std::mem;
using std::process;
using build_script;

pub fn main(init: process::Init) process::ExitCode!void {
    var page_allocator = mem::PageAllocator::init();
    var allocator = mem::GeneralPurposeAllocator::init(&mut page_allocator);
    defer allocator.deinit().ok().exit().?;

    var api = build::Build::init(
        init,
        &mut allocator,
        fs::Path::init({package_root}),
        fs::Path::init({build_dir}),
        fs::Path::init({cache_dir}),
        fs::Path::init({toolchain_executable}),
    );
    defer api.deinit().exit().?;

    build_script::build(&mut api).exit().?;
    api.run_requested_step().exit().?;
    !{}
}
"#
    .trim_start()
    .replace("{package_root}", &package_root)
    .replace("{build_dir}", &build_dir)
    .replace("{cache_dir}", &cache_dir)
    .replace("{toolchain_executable}", &toolchain_executable);
    Ok(BuildRunnerSource { path, source })
}

fn compile_build_runner(plan: &BuildPlan) -> Result<(), BuildError> {
    fs::create_dir_all(&plan.runner_dir).map_err(|error| BuildError::CreateRunnerDirectory {
        path: plan.runner_dir.clone(),
        error,
    })?;
    let runner = build_runner_source(plan)?;
    let driver = Driver::new();
    driver.set_source(runner.path.clone(), runner.source.clone());
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "build_script",
        SourcePath::new(plan.build_script.to_string_lossy().into_owned()),
    );
    let output = driver.link_executable(LinkExecutableRequest::new(
        CheckRequest::new(runner.path.clone()).with_module_map(module_map),
        &plan.runner_executable,
    ));
    output
        .result
        .map(|_| ())
        .map_err(|error| BuildError::CompileRunner {
            path: runner.path,
            source: runner.source,
            error,
        })
}

fn prepare_build_directories(plan: &BuildPlan) -> Result<(), BuildError> {
    fs::create_dir_all(&plan.build_dir).map_err(|error| BuildError::CreateBuildDirectory {
        path: plan.build_dir.clone(),
        error,
    })?;
    fs::create_dir_all(&plan.cache_dir).map_err(|error| BuildError::CreateCacheDirectory {
        path: plan.cache_dir.clone(),
        error,
    })
}

fn nia_path_literal(role: &'static str, path: &Path) -> Result<String, BuildError> {
    let Some(text) = path.to_str() else {
        return Err(BuildError::NonUtf8Path {
            role,
            path: path.to_path_buf(),
        });
    };
    Ok(nia_string_literal(text))
}

fn nia_string_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\0' => out.push_str("\\0"),
            ch if ch.is_control() => {
                use std::fmt::Write;
                write!(&mut out, "\\u{{{:x}}}", ch as u32).expect("writing to String cannot fail");
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn run_build_runner(plan: &BuildPlan) -> Result<(), BuildError> {
    let mut command = Command::new(&plan.runner_executable);
    command.current_dir(&plan.package_root);
    if let Some(step) = plan.step.as_runner_arg() {
        command.arg(step);
    }
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(BuildError::RunnerFailed {
            path: plan.runner_executable.clone(),
            status,
        }),
        Err(error) => Err(BuildError::RunRunner {
            path: plan.runner_executable.clone(),
            error,
        }),
    }
}

fn find_package_root(start: &Path) -> Result<PathBuf, BuildError> {
    let mut cursor = start.to_path_buf();
    loop {
        if cursor.join("build.nia").is_file() {
            return Ok(cursor);
        }
        if !cursor.pop() {
            return Err(BuildError::MissingBuildScript {
                start: start.to_path_buf(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_package_root_from_child_directory() {
        let root = temp_root("resolves_package_root_from_child_directory");
        let child = root.join("src").join("nested");
        std::fs::create_dir_all(&child).expect("create child");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_plan(BuildRequest::new().with_root(&child)).expect("build plan");

        assert_eq!(plan.package_root, root);
        assert_eq!(plan.build_script, plan.package_root.join("build.nia"));
        assert!(plan.toolchain_executable.is_file());
        assert_eq!(plan.build_dir, plan.package_root.join(".nia-build"));
        assert_eq!(plan.cache_dir, plan.package_root.join(".nia-cache"));
        assert_eq!(plan.runner_dir, plan.package_root.join(".nia-build/runner"));
        assert_eq!(
            plan.runner_executable,
            plan.package_root.join(".nia-build/runner/nia-build-runner")
        );
        assert_eq!(plan.step, BuildStepSelection::Default);
    }

    #[test]
    fn generated_runner_invokes_build_script_as_normal_nia_module() {
        let root = temp_root("generated_runner_invokes_build_script_as_normal_nia_module");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");

        let runner = build_runner_source(&plan).expect("build runner source");

        assert_eq!(
            runner.path,
            plan.runner_dir.join("root.nia").to_string_lossy()
        );
        assert!(runner.source.contains("using std::build;"));
        assert!(runner.source.contains("using std::fs;"));
        assert!(runner.source.contains("using std::mem;"));
        assert!(runner.source.contains("using build_script;"));
        assert!(runner.source.contains("var api = build::Build::init("));
        assert!(runner.source.contains("fs::Path::init("));
        assert!(runner.source.contains(&format!(
            "fs::Path::init({})",
            nia_string_literal(root.to_str().expect("utf-8 temp path"))
        )));
        assert!(runner.source.contains(&format!(
            "fs::Path::init({})",
            nia_string_literal(root.join(".nia-build").to_str().expect("utf-8 build path"))
        )));
        assert!(runner.source.contains(&format!(
            "fs::Path::init({})",
            nia_string_literal(root.join(".nia-cache").to_str().expect("utf-8 cache path"))
        )));
        assert!(runner.source.contains(&format!(
            "fs::Path::init({})",
            nia_string_literal(
                plan.toolchain_executable
                    .to_str()
                    .expect("utf-8 toolchain executable path")
            )
        )));
        assert!(
            runner
                .source
                .contains("build_script::build(&mut api).exit().?;")
        );
        assert!(!runner.source.contains("comptime let"));
    }

    #[test]
    fn preserves_named_step() {
        let root = temp_root("preserves_named_step");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_plan(BuildRequest::new().with_root(&root).with_step("install"))
            .expect("build plan");

        assert_eq!(plan.step, BuildStepSelection::Named("install".to_string()));
        assert_eq!(plan.step.as_runner_arg(), Some("install"));
    }

    #[test]
    fn reports_missing_build_script_from_start_directory() {
        let root = temp_root("reports_missing_build_script_from_start_directory");

        let error = resolve_build_plan(BuildRequest::new().with_root(&root))
            .expect_err("missing build script");

        assert!(matches!(error, BuildError::MissingBuildScript { start } if start == root));
    }

    #[test]
    fn prepares_build_and_cache_directories() {
        let root = temp_root("prepares_build_and_cache_directories");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");

        prepare_build_directories(&plan).expect("prepare build directories");

        assert!(plan.build_dir.is_dir());
        assert!(plan.cache_dir.is_dir());
    }

    #[test]
    fn escapes_paths_in_generated_runner_source() {
        let root = temp_root("escapes_paths_in_generated_runner_source");
        let package_root = root.join("quote\"slash\\tab\t");
        std::fs::create_dir_all(&package_root).expect("create package root");
        std::fs::write(package_root.join("build.nia"), "").expect("write build script");

        let plan =
            resolve_build_plan(BuildRequest::new().with_root(&package_root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");

        assert!(runner.source.contains("\\\""));
        assert!(runner.source.contains("\\\\"));
        assert!(runner.source.contains("\\t"));
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("nia-build-{name}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("remove old temp root");
        }
        std::fs::create_dir_all(&root).expect("create temp root");
        root
    }
}
