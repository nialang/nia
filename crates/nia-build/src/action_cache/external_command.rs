// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use nia_compat::formats::{EXTERNAL_COMMAND_CACHE, EXTERNAL_COMMAND_ENTRY};
use nia_query::{FingerprintDomain, QueryFingerprint, QueryFingerprintBuilder};
use nia_toolchain::ToolchainIdentity;

use super::{
    ActionCacheInvalidation, ActionCacheMissReason, CACHE_STAGE_SEQUENCE, action_identity,
    fingerprint_text, integer_component, logical_path_identity, package_roots_identity, read_bytes,
    read_fingerprint, read_u64, text_component, write_bytes, write_fingerprint,
};
use crate::{
    ActionKey, CommandArgument, CommandProgram, EnvironmentInput, LogicalPath, LogicalPathRoot,
    PlanPackage, lock::ScopedFileLock,
};

const EXTERNAL_COMMAND_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command.v1");
const EXTERNAL_COMMAND_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-key.v1");
const EXTERNAL_COMMAND_DECLARATION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-declaration.v1");
const EXTERNAL_COMMAND_TOOL_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-tool.v1");
const EXTERNAL_COMMAND_ENVIRONMENT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-environment.v1");
const EXTERNAL_COMMAND_INPUTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-inputs.v1");
const EXTERNAL_COMMAND_DEPENDENCIES_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-dependencies.v1");
const EXTERNAL_COMMAND_WORKING_DIRECTORY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-working-directory.v1");
const EXTERNAL_COMMAND_PACKAGE_ROOTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-package-roots.v1");
const EXTERNAL_COMMAND_OUTPUTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-outputs.v1");
const EXTERNAL_COMMAND_COMPILER_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-compiler.v1");
const EXTERNAL_COMMAND_RESOURCE_LAYOUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-resource-layout.v1");
const EXTERNAL_COMMAND_STANDARD_LIBRARY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-standard-library.v1");
const EXTERNAL_COMMAND_BUILD_PROTOCOL_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-build-protocol.v1");
const EXTERNAL_COMMAND_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-payload.v1");
const EXTERNAL_COMMAND_TOOL_CONTENTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-tool-contents.v1");
const EXTERNAL_COMMAND_INPUT_CONTENTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.external-command-input-contents.v2");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FingerprintComponents {
    command: QueryFingerprint,
    tool: QueryFingerprint,
    environment: QueryFingerprint,
    inputs: QueryFingerprint,
    dependencies: QueryFingerprint,
    working_directory: QueryFingerprint,
    package_roots: QueryFingerprint,
    outputs: QueryFingerprint,
    compiler: QueryFingerprint,
    resource_layout: QueryFingerprint,
    standard_library: QueryFingerprint,
    build_protocol: QueryFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FingerprintSet {
    cache_key: QueryFingerprint,
    fingerprint: QueryFingerprint,
    components: FingerprintComponents,
}

impl FingerprintSet {
    fn new(cache_key: QueryFingerprint, components: FingerprintComponents) -> Self {
        let mut builder = QueryFingerprintBuilder::new(EXTERNAL_COMMAND_FINGERPRINT_DOMAIN);
        builder.write_fingerprint(cache_key);
        for component in components.values() {
            builder.write_fingerprint(component);
        }
        Self {
            cache_key,
            fingerprint: builder.finish(),
            components,
        }
    }
}

