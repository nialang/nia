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
}

/// The current producer input corresponding to a range in generated source.
pub struct SourceOrigin {
    /// Physical input file.
    pub path: PathBuf,
    /// Verified UTF-8 input text.
    pub text: Arc<str>,
    /// Input range chosen by the producer.
    pub span: Span,
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
    source_path: String,
    source_digest: String,
    source: [usize; 2],
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
        let map_name = format!("{relative}.map");
        let logical_map = LogicalPath::new(LogicalPathRoot::Build, &map_name)
            .map_err(|_| invalid("invalid generated source map path"))?;
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
        let map = output.join(map_name);
        if canonical_file(&map)? != normalize_path(&map.to_string_lossy()) {
            return Err(invalid(
                "generated source map resolves outside its declared path",
            ));
        }
        let Manifest {
            version,
            generated_digest,
            mut mappings,
        } = serde_json::from_str(&read_source_text(map)?).map_err(io::Error::other)?;
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
        }))
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
        let relative = LogicalPath::new(
            LogicalPathRoot::Package(self.root.clone()),
            &mapping.source_path,
        )
        .map_err(|_| invalid("invalid mapped source path"))?;
        if !self.inputs.iter().any(|input| {
            input.root() == relative.root() && relative.components().starts_with(input.components())
        }) {
            return Err(invalid("mapped source is not a declared producer input"));
        }
        let path = package_root.join(relative.protocol_path());
        let physical = canonical_file(&path)?;
        if path.starts_with(output) || physical != normalize_path(&path.to_string_lossy()) {
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
        Ok(Some(SourceOrigin { path, text, span }))
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
        assert!(GeneratedSourceMap::read(&plan, &output, &generated_path, generated).is_err());
    }
}
