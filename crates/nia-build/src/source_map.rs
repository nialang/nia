// SPDX-License-Identifier: GPL-3.0-or-later
//! Source provenance published with a generated source file by one build action.

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use nia_build_plan::{ActionKind, BuildPlan, LogicalPath, LogicalPathRoot};
use nia_source::{normalize_path, read_source_text};
use nia_span::Span;
use serde::Deserialize;

/// A source map whose generated ranges match the current generated text.
pub struct GeneratedSourceMap<'a> {
    generated_text: &'a str,
    entries: Vec<Mapping>,
    root: nia_build_plan::PackageKey,
    inputs: Vec<LogicalPath>,
    snapshot: GeneratedSourceSnapshot,
}

/// The generated source and mapping observed during validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedSourceSnapshot {
    generated: PathBuf,
    generated_digest: String,
    map: PathBuf,
    map_digest: blake3::Hash,
}

impl GeneratedSourceSnapshot {
    /// Checks that the producer outputs still match the validated snapshots on disk.
    pub fn verify_current(&self) -> io::Result<()> {
        if canonical_file(&self.generated)? != normalize_path(&self.generated.to_string_lossy())
            || canonical_file(&self.map)? != normalize_path(&self.map.to_string_lossy())
            || !matches_digest(&read_source_text(&self.generated)?, &self.generated_digest)
            || blake3::hash(read_source_text(&self.map)?.as_bytes()) != self.map_digest
        {
            return Err(invalid("generated source or source map changed"));
        }
        Ok(())
    }
}

/// Lists Nia and source-mapped outputs of commands consuming this input.
pub fn outputs_reading_input(
    plan: &BuildPlan,
    package_root: &Path,
    output: &Path,
    path: &Path,
) -> Vec<PathBuf> {
    let Some(input) = logical_input(plan.root_package(), package_root, output, path) else {
        return Vec::new();
    };
    plan.actions()
        .iter()
        .filter_map(|action| match &action.kind {
            ActionKind::ExternalCommand(command) | ActionKind::RunExecutable(command)
                if command.inputs.iter().any(|path| {
                    path.root() == input.root() && input.components().starts_with(path.components())
                }) =>
            {
                Some(command)
            }
            _ => None,
        })
        .flat_map(|command| {
            command.outputs.iter().filter(|path| {
                path.root() == &LogicalPathRoot::Build
                    && (path.protocol_path().ends_with(".nia")
                        || map_output(path).is_ok_and(|map| command.outputs.contains(&map)))
            })
        })
        .map(|path| output.join(path.protocol_path()))
        .collect()
}

/// Appended to a generated source's file name to name its source map.
const MAP_SUFFIX: &str = ".map";

/// The path of the source map that may accompany the generated source at
/// `generated`.
pub fn map_path(generated: &Path) -> PathBuf {
    let mut name = generated.as_os_str().to_owned();
    name.push(MAP_SUFFIX);
    PathBuf::from(name)
}

fn map_output(source: &LogicalPath) -> Result<LogicalPath, nia_build_plan::LogicalPathError> {
    LogicalPath::new(
        source.root().clone(),
        &format!("{}{MAP_SUFFIX}", source.protocol_path()),
    )
}

fn logical_input(
    package: &nia_build_plan::PackageKey,
    package_root: &Path,
    output: &Path,
    path: &Path,
) -> Option<LogicalPath> {
    let (root, relative) = if let Ok(relative) = path.strip_prefix(output) {
        (LogicalPathRoot::Build, relative)
    } else {
        (
            LogicalPathRoot::Package(package.clone()),
            path.strip_prefix(package_root).ok()?,
        )
    };
    LogicalPath::new(root, &normalize_path(relative.to_str()?)).ok()
}

/// The current producer input corresponding to a range in generated source.
pub struct SourceOrigin {
    /// Physical input file.
    pub path: PathBuf,
    /// Verified UTF-8 input text.
    pub text: Arc<str>,
    /// Input range chosen by the producer.
    pub span: Span,
    /// The producer declares that identifier edits in this byte copy propagate to its output.
    pub editable: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: u32,
    generated_digest: String,
    mappings: Vec<Mapping>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Mapping {
    generated: [usize; 2],
    #[serde(default)]
    source_root: SourceRoot,
    source_path: String,
    source_digest: String,
    source: [usize; 2],
    #[serde(default)]
    editable: bool,
}

#[derive(Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum SourceRoot {
    #[default]
    Package,
    Build,
}