impl FingerprintComponents {
    fn values(self) -> [QueryFingerprint; 12] {
        [
            self.command,
            self.tool,
            self.environment,
            self.inputs,
            self.dependencies,
            self.working_directory,
            self.package_roots,
            self.outputs,
            self.compiler,
            self.resource_layout,
            self.standard_library,
            self.build_protocol,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalCommandCacheIdentity {
    fingerprints: FingerprintSet,
    action: Vec<u8>,
    command: Vec<u8>,
    tool: Vec<u8>,
    environment: Vec<u8>,
    inputs: Vec<u8>,
    dependencies: Vec<u8>,
    working_directory: Vec<u8>,
    package_roots: Vec<u8>,
    outputs: Vec<u8>,
    output_count: usize,
}

impl ExternalCommandCacheIdentity {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        action: &ActionKey,
        program: &CommandProgram,
        arguments: &[CommandArgument],
        working_directory: &LogicalPath,
        environment: &[EnvironmentInput],
        inputs: &[(LogicalPath, Vec<u8>)],
        outputs: &[LogicalPath],
        packages: &[PlanPackage],
        tool_contents: &[u8],
        toolchain: &ToolchainIdentity,
    ) -> Option<Self> {
        let action_identity = action_identity(action);
        let command = command_identity(program, arguments);
        let tool = tool_identity(program, tool_contents);
        let environment = environment_identity(environment);
        let regular_inputs = inputs
            .iter()
            .filter(|(path, _)| !matches!(path.root(), LogicalPathRoot::Artifact(_)))
            .map(|(path, contents)| (path, contents.as_slice()))
            .collect::<Vec<_>>();
        let dependency_inputs = inputs
            .iter()
            .filter(|(path, _)| matches!(path.root(), LogicalPathRoot::Artifact(_)))
            .map(|(path, contents)| (path, contents.as_slice()))
            .collect::<Vec<_>>();
        let mut package_paths = Vec::new();
        if let CommandProgram::Path(path) = program {
            package_paths.push(path);
        }
        package_paths.push(working_directory);
        for argument in arguments {
            if let CommandArgument::InputPath(path) | CommandArgument::OutputPath(path) = argument {
                package_paths.push(path);
            }
        }
        package_paths.extend(inputs.iter().map(|(path, _)| path));
        package_paths.extend(outputs.iter());
        let package_roots = package_roots_identity(packages, package_paths)?;
        let inputs = input_identity(&regular_inputs);
        let dependencies = input_identity(&dependency_inputs);
        let working_directory = logical_path_identity(working_directory);
        let outputs_identity = path_list_identity(outputs);
        let cache_key = component(EXTERNAL_COMMAND_KEY_DOMAIN, &action_identity);
        let components = FingerprintComponents {
            command: component(EXTERNAL_COMMAND_DECLARATION_DOMAIN, &command),
            tool: component(EXTERNAL_COMMAND_TOOL_DOMAIN, &tool),
            environment: component(EXTERNAL_COMMAND_ENVIRONMENT_DOMAIN, &environment),
            inputs: component(EXTERNAL_COMMAND_INPUTS_DOMAIN, &inputs),
            dependencies: component(EXTERNAL_COMMAND_DEPENDENCIES_DOMAIN, &dependencies),
            working_directory: component(
                EXTERNAL_COMMAND_WORKING_DIRECTORY_DOMAIN,
                &working_directory,
            ),
            package_roots: component(EXTERNAL_COMMAND_PACKAGE_ROOTS_DOMAIN, &package_roots),
            outputs: component(EXTERNAL_COMMAND_OUTPUTS_DOMAIN, &outputs_identity),
            compiler: text_component(
                EXTERNAL_COMMAND_COMPILER_DOMAIN,
                toolchain.compiler_version(),
            ),
            resource_layout: integer_component(
                EXTERNAL_COMMAND_RESOURCE_LAYOUT_DOMAIN,
                toolchain.resource_layout_schema(),
            ),
            standard_library: integer_component(
                EXTERNAL_COMMAND_STANDARD_LIBRARY_DOMAIN,
                toolchain.std_schema(),
            ),
            build_protocol: integer_component(
                EXTERNAL_COMMAND_BUILD_PROTOCOL_DOMAIN,
                toolchain.build_protocol_schema(),
            ),
        };
        Some(Self {
            fingerprints: FingerprintSet::new(cache_key, components),
            action: action_identity,
            command,
            tool,
            environment,
            inputs,
            dependencies,
            working_directory,
            package_roots,
            outputs: outputs_identity,
            output_count: outputs.len(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExternalCommandCacheLookup {
    Hit(Vec<Vec<u8>>),
    Miss(ActionCacheMissReason),
}

#[derive(Debug)]
pub(crate) struct ExternalCommandCache {
    root: PathBuf,
}

impl ExternalCommandCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn lookup(
        &self,
        identity: &ExternalCommandCacheIdentity,
    ) -> io::Result<ExternalCommandCacheLookup> {
        let path = self.path(identity.fingerprints);
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(identity);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = decode_entry(&encoded) else {
            self.retire_corrupt(&path, &encoded)?;
            return Ok(ExternalCommandCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        };
        if !entry_matches(&entry, identity) || path != self.path(entry.fingerprints) {
            self.retire_corrupt(&path, &encoded)?;
            return Ok(ExternalCommandCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        }
        Ok(ExternalCommandCacheLookup::Hit(entry.payloads))
    }

    pub(crate) fn publish(
        &self,
        identity: &ExternalCommandCacheIdentity,
        payloads: &[Vec<u8>],
    ) -> io::Result<()> {
        if payloads.len() != identity.output_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "external-command cache payload count does not match outputs",
            ));
        }
        if let ExternalCommandCacheLookup::Hit(found) = self.lookup(identity)? {
            if found == payloads {
                return Ok(());
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "external command produced different outputs for one cache identity",
            ));
        }
        let path = self.path(identity.fingerprints);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid external-command cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(
            ".nia-command-cache-{}-{}.tmp",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let encoded = encode_entry(identity, payloads);
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            drop(file);
            self.install_immutable_entry(&staged, &path, identity, payloads)?;
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result
    }

    fn lookup_invalidation(
        &self,
        expected: &ExternalCommandCacheIdentity,
    ) -> io::Result<ExternalCommandCacheLookup> {
        let entries = match fs::read_dir(self.key_dir(expected.fingerprints.cache_key)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ExternalCommandCacheLookup::Miss(
                    ActionCacheMissReason::NotFound,
                ));
            }
            Err(error) => return Err(error),
        };
        let mut nearest = None::<(usize, QueryFingerprint, Vec<ActionCacheInvalidation>)>;
        let mut corrupt = false;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("entry") {
                continue;
            }
            let encoded = fs::read(&path)?;
            let Some(entry) = decode_entry(&encoded) else {
                self.retire_corrupt(&path, &encoded)?;
                corrupt = true;
                continue;
            };
            if entry.fingerprints.cache_key != expected.fingerprints.cache_key
                || path != self.path(entry.fingerprints)
            {
                self.retire_corrupt(&path, &encoded)?;
                corrupt = true;
                continue;
            }
            let reasons = invalidations(
                entry.fingerprints.components,
                expected.fingerprints.components,
            );
            let candidate = (reasons.len(), entry.fingerprints.fingerprint, reasons);
            if nearest
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                nearest = Some(candidate);
            }
        }
        if let Some((_, _, reasons)) = nearest {
            Ok(ExternalCommandCacheLookup::Miss(
                ActionCacheMissReason::Invalidated(reasons),
            ))
        } else if corrupt {
            Ok(ExternalCommandCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ))
        } else {
            Ok(ExternalCommandCacheLookup::Miss(
                ActionCacheMissReason::NotFound,
            ))
        }
    }

