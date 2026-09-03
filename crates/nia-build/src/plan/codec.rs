// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical registered build-plan encoding.
//!
//! The codec bounds every collection and payload, rejects unknown identities
//! and trailing data, and routes decoded drafts back through semantic freeze.

use super::*;
use std::fmt;

use nia_compat::formats::BUILD_PLAN;

pub(crate) const MAX_PLAN_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_ITEMS: usize = 100_000;
pub(crate) const MAX_PLAN_STRING_BYTES: usize = 1024 * 1024;

/// Rejection reason for a non-canonical, unsupported, or invalid plan encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanCodecError {
    /// Encoded payload exceeds the bounded plan size.
    TooLarge {
        /// Maximum accepted payload size.
        limit: usize,
        /// Actual payload size.
        actual: usize,
    },
    /// Payload magic does not match the registered format.
    BadMagic,
    /// Payload uses an unsupported schema version.
    UnsupportedVersion(u32),
    /// Payload ended before the required field at this offset.
    Truncated {
        /// Byte offset where decoding stopped.
        offset: usize,
    },
    /// Payload contains bytes after the canonical plan.
    TrailingData {
        /// Byte offset of the unexpected bytes.
        offset: usize,
    },
    /// A tagged value uses an unknown tag.
    InvalidTag {
        /// Value family whose tag was rejected.
        kind: &'static str,
        /// Unknown tag value.
        tag: u8,
        /// Byte offset containing the tag.
        offset: usize,
    },
    /// A string field is not valid UTF-8.
    InvalidUtf8 {
        /// Byte offset of the invalid string payload.
        offset: usize,
    },
    /// A stable name failed validation.
    InvalidStableName(StableNameError),
    /// A logical path failed validation.
    InvalidLogicalPath(LogicalPathError),
    /// Decoded fields failed semantic plan validation.
    Semantic(Box<PlanError>),
}

impl fmt::Display for PlanCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid build-plan encoding: {self:?}")
    }
}

impl std::error::Error for PlanCodecError {}

impl BuildPlan {
    /// Encodes this frozen plan in the registered canonical bounded format.
    pub fn encode(&self) -> Result<Vec<u8>, PlanCodecError> {
        let mut writer = Writer::new();
        writer.bytes(BUILD_PLAN.magic);
        writer.u32(self.schema_version);
        writer.package_key(&self.root_package)?;
        writer.count(self.packages.len())?;
        for package in &self.packages {
            writer.package_key(&package.key)?;
            writer.string(&package.root)?;
        }
        writer.target(&self.host_target)?;
        writer.target(&self.artifact_target)?;
        writer.count(self.modules.len())?;
        for module in &self.modules {
            writer.module(module)?;
        }
        writer.count(self.artifacts.len())?;
        for artifact in &self.artifacts {
            writer.artifact(artifact)?;
        }
        writer.count(self.actions.len())?;
        for action in &self.actions {
            writer.action(action)?;
        }
        writer.count(self.steps.len())?;
        for step in &self.steps {
            writer.step(step)?;
        }
        writer.option_step_key(self.default_step.as_ref())?;
        writer.option_step_key(self.selected_step.as_ref())?;
        writer.finish()
    }

    /// Decodes, bounds, and semantically freezes one canonical plan payload.
    pub fn decode(bytes: &[u8]) -> Result<Self, PlanCodecError> {
        if bytes.len() > MAX_PLAN_BYTES {
            return Err(PlanCodecError::TooLarge {
                limit: MAX_PLAN_BYTES,
                actual: bytes.len(),
            });
        }
        let mut reader = Reader::new(bytes);
        if reader.take(BUILD_PLAN.magic.len())? != BUILD_PLAN.magic {
            return Err(PlanCodecError::BadMagic);
        }
        let version = reader.u32()?;
        if version != BUILD_PLAN.schema {
            return Err(PlanCodecError::UnsupportedVersion(version));
        }
        let root_package = reader.package_key()?;
        let packages = reader.list(|reader| {
            Ok(PlanPackage {
                key: reader.package_key()?,
                root: reader.string()?,
            })
        })?;
        let host_target = reader.target()?;
        let artifact_target = reader.target()?;
        let modules = reader.list(Reader::module)?;
        let artifacts = reader.list(Reader::artifact)?;
        let actions = reader.list(Reader::action)?;
        let steps = reader.list(Reader::step)?;
        let default_step = reader.option_step_key()?;
        let selected_step = reader.option_step_key()?;
        if reader.offset != bytes.len() {
            return Err(PlanCodecError::TrailingData {
                offset: reader.offset,
            });
        }
        BuildPlan::freeze(BuildPlanDraft {
            root_package,
            packages,
            host_target,
            artifact_target,
            modules,
            artifacts,
            actions,
            steps,
            default_step,
            selected_step,
        })
        .map_err(|error| PlanCodecError::Semantic(Box::new(error)))
    }
}

struct Writer {
    bytes: Vec<u8>,
    max_bytes: usize,
    failure: Option<PlanCodecError>,
}