impl<'a> GeneratedSourceMap<'a> {
    /// Loads the `.map` companion if the configured action declares both
    /// exactly named outputs. Digest and range validation use UTF-8 bytes.
    pub fn read(
        plan: &BuildPlan,
        output: &Path,
        generated: &Path,
        generated_text: &'a str,
    ) -> io::Result<Option<Self>> {
        let Ok(relative) = generated.strip_prefix(output) else {
            return Ok(None);
        };
        let relative = normalize_path(&relative.to_string_lossy());
        let name = LogicalPath::new(LogicalPathRoot::Build, &relative)
            .map_err(|_| invalid("invalid generated source path"))?;
        let logical_map =
            map_output(&name).map_err(|_| invalid("invalid generated source map path"))?;
        let Some(producer) = plan.actions().iter().find_map(|action| match &action.kind {
            ActionKind::ExternalCommand(command) | ActionKind::RunExecutable(command)
                if command.outputs.contains(&name) && command.outputs.contains(&logical_map) =>
            {
                Some(command)
            }
            _ => None,
        }) else {
            return Ok(None);
        };
        if canonical_file(generated)? != normalize_path(&generated.to_string_lossy()) {
            return Err(invalid(
                "generated source resolves outside its declared path",
            ));
        }
        let map = output.join(logical_map.protocol_path());
        if canonical_file(&map)? != normalize_path(&map.to_string_lossy()) {
            return Err(invalid(
                "generated source map resolves outside its declared path",
            ));
        }
        let map_text = read_source_text(&map)?;
        let Manifest {
            version,
            generated_digest,
            mut mappings,
        } = serde_json::from_str(&map_text).map_err(io::Error::other)?;
        if version != 1 || !matches_digest(generated_text, &generated_digest) {
            return Err(invalid(
                "generated source map has an unsupported version or stale digest",
            ));
        }
        mappings.sort_by_key(|mapping| mapping.generated);
        if mappings
            .iter()
            .any(|mapping| checked_span(generated_text, mapping.generated).is_none())
            || mappings
                .windows(2)
                .any(|pair| pair[0].generated[1] > pair[1].generated[0])
        {
            return Err(invalid("generated source ranges are invalid or overlap"));
        }
        Ok(Some(Self {
            generated_text,
            entries: mappings,
            root: plan.root_package().clone(),
            inputs: producer.inputs.clone(),
            snapshot: GeneratedSourceSnapshot {
                generated: generated.to_owned(),
                generated_digest,
                map,
                map_digest: blake3::hash(map_text.as_bytes()),
            },
        }))
    }

    /// Returns the identity of the validated source and map snapshots.
    pub fn snapshot(&self) -> &GeneratedSourceSnapshot {
        &self.snapshot
    }