    fn key_dir(&self, cache_key: QueryFingerprint) -> PathBuf {
        self.root
            .join("actions/external-commands")
            .join(EXTERNAL_COMMAND_CACHE.path_component)
            .join(fingerprint_text(cache_key))
    }

    fn path(&self, fingerprints: FingerprintSet) -> PathBuf {
        self.key_dir(fingerprints.cache_key).join(format!(
            "{}.entry",
            fingerprint_text(fingerprints.fingerprint)
        ))
    }

    fn install_immutable_entry(
        &self,
        staged: &Path,
        path: &Path,
        identity: &ExternalCommandCacheIdentity,
        payloads: &[Vec<u8>],
    ) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        for _ in 0..4 {
            match fs::hard_link(staged, path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let encoded = fs::read(path)?;
                    if let Some(entry) = decode_entry(&encoded)
                        && entry_matches(&entry, identity)
                    {
                        if entry.payloads == payloads {
                            return Ok(());
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "concurrent external command produced different outputs for one cache identity",
                        ));
                    }
                    match fs::remove_file(path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "external-command cache entry changed during publication",
        ))
    }

    fn retire_corrupt(&self, path: &Path, observed: &[u8]) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        match fs::read(path) {
            Ok(current) if current == observed => match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn acquire_mutation_lock(&self, path: &Path) -> io::Result<ScopedFileLock> {
        let mut builder = QueryFingerprintBuilder::new(super::ACTION_CACHE_MUTATION_LOCK_DOMAIN);
        builder.write_bytes(path.as_os_str().as_encoded_bytes());
        let lock = self
            .root
            .join("coordination/action-cache-mutations")
            .join(EXTERNAL_COMMAND_CACHE.path_component)
            .join(format!("{}.lock", fingerprint_text(builder.finish())));
        ScopedFileLock::acquire_interruptible(lock, || false)?
            .ok_or_else(|| io::Error::other("action-cache mutation lock was cancelled"))
    }
}