impl Writer {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes: MAX_PLAN_BYTES,
            failure: None,
        }
    }

    #[cfg(test)]
    fn with_limit(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
            failure: None,
        }
    }

    fn bytes(&mut self, bytes: &[u8]) {
        if self.failure.is_some() {
            return;
        }
        let actual = self.bytes.len().saturating_add(bytes.len());
        if actual > self.max_bytes {
            self.failure = Some(PlanCodecError::TooLarge {
                limit: self.max_bytes,
                actual,
            });
            return;
        }
        self.bytes.extend_from_slice(bytes);
    }
    fn u8(&mut self, value: u8) {
        self.bytes(std::slice::from_ref(&value));
    }
    fn u32(&mut self, value: u32) {
        self.bytes(value.to_le_bytes().as_slice());
    }

    fn count(&mut self, value: usize) -> Result<(), PlanCodecError> {
        if value > MAX_ITEMS || value > u32::MAX as usize {
            return Err(PlanCodecError::TooLarge {
                limit: MAX_ITEMS,
                actual: value,
            });
        }
        self.u32(value as u32);
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), PlanCodecError> {
        if value.len() > MAX_PLAN_STRING_BYTES || value.len() > u32::MAX as usize {
            return Err(PlanCodecError::TooLarge {
                limit: MAX_PLAN_STRING_BYTES,
                actual: value.len(),
            });
        }
        self.u32(value.len() as u32);
        self.bytes(value.as_bytes());
        Ok(())
    }

    fn blob(&mut self, value: &[u8]) -> Result<(), PlanCodecError> {
        if value.len() > MAX_PLAN_BYTES || value.len() > u32::MAX as usize {
            return Err(PlanCodecError::TooLarge {
                limit: MAX_PLAN_BYTES,
                actual: value.len(),
            });
        }
        self.u32(value.len() as u32);
        self.bytes(value);
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, PlanCodecError> {
        match self.failure {
            Some(error) => Err(error),
            None => {
                debug_assert!(self.bytes.len() <= self.max_bytes);
                Ok(self.bytes)
            }
        }
    }

    fn package_key(&mut self, key: &PackageKey) -> Result<(), PlanCodecError> {
        self.string(key.as_str())
    }

    fn node_key(&mut self, package: &PackageKey, name: &str) -> Result<(), PlanCodecError> {
        self.package_key(package)?;
        self.string(name)
    }

    fn module_key(&mut self, key: &ModuleKey) -> Result<(), PlanCodecError> {
        self.node_key(key.package(), key.name())
    }
    fn artifact_key(&mut self, key: &ArtifactKey) -> Result<(), PlanCodecError> {
        self.node_key(key.package(), key.name())
    }
    fn action_key(&mut self, key: &ActionKey) -> Result<(), PlanCodecError> {
        self.node_key(key.package(), key.name())
    }
    fn step_key(&mut self, key: &StepKey) -> Result<(), PlanCodecError> {
        self.node_key(key.package(), key.name())
    }

    fn logical_path(&mut self, path: &LogicalPath) -> Result<(), PlanCodecError> {
        match path.root() {
            LogicalPathRoot::Package(package) => {
                self.u8(0);
                self.package_key(package)?;
            }
            LogicalPathRoot::Build => self.u8(1),
            LogicalPathRoot::Cache => self.u8(2),
            LogicalPathRoot::Toolchain => self.u8(3),
            LogicalPathRoot::Artifact(artifact) => {
                self.u8(4);
                self.artifact_key(artifact)?;
            }
        }
        self.string(&path.protocol_path())
    }

    fn target(&mut self, target: &TargetSpec) -> Result<(), PlanCodecError> {
        for value in [
            &target.arch,
            &target.vendor,
            &target.os,
            &target.env,
            &target.abi,
            &target.endian,
        ] {
            self.string(value)?;
        }
        self.u32(target.pointer_width);
        Ok(())
    }

    fn optimization(&mut self, value: OptimizationMode) {
        self.u8(match value {
            OptimizationMode::O0 => 0,
            OptimizationMode::O1 => 1,
            OptimizationMode::O2 => 2,
            OptimizationMode::O3 => 3,
            OptimizationMode::Os => 4,
            OptimizationMode::Oz => 5,
        });
    }

    fn runtime(&mut self, value: Runtime) {
        self.u8(match value {
            Runtime::Bare => 0,
            Runtime::Freestanding => 1,
        });
    }

    fn module(&mut self, module: &PlanModule) -> Result<(), PlanCodecError> {
        self.module_key(&module.key)?;
        self.logical_path(&module.root_source)?;
        self.optimization(module.optimization);
        self.count(module.imports.len())?;
        for import in &module.imports {
            self.string(&import.name)?;
            self.logical_path(&import.path)?;
        }
        Ok(())
    }

    fn artifact(&mut self, artifact: &PlanArtifact) -> Result<(), PlanCodecError> {
        self.artifact_key(&artifact.key)?;
        self.module_key(&artifact.root_module)?;
        self.u8(match artifact.kind {
            PlanArtifactKind::Executable => 0,
            PlanArtifactKind::ObjectSet => 1,
            PlanArtifactKind::StaticArchive => 2,
        });
        self.logical_path(&artifact.output)?;
        self.runtime(artifact.runtime);
        Ok(())
    }

    fn action(&mut self, action: &PlanAction) -> Result<(), PlanCodecError> {
        self.action_key(&action.key)?;
        match &action.kind {
            ActionKind::CompilerCheck {
                module,
                target,
                runtime,
            } => {
                self.u8(0);
                self.module_key(module)?;
                self.target(target)?;
                self.runtime(*runtime);
            }
            ActionKind::CompilerEmit {
                artifact,
                target,
                static_archives,
            } => {
                self.u8(1);
                self.artifact_key(artifact)?;
                self.target(target)?;
                self.count(static_archives.len())?;
                for archive in static_archives {
                    self.artifact_key(archive)?;
                }
            }
            ActionKind::ExternalCommand {
                resource_class,
                environment_policy,
                cache_policy,
                program,
                arguments,
                working_directory,
                environment,
                inputs,
                outputs,
            }
            | ActionKind::TestExecutable {
                resource_class,
                environment_policy,
                cache_policy,
                program,
                arguments,
                working_directory,
                environment,
                inputs,
                outputs,
            } => {
                self.u8(
                    if matches!(&action.kind, ActionKind::TestExecutable { .. }) {
                        7
                    } else {
                        2
                    },
                );
                self.resource_class(*resource_class);
                self.command_environment_policy(*environment_policy);
                self.command_cache_policy(*cache_policy);
                match program {
                    CommandProgram::Path(path) => {
                        self.u8(0);
                        self.logical_path(path)?;
                    }
                    CommandProgram::Search(name) => {
                        self.u8(1);
                        self.string(name)?;
                    }
                }
                self.count(arguments.len())?;
                for argument in arguments {
                    match argument {
                        CommandArgument::Literal(value) => {
                            self.u8(0);
                            self.string(value)?;
                        }
                        CommandArgument::InputPath(path) => {
                            self.u8(1);
                            self.logical_path(path)?;
                        }
                        CommandArgument::OutputPath(path) => {
                            self.u8(2);
                            self.logical_path(path)?;
                        }
                    }
                }
                self.logical_path(working_directory)?;
                self.count(environment.len())?;
                for input in environment {
                    self.string(&input.name)?;
                    self.option_string(input.value.as_deref())?;
                }
                self.paths(inputs)?;
                self.paths(outputs)?;
            }
            ActionKind::GeneratedFile { output, contents } => {
                self.u8(3);
                self.logical_path(output)?;
                self.blob(contents)?;
            }
            ActionKind::InstallArtifact {
                artifact,
                destination,
            } => {
                self.u8(6);
                self.artifact_key(artifact)?;
                self.logical_path(destination)?;
            }
            ActionKind::Aggregate => self.u8(4),
            ActionKind::Uncacheable { description } => {
                self.u8(5);
                self.string(description)?;
            }
        }
        Ok(())
    }

    fn step(&mut self, step: &PlanStep) -> Result<(), PlanCodecError> {
        self.step_key(&step.key)?;
        self.action_key(&step.action)?;
        self.count(step.dependencies.len())?;
        for dependency in &step.dependencies {
            self.step_key(dependency)?;
        }
        Ok(())
    }

    fn paths(&mut self, values: &[LogicalPath]) -> Result<(), PlanCodecError> {
        self.count(values.len())?;
        for value in values {
            self.logical_path(value)?;
        }
        Ok(())
    }

    fn option_string(&mut self, value: Option<&str>) -> Result<(), PlanCodecError> {
        match value {
            Some(value) => {
                self.u8(1);
                self.string(value)
            }
            None => {
                self.u8(0);
                Ok(())
            }
        }
    }

    fn resource_class(&mut self, value: ActionResourceClass) {
        self.u8(match value {
            ActionResourceClass::Conservative => 0,
            ActionResourceClass::Cpu => 1,
            ActionResourceClass::Io => 2,
        });
    }

    fn command_environment_policy(&mut self, value: CommandEnvironmentPolicy) {
        self.u8(match value {
            CommandEnvironmentPolicy::Inherit => 0,
            CommandEnvironmentPolicy::Clear => 1,
        });
    }

    fn command_cache_policy(&mut self, value: CommandCachePolicy) {
        self.u8(match value {
            CommandCachePolicy::Uncacheable => 0,
            CommandCachePolicy::DeclaredInputs => 1,
        });
    }

    fn option_step_key(&mut self, value: Option<&StepKey>) -> Result<(), PlanCodecError> {
        match value {
            Some(value) => {
                self.u8(1);
                self.step_key(value)
            }
            None => {
                self.u8(0);
                Ok(())
            }
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], PlanCodecError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(PlanCodecError::Truncated {
                offset: self.offset,
            })?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(PlanCodecError::Truncated {
                offset: self.offset,
            })?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, PlanCodecError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, PlanCodecError> {
        let offset = self.offset;
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| PlanCodecError::Truncated { offset })?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn count(&mut self) -> Result<usize, PlanCodecError> {
        let value = self.u32()? as usize;
        if value > MAX_ITEMS {
            Err(PlanCodecError::TooLarge {
                limit: MAX_ITEMS,
                actual: value,
            })
        } else {
            Ok(value)
        }
    }

    fn list<T>(
        &mut self,
        mut read: impl FnMut(&mut Self) -> Result<T, PlanCodecError>,
    ) -> Result<Vec<T>, PlanCodecError> {
        let count = self.count()?;
        // Count is runner-controlled. Grow only after each item has consumed
        // and validated its bytes, so a truncated prefix cannot amplify a
        // four-byte count into count * size_of::<T>() of allocation.
        let mut values = Vec::new();
        for _ in 0..count {
            values.push(read(self)?);
        }
        Ok(values)
    }

    fn string(&mut self) -> Result<String, PlanCodecError> {
        let start = self.offset;
        let len = self.u32()? as usize;
        if len > MAX_PLAN_STRING_BYTES {
            return Err(PlanCodecError::TooLarge {
                limit: MAX_PLAN_STRING_BYTES,
                actual: len,
            });
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| PlanCodecError::InvalidUtf8 { offset: start })
    }

    fn blob(&mut self) -> Result<Vec<u8>, PlanCodecError> {
        let len = self.u32()? as usize;
        if len > MAX_PLAN_BYTES {
            return Err(PlanCodecError::TooLarge {
                limit: MAX_PLAN_BYTES,
                actual: len,
            });
        }
        Ok(self.take(len)?.to_vec())
    }

    fn package_key(&mut self) -> Result<PackageKey, PlanCodecError> {
        PackageKey::new(self.string()?).map_err(PlanCodecError::InvalidStableName)
    }

    fn node_parts(&mut self) -> Result<(PackageKey, String), PlanCodecError> {
        Ok((self.package_key()?, self.string()?))
    }

    fn module_key(&mut self) -> Result<ModuleKey, PlanCodecError> {
        let (package, name) = self.node_parts()?;
        ModuleKey::new(package, name).map_err(PlanCodecError::InvalidStableName)
    }
    fn artifact_key(&mut self) -> Result<ArtifactKey, PlanCodecError> {
        let (package, name) = self.node_parts()?;
        ArtifactKey::new(package, name).map_err(PlanCodecError::InvalidStableName)
    }
    fn action_key(&mut self) -> Result<ActionKey, PlanCodecError> {
        let (package, name) = self.node_parts()?;
        ActionKey::new(package, name).map_err(PlanCodecError::InvalidStableName)
    }
    fn step_key(&mut self) -> Result<StepKey, PlanCodecError> {
        let (package, name) = self.node_parts()?;
        StepKey::new(package, name).map_err(PlanCodecError::InvalidStableName)
    }

    fn logical_path(&mut self) -> Result<LogicalPath, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        let root = match tag {
            0 => LogicalPathRoot::Package(self.package_key()?),
            1 => LogicalPathRoot::Build,
            2 => LogicalPathRoot::Cache,
            3 => LogicalPathRoot::Toolchain,
            4 => LogicalPathRoot::Artifact(self.artifact_key()?),
            _ => {
                return Err(PlanCodecError::InvalidTag {
                    kind: "logical path root",
                    tag,
                    offset,
                });
            }
        };
        LogicalPath::new(root, &self.string()?).map_err(PlanCodecError::InvalidLogicalPath)
    }

    fn target(&mut self) -> Result<TargetSpec, PlanCodecError> {
        Ok(TargetSpec {
            arch: self.string()?,
            vendor: self.string()?,
            os: self.string()?,
            env: self.string()?,
            abi: self.string()?,
            endian: self.string()?,
            pointer_width: self.u32()?,
        })
    }

    fn optimization(&mut self) -> Result<OptimizationMode, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        match tag {
            0 => Ok(OptimizationMode::O0),
            1 => Ok(OptimizationMode::O1),
            2 => Ok(OptimizationMode::O2),
            3 => Ok(OptimizationMode::O3),
            4 => Ok(OptimizationMode::Os),
            5 => Ok(OptimizationMode::Oz),
            _ => Err(PlanCodecError::InvalidTag {
                kind: "optimization",
                tag,
                offset,
            }),
        }
    }

    fn runtime(&mut self) -> Result<Runtime, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        match tag {
            0 => Ok(Runtime::Bare),
            1 => Ok(Runtime::Freestanding),
            _ => Err(PlanCodecError::InvalidTag {
                kind: "runtime",
                tag,
                offset,
            }),
        }
    }

    fn module(&mut self) -> Result<PlanModule, PlanCodecError> {
        Ok(PlanModule {
            key: self.module_key()?,
            root_source: self.logical_path()?,
            optimization: self.optimization()?,
            imports: self.list(|reader| {
                Ok(ModuleImport {
                    name: reader.string()?,
                    path: reader.logical_path()?,
                })
            })?,
        })
    }

    fn artifact(&mut self) -> Result<PlanArtifact, PlanCodecError> {
        let key = self.artifact_key()?;
        let root_module = self.module_key()?;
        let kind_offset = self.offset;
        let kind = match self.u8()? {
            0 => PlanArtifactKind::Executable,
            1 => PlanArtifactKind::ObjectSet,
            2 => PlanArtifactKind::StaticArchive,
            tag => {
                return Err(PlanCodecError::InvalidTag {
                    kind: "artifact kind",
                    tag,
                    offset: kind_offset,
                });
            }
        };
        Ok(PlanArtifact {
            key,
            root_module,
            kind,
            output: self.logical_path()?,
            runtime: self.runtime()?,
        })
    }

    fn action(&mut self) -> Result<PlanAction, PlanCodecError> {
        let key = self.action_key()?;
        let offset = self.offset;
        let tag = self.u8()?;
        let kind = match tag {
            0 => ActionKind::CompilerCheck {
                module: self.module_key()?,
                target: self.target()?,
                runtime: self.runtime()?,
            },
            1 => ActionKind::CompilerEmit {
                artifact: self.artifact_key()?,
                target: self.target()?,
                static_archives: self.list(|reader| reader.artifact_key())?,
            },
            2 | 7 => {
                let resource_class = self.resource_class()?;
                let environment_policy = self.command_environment_policy()?;
                let cache_policy = self.command_cache_policy()?;
                let program_offset = self.offset;
                let program_tag = self.u8()?;
                let program = match program_tag {
                    0 => CommandProgram::Path(self.logical_path()?),
                    1 => CommandProgram::Search(self.string()?),
                    _ => {
                        return Err(PlanCodecError::InvalidTag {
                            kind: "command program",
                            tag: program_tag,
                            offset: program_offset,
                        });
                    }
                };
                let arguments = self.list(|reader| {
                    let offset = reader.offset;
                    let tag = reader.u8()?;
                    match tag {
                        0 => Ok(CommandArgument::Literal(reader.string()?)),
                        1 => Ok(CommandArgument::InputPath(reader.logical_path()?)),
                        2 => Ok(CommandArgument::OutputPath(reader.logical_path()?)),
                        _ => Err(PlanCodecError::InvalidTag {
                            kind: "command argument",
                            tag,
                            offset,
                        }),
                    }
                })?;
                let working_directory = self.logical_path()?;
                let environment = self.list(|reader| {
                    Ok(EnvironmentInput {
                        name: reader.string()?,
                        value: reader.option_string()?,
                    })
                })?;
                let inputs = self.list(Reader::logical_path)?;
                let outputs = self.list(Reader::logical_path)?;
                if tag == 7 {
                    ActionKind::TestExecutable {
                        resource_class,
                        environment_policy,
                        cache_policy,
                        program,
                        arguments,
                        working_directory,
                        environment,
                        inputs,
                        outputs,
                    }
                } else {
                    ActionKind::ExternalCommand {
                        resource_class,
                        environment_policy,
                        cache_policy,
                        program,
                        arguments,
                        working_directory,
                        environment,
                        inputs,
                        outputs,
                    }
                }
            }
            3 => {
                let output = self.logical_path()?;
                ActionKind::GeneratedFile {
                    output,
                    contents: self.blob()?,
                }
            }
            4 => ActionKind::Aggregate,
            5 => ActionKind::Uncacheable {
                description: self.string()?,
            },
            6 => ActionKind::InstallArtifact {
                artifact: self.artifact_key()?,
                destination: self.logical_path()?,
            },
            _ => {
                return Err(PlanCodecError::InvalidTag {
                    kind: "action",
                    tag,
                    offset,
                });
            }
        };
        Ok(PlanAction { key, kind })
    }

    fn step(&mut self) -> Result<PlanStep, PlanCodecError> {
        Ok(PlanStep {
            key: self.step_key()?,
            action: self.action_key()?,
            dependencies: self.list(Reader::step_key)?,
        })
    }

    fn option_string(&mut self) -> Result<Option<String>, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        match tag {
            0 => Ok(None),
            1 => Ok(Some(self.string()?)),
            _ => Err(PlanCodecError::InvalidTag {
                kind: "optional string",
                tag,
                offset,
            }),
        }
    }

    fn resource_class(&mut self) -> Result<ActionResourceClass, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        match tag {
            0 => Ok(ActionResourceClass::Conservative),
            1 => Ok(ActionResourceClass::Cpu),
            2 => Ok(ActionResourceClass::Io),
            _ => Err(PlanCodecError::InvalidTag {
                kind: "action resource class",
                tag,
                offset,
            }),
        }
    }

    fn command_environment_policy(&mut self) -> Result<CommandEnvironmentPolicy, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        match tag {
            0 => Ok(CommandEnvironmentPolicy::Inherit),
            1 => Ok(CommandEnvironmentPolicy::Clear),
            _ => Err(PlanCodecError::InvalidTag {
                kind: "command environment policy",
                tag,
                offset,
            }),
        }
    }

    fn command_cache_policy(&mut self) -> Result<CommandCachePolicy, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        match tag {
            0 => Ok(CommandCachePolicy::Uncacheable),
            1 => Ok(CommandCachePolicy::DeclaredInputs),
            _ => Err(PlanCodecError::InvalidTag {
                kind: "command cache policy",
                tag,
                offset,
            }),
        }
    }

    fn option_step_key(&mut self) -> Result<Option<StepKey>, PlanCodecError> {
        let offset = self.offset;
        let tag = self.u8()?;
        match tag {
            0 => Ok(None),
            1 => Ok(Some(self.step_key()?)),
            _ => Err(PlanCodecError::InvalidTag {
                kind: "optional step",
                tag,
                offset,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{draft, generated_source_draft, static_archive_link_draft};
    use super::*;

    fn encode_draft_without_freeze(draft: &BuildPlanDraft) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.bytes(BUILD_PLAN.magic);
        writer.u32(BUILD_PLAN.schema);
        writer.package_key(&draft.root_package).unwrap();
        writer.count(draft.packages.len()).unwrap();
        for package in &draft.packages {
            writer.package_key(&package.key).unwrap();
            writer.string(&package.root).unwrap();
        }
        writer.target(&draft.host_target).unwrap();
        writer.target(&draft.artifact_target).unwrap();
        writer.count(draft.modules.len()).unwrap();
        for module in &draft.modules {
            writer.module(module).unwrap();
        }
        writer.count(draft.artifacts.len()).unwrap();
        for artifact in &draft.artifacts {
            writer.artifact(artifact).unwrap();
        }
        writer.count(draft.actions.len()).unwrap();
        for action in &draft.actions {
            writer.action(action).unwrap();
        }
        writer.count(draft.steps.len()).unwrap();
        for step in &draft.steps {
            writer.step(step).unwrap();
        }
        writer.option_step_key(draft.default_step.as_ref()).unwrap();
        writer
            .option_step_key(draft.selected_step.as_ref())
            .unwrap();
        writer.finish().unwrap()
    }

    fn mutation_corpus() -> Vec<Vec<u8>> {
        let mut command = draft(false);
        command.actions.push(PlanAction {
            key: ActionKey::new(PackageKey::root(), "command-corpus").unwrap(),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy: CommandEnvironmentPolicy::Clear,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: vec![CommandArgument::Literal("--version".to_string())],
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: vec![EnvironmentInput {
                    name: "LANG".to_string(),
                    value: Some("C".to_string()),
                }],
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        });
        [
            draft(false),
            static_archive_link_draft(),
            generated_source_draft("generated/root.nia", vec!["generate"]),
            command,
        ]
        .into_iter()
        .map(|draft| BuildPlan::freeze(draft).unwrap().encode().unwrap())
        .collect()
    }

    fn assert_accepted_encoding_recanonicalizes(bytes: &[u8]) {
        let Ok(plan) = BuildPlan::decode(bytes) else {
            return;
        };
        let canonical = plan.encode().unwrap();
        assert_eq!(BuildPlan::decode(&canonical), Ok(plan));
    }

    #[test]
    fn canonical_compiler_plan_round_trips() {
        let plan = BuildPlan::freeze(draft(false)).unwrap();
        let bytes = plan.encode().unwrap();
        assert_eq!(BuildPlan::decode(&bytes).unwrap(), plan);
    }

    #[test]
    fn typed_static_archive_links_round_trip_in_declaration_order() {
        let plan = BuildPlan::freeze(static_archive_link_draft()).unwrap();
        let decoded = BuildPlan::decode(&plan.encode().unwrap()).unwrap();
        let emit = decoded
            .actions()
            .iter()
            .find(|action| action.key.name() == "emit")
            .unwrap();
        assert!(matches!(
            &emit.kind,
            ActionKind::CompilerEmit { static_archives, .. }
                if static_archives.iter().map(ArtifactKey::name).collect::<Vec<_>>()
                    == ["runtime", "support"]
        ));
    }

    #[test]
    fn declared_package_roots_round_trip_canonically() {
        let mut value = draft(false);
        value.packages.push(PlanPackage {
            key: PackageKey::new("assets").unwrap(),
            root: "packages/assets".to_string(),
        });
        let plan = BuildPlan::freeze(value).unwrap();
        let bytes = plan.encode().unwrap();
        let decoded = BuildPlan::decode(&bytes).unwrap();
        assert_eq!(decoded, plan);
        assert_eq!(decoded.packages()[0].root, "packages/assets");
    }

    #[test]
    fn typed_action_variants_round_trip() {
        let mut value = draft(false);
        value.artifacts.push(PlanArtifact {
            key: ArtifactKey::new(PackageKey::root(), "objects").unwrap(),
            root_module: ModuleKey::new(PackageKey::root(), "a").unwrap(),
            kind: PlanArtifactKind::ObjectSet,
            output: LogicalPath::new(LogicalPathRoot::Build, "objects/app").unwrap(),
            runtime: Runtime::Bare,
        });
        value.artifacts.push(PlanArtifact {
            key: ArtifactKey::new(PackageKey::root(), "archive").unwrap(),
            root_module: ModuleKey::new(PackageKey::root(), "a").unwrap(),
            kind: PlanArtifactKind::StaticArchive,
            output: LogicalPath::new(LogicalPathRoot::Build, "lib/libarchive.a").unwrap(),
            runtime: Runtime::Bare,
        });
        let package = PackageKey::root();
        value.actions.extend([
            PlanAction {
                key: ActionKey::new(package.clone(), "command").unwrap(),
                kind: ActionKind::ExternalCommand {
                    resource_class: ActionResourceClass::Cpu,
                    environment_policy: CommandEnvironmentPolicy::Clear,
                    cache_policy: CommandCachePolicy::DeclaredInputs,
                    program: CommandProgram::Search("cc".into()),
                    arguments: vec![
                        CommandArgument::Literal("-v".into()),
                        CommandArgument::InputPath(
                            LogicalPath::new(
                                LogicalPathRoot::Package(package.clone()),
                                "src/a.nia",
                            )
                            .unwrap(),
                        ),
                        CommandArgument::OutputPath(
                            LogicalPath::new(LogicalPathRoot::Build, "command.out").unwrap(),
                        ),
                    ],
                    working_directory: LogicalPath::new(
                        LogicalPathRoot::Package(package.clone()),
                        "",
                    )
                    .unwrap(),
                    environment: vec![EnvironmentInput {
                        name: "LANG".into(),
                        value: Some("C".into()),
                    }],
                    inputs: vec![
                        LogicalPath::new(LogicalPathRoot::Package(package.clone()), "src/a.nia")
                            .unwrap(),
                    ],
                    outputs: vec![LogicalPath::new(LogicalPathRoot::Build, "command.out").unwrap()],
                },
            },
            PlanAction {
                key: ActionKey::new(package.clone(), "generate").unwrap(),
                kind: ActionKind::GeneratedFile {
                    output: LogicalPath::new(LogicalPathRoot::Build, "generated.nia").unwrap(),
                    contents: b"pub fn generated() () {}\n".to_vec(),
                },
            },
            PlanAction {
                key: ActionKey::new(package.clone(), "install").unwrap(),
                kind: ActionKind::InstallArtifact {
                    artifact: ArtifactKey::new(package.clone(), "app").unwrap(),
                    destination: LogicalPath::new(LogicalPathRoot::Build, "install/app").unwrap(),
                },
            },
            PlanAction {
                key: ActionKey::new(package.clone(), "aggregate").unwrap(),
                kind: ActionKind::Aggregate,
            },
            PlanAction {
                key: ActionKey::new(package, "opaque").unwrap(),
                kind: ActionKind::Uncacheable {
                    description: "reads untracked host state".into(),
                },
            },
        ]);
        let plan = BuildPlan::freeze(value).unwrap();
        let bytes = plan.encode().unwrap();
        let decoded = BuildPlan::decode(&bytes).unwrap();
        assert_eq!(decoded, plan);
        assert!(
            decoded
                .artifacts()
                .iter()
                .any(|artifact| artifact.kind == PlanArtifactKind::ObjectSet)
        );
        assert!(
            decoded
                .artifacts()
                .iter()
                .any(|artifact| artifact.kind == PlanArtifactKind::StaticArchive)
        );
    }

    #[test]
    fn test_executable_action_has_distinct_protocol_identity() {
        let mut value = draft(false);
        let package = PackageKey::root();
        let action = ActionKey::new(package.clone(), "tests").unwrap();
        let step = StepKey::new(package.clone(), "tests").unwrap();
        value.actions.push(PlanAction {
            key: action.clone(),
            kind: ActionKind::TestExecutable {
                resource_class: ActionResourceClass::Conservative,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("test-runner".into()),
                arguments: Vec::new(),
                working_directory: LogicalPath::new(LogicalPathRoot::Package(package.clone()), "")
                    .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        });
        value.steps.push(PlanStep {
            key: step.clone(),
            action,
            dependencies: Vec::new(),
        });
        value.default_step = Some(step);
        let plan = BuildPlan::freeze(value).unwrap();
        let decoded = BuildPlan::decode(&plan.encode().unwrap()).unwrap();
        assert!(decoded.actions().iter().any(|action| action.kind.is_test()));
    }

    #[test]
    fn resource_class_codec_rejects_unknown_tags() {
        let mut reader = Reader::new(&[0, 1, 2, 3]);
        assert_eq!(
            reader.resource_class().unwrap(),
            ActionResourceClass::Conservative
        );
        assert_eq!(reader.resource_class().unwrap(), ActionResourceClass::Cpu);
        assert_eq!(reader.resource_class().unwrap(), ActionResourceClass::Io);
        assert_eq!(
            reader.resource_class(),
            Err(PlanCodecError::InvalidTag {
                kind: "action resource class",
                tag: 3,
                offset: 3,
            })
        );
    }

    #[test]
    fn command_policy_codecs_reject_unknown_tags() {
        let mut environment = Reader::new(&[0, 1, 2]);
        assert_eq!(
            environment.command_environment_policy().unwrap(),
            CommandEnvironmentPolicy::Inherit
        );
        assert_eq!(
            environment.command_environment_policy().unwrap(),
            CommandEnvironmentPolicy::Clear
        );
        assert!(matches!(
            environment.command_environment_policy(),
            Err(PlanCodecError::InvalidTag {
                kind: "command environment policy",
                tag: 2,
                ..
            })
        ));

        let mut cache = Reader::new(&[0, 1, 2]);
        assert_eq!(
            cache.command_cache_policy().unwrap(),
            CommandCachePolicy::Uncacheable
        );
        assert_eq!(
            cache.command_cache_policy().unwrap(),
            CommandCachePolicy::DeclaredInputs
        );
        assert!(matches!(
            cache.command_cache_policy(),
            Err(PlanCodecError::InvalidTag {
                kind: "command cache policy",
                tag: 2,
                ..
            })
        ));
    }

    #[test]
    fn generated_content_uses_blob_not_node_count_limit() {
        let mut value = draft(false);
        value.actions.push(PlanAction {
            key: ActionKey::new(PackageKey::root(), "large-generated").unwrap(),
            kind: ActionKind::GeneratedFile {
                output: LogicalPath::new(LogicalPathRoot::Build, "large.bin").unwrap(),
                contents: vec![7; MAX_ITEMS + 1],
            },
        });
        let plan = BuildPlan::freeze(value).unwrap();
        assert_eq!(BuildPlan::decode(&plan.encode().unwrap()).unwrap(), plan);
    }

    #[test]
    fn writer_stops_before_exceeding_the_total_plan_budget() {
        let mut writer = Writer::with_limit(8);
        writer.bytes(&[1, 2, 3, 4, 5, 6]);
        writer.u32(7);
        writer.u8(8);

        assert_eq!(writer.bytes, [1, 2, 3, 4, 5, 6]);
        assert_eq!(
            writer.finish(),
            Err(PlanCodecError::TooLarge {
                limit: 8,
                actual: 10,
            })
        );
    }

    #[test]
    fn truncated_list_count_cannot_preallocate_element_capacity() {
        type LargeItem = [u8; (isize::MAX as usize / MAX_ITEMS) + 1];

        let encoded = (MAX_ITEMS as u32).to_le_bytes();
        let mut reader = Reader::new(&encoded);
        let result = reader.list::<LargeItem>(|reader| {
            reader.take(1)?;
            unreachable!()
        });

        assert_eq!(result, Err(PlanCodecError::Truncated { offset: 4 }));
    }

    #[test]
    fn allocation_order_produces_identical_bytes() {
        let first = BuildPlan::freeze(draft(false)).unwrap().encode().unwrap();
        let second = BuildPlan::freeze(draft(true)).unwrap().encode().unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn rejects_unknown_version_truncation_and_trailing_data() {
        let plan = BuildPlan::freeze(draft(false)).unwrap();
        let bytes = plan.encode().unwrap();
        let mut unknown = bytes.clone();
        unknown[8..12].copy_from_slice(&(BUILD_PLAN.schema + 1).to_le_bytes());
        assert_eq!(
            BuildPlan::decode(&unknown),
            Err(PlanCodecError::UnsupportedVersion(BUILD_PLAN.schema + 1))
        );
        assert!(matches!(
            BuildPlan::decode(&bytes[..bytes.len() - 1]),
            Err(PlanCodecError::Truncated { .. })
        ));
        let mut trailing = bytes;
        trailing.push(0);
        assert!(matches!(
            BuildPlan::decode(&trailing),
            Err(PlanCodecError::TrailingData { .. })
        ));
    }

    #[test]
    fn every_truncated_corpus_prefix_is_rejected() {
        for (corpus_index, bytes) in mutation_corpus().into_iter().enumerate() {
            for prefix_len in 0..bytes.len() {
                assert!(
                    BuildPlan::decode(&bytes[..prefix_len]).is_err(),
                    "corpus {corpus_index} accepted prefix {prefix_len}/{}",
                    bytes.len()
                );
            }
        }
    }

    #[test]
    fn deterministic_codec_mutations_never_escape_validation() {
        for (corpus_index, bytes) in mutation_corpus().into_iter().enumerate() {
            for offset in 0..bytes.len() {
                for bit in 0..8 {
                    let mut mutated = bytes.clone();
                    mutated[offset] ^= 1 << bit;
                    assert_accepted_encoding_recanonicalizes(&mutated);
                }
            }
            for offset in 0..bytes.len().saturating_sub(3) {
                let mut oversized = bytes.clone();
                oversized[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
                assert_accepted_encoding_recanonicalizes(&oversized);
            }
            assert_eq!(
                BuildPlan::decode(&bytes),
                BuildPlan::decode(&BuildPlan::decode(&bytes).unwrap().encode().unwrap()),
                "corpus {corpus_index} changed after canonical round trip"
            );
        }
    }

    #[test]
    fn rejects_semantically_invalid_payload_after_decoding() {
        let plan = BuildPlan::freeze(draft(false)).unwrap();
        let mut bytes = plan.encode().unwrap();
        // The root package is the first length-prefixed value after magic/version.
        bytes[16..20].copy_from_slice(b"nope");
        assert!(matches!(
            BuildPlan::decode(&bytes),
            Err(PlanCodecError::Semantic(error))
                if matches!(*error, PlanError::MissingPackage(_))
        ));
    }

    #[test]
    fn decoded_draft_cannot_bypass_generated_source_producer_closure() {
        let draft = generated_source_draft("generated/root.nia", vec![]);
        let bytes = encode_draft_without_freeze(&draft);
        assert!(matches!(
            BuildPlan::decode(&bytes),
            Err(PlanCodecError::Semantic(error))
                if matches!(*error, PlanError::GeneratedSourceProducerOutsideClosure { .. })
        ));
    }

    #[test]
    fn decoded_draft_cannot_introduce_a_third_compiler_target() {
        let mut draft = draft(false);
        let emit = draft
            .actions
            .iter_mut()
            .find(|action| action.key.name() == "emit")
            .unwrap();
        let ActionKind::CompilerEmit { target, .. } = &mut emit.kind else {
            unreachable!()
        };
        target.arch = "third-architecture".to_string();

        let bytes = encode_draft_without_freeze(&draft);
        assert!(matches!(
            BuildPlan::decode(&bytes),
            Err(PlanCodecError::Semantic(error))
                if matches!(*error, PlanError::InvalidActionTarget(_))
        ));
    }
}