    /// Finds a unique enclosing generated range and validates the mapped input
    /// against the caller's current text, including any unsaved editor buffer.
    /// Byte-identical ranges preserve the offset within that range.
    pub fn origin(
        &self,
        package_root: &Path,
        output: &Path,
        span: Span,
        load: impl FnOnce(&Path) -> io::Result<Arc<str>>,
    ) -> io::Result<Option<SourceOrigin>> {
        let index = self
            .entries
            .partition_point(|mapping| mapping.generated[0] <= span.start);
        let Some(mapping) = index
            .checked_sub(1)
            .and_then(|index| self.entries.get(index))
        else {
            return Ok(None);
        };
        if span.end > mapping.generated[1]
            || checked_span(self.generated_text, [span.start, span.end]).is_none()
        {
            return Ok(None);
        }
        let (root, directory) = match mapping.source_root {
            SourceRoot::Package => (LogicalPathRoot::Package(self.root.clone()), package_root),
            SourceRoot::Build => (LogicalPathRoot::Build, output),
        };
        let relative = LogicalPath::new(root, &mapping.source_path)
            .map_err(|_| invalid("invalid mapped source path"))?;
        if !self.inputs.iter().any(|input| {
            input.root() == relative.root() && relative.components().starts_with(input.components())
        }) {
            return Err(invalid("mapped source is not a declared producer input"));
        }
        let path = directory.join(relative.protocol_path());
        let physical = canonical_file(&path)?;
        if (mapping.source_root == SourceRoot::Package && path.starts_with(output))
            || physical != normalize_path(&path.to_string_lossy())
        {
            return Err(invalid(
                "mapped source is outside the producer's package inputs",
            ));
        }
        let text = load(&path)?;
        if !matches_digest(&text, &mapping.source_digest) {
            return Err(invalid("mapped source digest is stale"));
        }
        let source_span = checked_span(&text, mapping.source)
            .ok_or_else(|| invalid("mapped source range is invalid"))?;
        let copied = self.generated_text[mapping.generated[0]..mapping.generated[1]]
            == text[source_span.start..source_span.end];
        let span = if copied {
            let start = source_span.start + (span.start - mapping.generated[0]);
            Span::new(start, start + span.len())
        } else {
            source_span
        };
        Ok(Some(SourceOrigin {
            path,
            text,
            span,
            editable: copied && mapping.editable,
        }))
    }

    /// Projects an input edit through all verbatim copies declared by the producer.
    /// Rejects edits that cross a mapping boundary or touch a transformed range.
    pub fn input_copies(
        &self,
        package_root: &Path,
        output: &Path,
        path: &Path,
        text: &str,
        span: Span,
    ) -> io::Result<Vec<Span>> {
        let Some(input) = logical_input(&self.root, package_root, output, path) else {
            return Ok(Vec::new());
        };
        let mut copies = Vec::new();
        for mapping in &self.entries {
            let root = match mapping.source_root {
                SourceRoot::Package => LogicalPathRoot::Package(self.root.clone()),
                SourceRoot::Build => LogicalPathRoot::Build,
            };
            if root != *input.root()
                || !LogicalPath::new(root, &mapping.source_path).is_ok_and(|path| path == input)
                || span.end <= mapping.source[0]
                || mapping.source[1] <= span.start
            {
                continue;
            }
            let range = checked_span(text, mapping.source)
                .ok_or_else(|| invalid("mapped source range is invalid"))?;
            if !mapping.editable
                || !matches_digest(text, &mapping.source_digest)
                || range.start > span.start
                || span.end > range.end
                || text[range.start..range.end]
                    != self.generated_text[mapping.generated[0]..mapping.generated[1]]
            {
                return Err(invalid(
                    "generated input edit touches a range without an editable byte copy",
                ));
            }
            let start = mapping.generated[0] + span.start - range.start;
            copies.push(Span::new(start, start + span.len()));
        }
        Ok(copies)
    }
}

fn checked_span(text: &str, [start, end]: [usize; 2]) -> Option<Span> {
    (start < end && text.get(start..end).is_some()).then(|| Span::new(start, end))
}

fn matches_digest(text: &str, digest: &str) -> bool {
    blake3::hash(text.as_bytes()).to_hex().as_str() == digest
}