struct DecodedEntry {
    fingerprints: FingerprintSet,
    action: Vec<u8>,
    command: Vec<u8>,
    tool: Vec<u8>,
    environment: Vec<u8>,
    inputs: Vec<u8>,
    dependencies: Vec<u8>,
    working_directory: Vec<u8>,
    package_roots: Vec<u8>,
    outputs: Vec<u8>,
    payloads: Vec<Vec<u8>>,
}

fn encode_entry(identity: &ExternalCommandCacheIdentity, payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(EXTERNAL_COMMAND_ENTRY.magic);
    write_fingerprint(&mut encoded, identity.fingerprints.cache_key);
    write_fingerprint(&mut encoded, identity.fingerprints.fingerprint);
    for component in identity.fingerprints.components.values() {
        write_fingerprint(&mut encoded, component);
    }
    for value in [
        &identity.action,
        &identity.command,
        &identity.tool,
        &identity.environment,
        &identity.inputs,
        &identity.dependencies,
        &identity.working_directory,
        &identity.package_roots,
        &identity.outputs,
    ] {
        write_bytes(&mut encoded, value);
    }
    encoded.extend_from_slice(&(payloads.len() as u64).to_le_bytes());
    for payload in payloads {
        write_fingerprint(
            &mut encoded,
            bytes_fingerprint(EXTERNAL_COMMAND_PAYLOAD_DOMAIN, payload),
        );
        write_bytes(&mut encoded, payload);
    }
    encoded
}

