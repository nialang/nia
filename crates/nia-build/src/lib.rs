// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fmt, io,
    path::{Path, PathBuf},
};

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
    pub build_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub compiler_link_plan: nia_linker::ToolchainLinkPlan,
    pub step: BuildStepSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStepSelection {
    Default,
    Named(String),
}

#[derive(Debug)]
pub enum BuildError {
    CurrentDirectory { error: io::Error },
    CurrentExecutable { error: io::Error },
    MissingBuildScript { start: PathBuf },
    UnsupportedRunner { plan: BuildPlan },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory { error } => {
                write!(f, "failed to read current directory: {error}")
            }
            Self::CurrentExecutable { error } => {
                write!(f, "failed to locate current Nia executable: {error}")
            }
            Self::MissingBuildScript { start } => write!(
                f,
                "failed to find `build.nia` from `{}` or any parent directory",
                start.display()
            ),
            Self::UnsupportedRunner { plan } => write!(
                f,
                "`nia build` found `{}` but the native build runner is not implemented yet",
                plan.build_script.display()
            ),
        }
    }
}

impl std::error::Error for BuildError {}

pub fn run_build(request: BuildRequest) -> Result<(), BuildError> {
    let plan = resolve_build_plan(request)?;
    Err(BuildError::UnsupportedRunner { plan })
}

pub fn resolve_build_plan(request: BuildRequest) -> Result<BuildPlan, BuildError> {
    let start = match request.root {
        Some(root) => root,
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let package_root = find_package_root(&start)?;
    let build_script = package_root.join("build.nia");
    let compiler_link_plan = nia_linker::ToolchainLinkPlan::compiler_hosted_development(
        nia_linker::CompilerHostedLayout::new(current_toolchain_library_dir()?),
    );
    Ok(BuildPlan {
        build_dir: package_root.join(".nia-build"),
        cache_dir: package_root.join(".nia-cache"),
        package_root,
        build_script,
        compiler_link_plan,
        step: request
            .step
            .map(BuildStepSelection::Named)
            .unwrap_or(BuildStepSelection::Default),
    })
}

fn current_toolchain_library_dir() -> Result<String, BuildError> {
    let executable = env::current_exe().map_err(|error| BuildError::CurrentExecutable { error })?;
    let Some(parent) = executable.parent() else {
        return Ok(PathBuf::from(".").to_string_lossy().into_owned());
    };
    let library_dir = if parent.file_name().is_some_and(|name| name == "deps") {
        parent.to_path_buf()
    } else {
        parent.join("deps")
    };
    Ok(library_dir.to_string_lossy().into_owned())
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
        assert_eq!(plan.build_dir, plan.package_root.join(".nia-build"));
        assert_eq!(plan.cache_dir, plan.package_root.join(".nia-cache"));
        assert_eq!(
            plan.compiler_link_plan.libraries,
            vec![
                nia_linker::NativeLibrary::static_("nia_capi"),
                nia_linker::NativeLibrary::dynamic("LLVM"),
                nia_linker::NativeLibrary::dynamic(":libgcc_s.so.1"),
                nia_linker::NativeLibrary::dynamic("c"),
            ]
        );
        assert_eq!(plan.step, BuildStepSelection::Default);
    }

    #[test]
    fn preserves_named_step() {
        let root = temp_root("preserves_named_step");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_plan(BuildRequest::new().with_root(&root).with_step("install"))
            .expect("build plan");

        assert_eq!(plan.step, BuildStepSelection::Named("install".to_string()));
    }

    #[test]
    fn reports_missing_build_script_from_start_directory() {
        let root = temp_root("reports_missing_build_script_from_start_directory");

        let error = resolve_build_plan(BuildRequest::new().with_root(&root))
            .expect_err("missing build script");

        assert!(matches!(error, BuildError::MissingBuildScript { start } if start == root));
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