fn canonical_file(path: &Path) -> io::Result<String> {
    let canonical = path.canonicalize()?;
    if !canonical.is_file() {
        return Err(invalid("source map path must identify a regular file"));
    }
    Ok(normalize_path(&canonical.to_string_lossy()))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_build_plan::{
        ActionKey, ActionResourceClass, BuildPlanDraft, CommandAction, CommandArgument,
        CommandCachePolicy, CommandEnvironmentPolicy, CommandProgram, CommandStdin, PackageKey,
        PlanAction, PlanPackage, PlanPackageSource, PlanStep, StepKey, StepVisibility,
    };

    fn producer(declare_map: bool) -> BuildPlan {
        let package = PackageKey::root();
        let step = StepKey::new(package.clone(), "generate").unwrap();
        let action = ActionKey::new(package.clone(), "generate").unwrap();
        let mut outputs = vec![LogicalPath::new(LogicalPathRoot::Build, "generated.nia").unwrap()];
        if declare_map {
            outputs.push(LogicalPath::new(LogicalPathRoot::Build, "generated.nia.map").unwrap());
        }
        let package_path =
            |path| LogicalPath::new(LogicalPathRoot::Package(package.clone()), path).unwrap();
        let target = nia_target::TargetConfig::host().unwrap();
        BuildPlan::freeze(BuildPlanDraft {
            root_package: package.clone(),
            packages: vec![PlanPackage {
                key: package.clone(),
                source: PlanPackageSource::Local(String::new()),
            }],
            host_target: target,
            artifact_target: target,
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: action.clone(),
                kind: ActionKind::ExternalCommand(CommandAction {
                    resource_class: ActionResourceClass::Io,
                    environment_policy: CommandEnvironmentPolicy::Clear,
                    cache_policy: CommandCachePolicy::Uncacheable,
                    stdin: CommandStdin::Null,
                    program: CommandProgram::Search("generate".into()),
                    arguments: outputs
                        .iter()
                        .cloned()
                        .map(CommandArgument::OutputPath)
                        .collect(),
                    working_directory: package_path(""),
                    environment: Vec::new(),
                    inputs: vec![package_path("definitions.nia")],
                    outputs,
                }),
            }],
            steps: vec![PlanStep {
                key: step.clone(),
                visibility: StepVisibility::Public,
                action,
                dependencies: Vec::new(),
            }],
            default_step: Some(step),
            selected_step: None,
        })
        .unwrap()
    }

    #[test]
    fn map_paths_agree_for_files_and_logical_outputs() {
        let generated = LogicalPath::new(LogicalPathRoot::Build, "gen/parser.nia").unwrap();
        let logical = map_output(&generated).unwrap();
        assert_eq!(logical.protocol_path(), "gen/parser.nia.map");
        assert_eq!(
            map_path(Path::new("out/gen/parser.nia")),
            Path::new("out").join(logical.protocol_path()),
        );
    }

    #[test]
    fn source_map_checks_producer_ownership_and_source_revisions() {
        let root = nia_test_support::test_dir("generated-source-map");
        let root = root.canonicalize().unwrap();
        let output = root.join("out");
        std::fs::create_dir(&output).unwrap();
        let input = "// 🙂\namount";
        let generated = "amount generated";
        let generated_path = output.join("generated.nia");
        let map_path = output.join("generated.nia.map");
        std::fs::write(root.join("definitions.nia"), input).unwrap();
        std::fs::write(&generated_path, generated).unwrap();
        let input_span = [input.find("amount").unwrap(), input.len()];
        let digest = |text: &str| blake3::hash(text.as_bytes()).to_hex().to_string();
        let mapping = |generated| {
            serde_json::json!({
                "generated": generated,
                "sourcePath": "definitions.nia",
                "sourceDigest": digest(input),
                "source": input_span,
            })
        };
        let mut manifest = serde_json::json!({
            "version": 1,
            "generatedDigest": digest(generated),
            "mappings": [mapping([0, 6]), mapping([7, 16])],
        });
        std::fs::write(&map_path, manifest.to_string()).unwrap();
        let plan = producer(true);
        let map = GeneratedSourceMap::read(&plan, &output, &generated_path, generated)
            .unwrap()
            .unwrap();
        assert_eq!(
            outputs_reading_input(&plan, &root, &output, &root.join("definitions.nia")),
            std::slice::from_ref(&generated_path)
        );
        assert!(outputs_reading_input(&plan, &root, &output, &root.join("other.nia")).is_empty());
        map.snapshot().verify_current().unwrap();
        for (span, expected) in [
            (Span::new(0, 6), Span::new(input_span[0], input_span[1])),
            (
                Span::new(1, 5),
                Span::new(input_span[0] + 1, input_span[0] + 5),
            ),
            (Span::new(8, 11), Span::new(input_span[0], input_span[1])),
        ] {
            let origin = map
                .origin(&root, &output, span, |path| {
                    read_source_text(path).map(Arc::from)
                })
                .unwrap()
                .unwrap();
            assert_eq!(origin.path, root.join("definitions.nia"));
            assert_eq!(origin.span, expected);
            assert!(!origin.editable);
        }
        assert!(
            map.origin(&root, &output, Span::new(0, 6), |_| Ok(Arc::from(
                "unsaved"
            )))
            .is_err()
        );
        assert!(GeneratedSourceMap::read(&plan, &output, &generated_path, "changed").is_err());
        assert!(
            GeneratedSourceMap::read(&producer(false), &output, &generated_path, generated)
                .unwrap()
                .is_none()
        );
        manifest["mappings"][1]["generated"] = serde_json::json!([5, 16]);
        std::fs::write(&map_path, manifest.to_string()).unwrap();
        assert!(map.snapshot().verify_current().is_err());
        assert!(GeneratedSourceMap::read(&plan, &output, &generated_path, generated).is_err());
    }

    #[test]
    fn editable_input_copies_account_for_every_mapped_occurrence() {
        let input = "amount";
        let generated = "amount amount changed";
        let root = Path::new("/package");
        let digest = blake3::hash(input.as_bytes()).to_hex().to_string();
        let mapping = |generated, editable| Mapping {
            generated,
            source_root: SourceRoot::Package,
            source_path: "definitions.nia".into(),
            source_digest: digest.clone(),
            source: [0, input.len()],
            editable,
        };
        let mut map = GeneratedSourceMap {
            generated_text: generated,
            entries: vec![mapping([0, 6], true), mapping([7, 13], true)],
            root: nia_build_plan::PackageKey::root(),
            inputs: Vec::new(),
            snapshot: GeneratedSourceSnapshot {
                generated: root.join("out/generated.nia"),
                generated_digest: blake3::hash(generated.as_bytes()).to_hex().to_string(),
                map: root.join("out/generated.nia.map"),
                map_digest: blake3::hash(b""),
            },
        };
        let path = root.join("definitions.nia");
        let range = Span::new(1, 5);
        assert_eq!(
            map.input_copies(root, &root.join("out"), &path, input, range)
                .unwrap(),
            [Span::new(1, 5), Span::new(8, 12)],
        );
        map.entries[1].editable = false;
        assert!(
            map.input_copies(root, &root.join("out"), &path, input, range)
                .is_err()
        );
        map.entries[1] = mapping([14, 21], true);
        assert!(
            map.input_copies(root, &root.join("out"), &path, input, range)
                .is_err()
        );
    }

    #[test]
    fn mapped_build_source_must_be_a_declared_build_input() {
        let root = nia_test_support::test_dir("generated-build-input");
        let root = root.canonicalize().unwrap();
        let output = root.join("out");
        std::fs::create_dir(&output).unwrap();
        let input = output.join("intermediate.nia");
        std::fs::write(&input, "amount").unwrap();
        let generated = output.join("generated.nia");
        let digest = blake3::hash(b"amount").to_hex().to_string();
        std::fs::write(&generated, "amount").unwrap();
        let mut map = GeneratedSourceMap {
            generated_text: "amount",
            entries: vec![Mapping {
                generated: [0, 6],
                source_root: SourceRoot::Build,
                source_path: "intermediate.nia".into(),
                source_digest: digest.clone(),
                source: [0, 6],
                editable: true,
            }],
            root: nia_build_plan::PackageKey::root(),
            inputs: vec![LogicalPath::new(LogicalPathRoot::Build, "intermediate.nia").unwrap()],
            snapshot: GeneratedSourceSnapshot {
                generated,
                generated_digest: digest,
                map: output.join("generated.nia.map"),
                map_digest: blake3::hash(b""),
            },
        };
        let project = |map: &GeneratedSourceMap<'_>| {
            map.origin(&root, &output, Span::new(1, 3), |path| {
                read_source_text(path).map(Arc::from)
            })
        };
        let origin = project(&map).unwrap().unwrap();
        assert_eq!(origin.path, input);
        assert_eq!(origin.span, Span::new(1, 3));
        assert_eq!(
            map.input_copies(&root, &output, &input, "amount", Span::new(1, 3))
                .unwrap(),
            [Span::new(1, 3)],
        );
        map.inputs[0] = LogicalPath::new(
            LogicalPathRoot::Package(map.root.clone()),
            "intermediate.nia",
        )
        .unwrap();
        assert!(project(&map).is_err());
    }
}