fn decode_entry(encoded: &[u8]) -> Option<DecodedEntry> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *EXTERNAL_COMMAND_ENTRY.magic).then_some(())?;
    let cache_key = read_fingerprint(&mut cursor)?;
    let fingerprint = read_fingerprint(&mut cursor)?;
    let components = FingerprintComponents {
        command: read_fingerprint(&mut cursor)?,
        tool: read_fingerprint(&mut cursor)?,
        environment: read_fingerprint(&mut cursor)?,
        inputs: read_fingerprint(&mut cursor)?,
        dependencies: read_fingerprint(&mut cursor)?,
        working_directory: read_fingerprint(&mut cursor)?,
        package_roots: read_fingerprint(&mut cursor)?,
        outputs: read_fingerprint(&mut cursor)?,
        compiler: read_fingerprint(&mut cursor)?,
        resource_layout: read_fingerprint(&mut cursor)?,
        standard_library: read_fingerprint(&mut cursor)?,
        build_protocol: read_fingerprint(&mut cursor)?,
    };
    let fingerprints = FingerprintSet::new(cache_key, components);
    (fingerprints.fingerprint == fingerprint).then_some(())?;
    let action = read_bytes(&mut cursor, encoded.len())?;
    let command = read_bytes(&mut cursor, encoded.len())?;
    let tool = read_bytes(&mut cursor, encoded.len())?;
    let environment = read_bytes(&mut cursor, encoded.len())?;
    let inputs = read_bytes(&mut cursor, encoded.len())?;
    let dependencies = read_bytes(&mut cursor, encoded.len())?;
    let working_directory = read_bytes(&mut cursor, encoded.len())?;
    let package_roots = read_bytes(&mut cursor, encoded.len())?;
    let outputs = read_bytes(&mut cursor, encoded.len())?;
    (component(EXTERNAL_COMMAND_KEY_DOMAIN, &action) == cache_key).then_some(())?;
    for (found, domain, value) in [
        (
            components.command,
            EXTERNAL_COMMAND_DECLARATION_DOMAIN,
            &command,
        ),
        (components.tool, EXTERNAL_COMMAND_TOOL_DOMAIN, &tool),
        (
            components.environment,
            EXTERNAL_COMMAND_ENVIRONMENT_DOMAIN,
            &environment,
        ),
        (components.inputs, EXTERNAL_COMMAND_INPUTS_DOMAIN, &inputs),
        (
            components.dependencies,
            EXTERNAL_COMMAND_DEPENDENCIES_DOMAIN,
            &dependencies,
        ),
        (
            components.working_directory,
            EXTERNAL_COMMAND_WORKING_DIRECTORY_DOMAIN,
            &working_directory,
        ),
        (
            components.package_roots,
            EXTERNAL_COMMAND_PACKAGE_ROOTS_DOMAIN,
            &package_roots,
        ),
        (
            components.outputs,
            EXTERNAL_COMMAND_OUTPUTS_DOMAIN,
            &outputs,
        ),
    ] {
        (component(domain, value) == found).then_some(())?;
    }
    let payload_count = usize::try_from(read_u64(&mut cursor)?).ok()?;
    (payload_count <= encoded.len() / 24).then_some(())?;
    (identity_count(&outputs)? == payload_count).then_some(())?;
    let mut payloads = Vec::with_capacity(payload_count);
    for _ in 0..payload_count {
        let checksum = read_fingerprint(&mut cursor)?;
        let payload = read_bytes(&mut cursor, encoded.len())?;
        (bytes_fingerprint(EXTERNAL_COMMAND_PAYLOAD_DOMAIN, &payload) == checksum).then_some(())?;
        payloads.push(payload);
    }
    (usize::try_from(cursor.position()).ok()? == encoded.len()).then_some(DecodedEntry {
        fingerprints,
        action,
        command,
        tool,
        environment,
        inputs,
        dependencies,
        working_directory,
        package_roots,
        outputs,
        payloads,
    })
}

fn entry_matches(entry: &DecodedEntry, identity: &ExternalCommandCacheIdentity) -> bool {
    entry.fingerprints == identity.fingerprints
        && entry.action == identity.action
        && entry.command == identity.command
        && entry.tool == identity.tool
        && entry.environment == identity.environment
        && entry.inputs == identity.inputs
        && entry.dependencies == identity.dependencies
        && entry.working_directory == identity.working_directory
        && entry.package_roots == identity.package_roots
        && entry.outputs == identity.outputs
        && entry.payloads.len() == identity.output_count
}

fn invalidations(
    found: FingerprintComponents,
    expected: FingerprintComponents,
) -> Vec<ActionCacheInvalidation> {
    let mut reasons = Vec::new();
    for (changed, reason) in [
        (
            found.command != expected.command,
            ActionCacheInvalidation::Command,
        ),
        (
            found.tool != expected.tool,
            ActionCacheInvalidation::ExternalTool,
        ),
        (
            found.environment != expected.environment,
            ActionCacheInvalidation::Environment,
        ),
        (
            found.inputs != expected.inputs,
            ActionCacheInvalidation::Inputs,
        ),
        (
            found.dependencies != expected.dependencies,
            ActionCacheInvalidation::Dependencies,
        ),
        (
            found.working_directory != expected.working_directory,
            ActionCacheInvalidation::WorkingDirectory,
        ),
        (
            found.package_roots != expected.package_roots,
            ActionCacheInvalidation::PackageRoots,
        ),
        (
            found.outputs != expected.outputs,
            ActionCacheInvalidation::Output,
        ),
        (
            found.compiler != expected.compiler,
            ActionCacheInvalidation::Compiler,
        ),
        (
            found.resource_layout != expected.resource_layout,
            ActionCacheInvalidation::ResourceLayout,
        ),
        (
            found.standard_library != expected.standard_library,
            ActionCacheInvalidation::StandardLibrary,
        ),
        (
            found.build_protocol != expected.build_protocol,
            ActionCacheInvalidation::BuildProtocol,
        ),
    ] {
        if changed {
            reasons.push(reason);
        }
    }
    reasons
}

fn component(domain: FingerprintDomain, value: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_bytes(value);
    builder.finish()
}

fn bytes_fingerprint(domain: FingerprintDomain, value: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_bytes(value);
    builder.finish()
}

fn command_identity(program: &CommandProgram, arguments: &[CommandArgument]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encode_program(&mut encoded, program);
    encoded.extend_from_slice(&(arguments.len() as u64).to_le_bytes());
    for argument in arguments {
        match argument {
            CommandArgument::Literal(value) => {
                encoded.push(0);
                write_bytes(&mut encoded, value.as_bytes());
            }
            CommandArgument::InputPath(path) => {
                encoded.push(1);
                write_bytes(&mut encoded, &logical_path_identity(path));
            }
            CommandArgument::OutputPath(path) => {
                encoded.push(2);
                write_bytes(&mut encoded, &logical_path_identity(path));
            }
        }
    }
    encoded
}

fn encode_program(encoded: &mut Vec<u8>, program: &CommandProgram) {
    match program {
        CommandProgram::Path(path) => {
            encoded.push(0);
            write_bytes(encoded, &logical_path_identity(path));
        }
        CommandProgram::Search(name) => {
            encoded.push(1);
            write_bytes(encoded, name.as_bytes());
        }
    }
}

fn tool_identity(program: &CommandProgram, contents: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encode_program(&mut encoded, program);
    encoded.extend_from_slice(&(contents.len() as u64).to_le_bytes());
    write_fingerprint(
        &mut encoded,
        bytes_fingerprint(EXTERNAL_COMMAND_TOOL_CONTENTS_DOMAIN, contents),
    );
    encoded
}

fn environment_identity(environment: &[EnvironmentInput]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(environment.len() as u64).to_le_bytes());
    for input in environment {
        write_bytes(&mut encoded, input.name.as_bytes());
        match &input.value {
            Some(value) => {
                encoded.push(1);
                write_bytes(&mut encoded, value.as_bytes());
            }
            None => encoded.push(0),
        }
    }
    encoded
}

fn input_identity(inputs: &[(&LogicalPath, &[u8])]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(inputs.len() as u64).to_le_bytes());
    for (path, contents) in inputs {
        write_bytes(&mut encoded, &logical_path_identity(path));
        encoded.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        write_fingerprint(
            &mut encoded,
            bytes_fingerprint(EXTERNAL_COMMAND_INPUT_CONTENTS_DOMAIN, contents),
        );
    }
    encoded
}

fn path_list_identity(paths: &[LogicalPath]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(paths.len() as u64).to_le_bytes());
    for path in paths {
        write_bytes(&mut encoded, &logical_path_identity(path));
    }
    encoded
}

fn identity_count(identity: &[u8]) -> Option<usize> {
    let mut cursor = Cursor::new(identity);
    usize::try_from(read_u64(&mut cursor)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn identity() -> ExternalCommandCacheIdentity {
        let action = b"action".to_vec();
        let command = b"command".to_vec();
        let tool = b"tool".to_vec();
        let environment = b"environment".to_vec();
        let inputs = b"inputs".to_vec();
        let dependencies = b"dependencies".to_vec();
        let working_directory = b"working-directory".to_vec();
        let package_roots = b"package-roots".to_vec();
        let mut outputs = Vec::new();
        outputs.extend_from_slice(&2_u64.to_le_bytes());
        outputs.extend_from_slice(b"outputs");
        let cache_key = component(EXTERNAL_COMMAND_KEY_DOMAIN, &action);
        let components = FingerprintComponents {
            command: component(EXTERNAL_COMMAND_DECLARATION_DOMAIN, &command),
            tool: component(EXTERNAL_COMMAND_TOOL_DOMAIN, &tool),
            environment: component(EXTERNAL_COMMAND_ENVIRONMENT_DOMAIN, &environment),
            inputs: component(EXTERNAL_COMMAND_INPUTS_DOMAIN, &inputs),
            dependencies: component(EXTERNAL_COMMAND_DEPENDENCIES_DOMAIN, &dependencies),
            working_directory: component(
                EXTERNAL_COMMAND_WORKING_DIRECTORY_DOMAIN,
                &working_directory,
            ),
            package_roots: component(EXTERNAL_COMMAND_PACKAGE_ROOTS_DOMAIN, &package_roots),
            outputs: component(EXTERNAL_COMMAND_OUTPUTS_DOMAIN, &outputs),
            compiler: QueryFingerprint::from_parts([1, 2]),
            resource_layout: QueryFingerprint::from_parts([3, 4]),
            standard_library: QueryFingerprint::from_parts([5, 6]),
            build_protocol: QueryFingerprint::from_parts([7, 8]),
        };
        ExternalCommandCacheIdentity {
            fingerprints: FingerprintSet::new(cache_key, components),
            action,
            command,
            tool,
            environment,
            inputs,
            dependencies,
            working_directory,
            package_roots,
            outputs,
            output_count: 2,
        }
    }

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia-external-command-cache-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn envelope_rejects_every_truncated_prefix_and_trailing_bytes() {
        let identity = identity();
        let payloads = vec![b"first".to_vec(), b"second".to_vec()];
        let encoded = encode_entry(&identity, &payloads);
        let decoded = decode_entry(&encoded).unwrap();
        assert!(entry_matches(&decoded, &identity));
        assert_eq!(decoded.payloads, payloads);
        for end in 0..encoded.len() {
            assert!(decode_entry(&encoded[..end]).is_none(), "prefix {end}");
        }
        let mut trailing = encoded;
        trailing.push(0);
        assert!(decode_entry(&trailing).is_none());
    }

    #[test]
    fn envelope_rejects_payload_checksum_damage() {
        let identity = identity();
        let mut encoded = encode_entry(&identity, &[b"first".to_vec(), b"second".to_vec()]);
        *encoded.last_mut().unwrap() ^= 1;
        assert!(decode_entry(&encoded).is_none());
    }

    #[test]
    fn immutable_identity_rejects_different_outputs() {
        let root = root("nondeterministic");
        let cache = ExternalCommandCache::new(root.clone());
        let identity = identity();
        let first = vec![b"first".to_vec(), b"second".to_vec()];
        let changed = vec![b"changed".to_vec(), b"second".to_vec()];
        cache.publish(&identity, &first).unwrap();

        let error = cache.publish(&identity, &changed).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            cache.lookup(&identity).unwrap(),
            ExternalCommandCacheLookup::Hit(first)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_artifact_change_has_its_own_invalidation_reason() {
        let root = root("dependency-invalidation");
        let cache = ExternalCommandCache::new(root.clone());
        let identity = identity();
        cache
            .publish(&identity, &[b"first".to_vec(), b"second".to_vec()])
            .unwrap();

        let mut changed = identity.clone();
        changed.dependencies = b"changed-dependencies".to_vec();
        changed.fingerprints.components.dependencies =
            component(EXTERNAL_COMMAND_DEPENDENCIES_DOMAIN, &changed.dependencies);
        changed.fingerprints = FingerprintSet::new(
            changed.fingerprints.cache_key,
            changed.fingerprints.components,
        );

        assert_eq!(
            cache.lookup(&changed).unwrap(),
            ExternalCommandCacheLookup::Miss(ActionCacheMissReason::Invalidated(vec![
                ActionCacheInvalidation::Dependencies,
            ]))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_root_mapping_change_has_its_own_invalidation_reason() {
        let baseline = identity().fingerprints.components;
        let changed = FingerprintComponents {
            package_roots: QueryFingerprint::from_parts([20, 20]),
            ..baseline
        };

        assert_eq!(
            invalidations(baseline, changed),
            [ActionCacheInvalidation::PackageRoots]
        );
    }
}
