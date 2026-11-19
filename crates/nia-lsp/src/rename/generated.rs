// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects verifiable generated renames back to the producer's inputs.

use super::*;
use nia_build::source_map::GeneratedSourceSnapshot;

#[derive(Default)]
pub(super) struct GeneratedEdits {
    pub(super) outputs: BTreeSet<String>,
    pub(super) inputs: BTreeMap<String, (lsp_types::Uri, Arc<str>)>,
    snapshots: BTreeMap<String, GeneratedSourceSnapshot>,
}

impl GeneratedEdits {
    fn observe(&mut self, source: &str, snapshot: &GeneratedSourceSnapshot) -> Result<(), String> {
        if self
            .snapshots
            .insert(source.to_owned(), snapshot.clone())
            .is_some_and(|previous| previous != *snapshot)
        {
            return Err(format!(
                "Generated source map changed during rename ({source})"
            ));
        }
        Ok(())
    }

    pub(super) fn verify_current(&self, cancel: &Cancellation) -> Result<(), String> {
        for (source, snapshot) in &self.snapshots {
            cancel.check()?;
            snapshot.verify_current().map_err(|error| {
                format!("Cannot verify the generated snapshot for {source} ({error})")
            })?;
        }
        Ok(())
    }
}

impl Session {
    pub(super) fn project_generated_rename(
        &mut self,
        workspace: &WorkspaceIndex,
        documents: &Documents,
        edits: &mut Edits,
        cancel: &Cancellation,
    ) -> Result<GeneratedEdits, String> {
        let mut generated = GeneratedEdits::default();
        let mut input_edits = Edits::new();
        for (source, spans) in edits.iter() {
            let path = Path::new(source);
            if !workspace.managed_source(path) {
                continue;
            }
            cancel.check()?;
            let (_, text) = workspace
                .files
                .get(source)
                .ok_or("Generated rename source disappeared")?;
            let package = self
                .projects
                .package(path)?
                .ok_or("Generated source has no producer package")?;
            let map = package
                .generated_source_map(path, text)
                .map_err(|error| {
                    format!("Cannot read generated source map for {source} ({error})")
                })?
                .ok_or_else(|| format!("Missing generated source map for {source}"))?;
            generated.observe(source, map.snapshot())?;
            for (&span, replacement) in spans {
                let origin = map
                    .origin(
                        &package.source_root(),
                        &package.output_root(),
                        span,
                        |input| {
                            documents.get(Path::new(&normalized(input))).map_or_else(
                                || nia_source::read_source_text(input).map(Arc::from),
                                |open| Ok(open.document.text.clone()),
                            )
                        },
                    )
                    .map_err(|error| {
                        format!("Cannot resolve generated source for {source} ({error})")
                    })?;
                let Some(origin) = origin.filter(|origin| {
                    origin.editable
                        && workspace.editable_source(Path::new(&normalized(&origin.path)))
                        && text.get(span.start..span.end).is_some_and(identifier)
                        && identifier(replacement)
                }) else {
                    return Err(format!(
                        "A rename would edit a file outside the workspace or a generated range without an editable source map ({source})"
                    ));
                };
                let input = normalized(&origin.path);
                let declared = edits.get(&input).and_then(|spans| spans.get(&origin.span));
                if declared.is_none()
                    && workspace.entries.iter().any(|entry| {
                        entry
                            .bindings
                            .get(&input)
                            .is_some_and(|bindings| bindings.contains_key(&origin.span))
                    })
                {
                    return Err(format!(
                        "A generator input has a separate source binding ({input})"
                    ));
                }
                if declared.is_some_and(|previous| previous != replacement)
                    || input_edits
                        .entry(input.clone())
                        .or_default()
                        .insert(origin.span, replacement.clone())
                        .is_some_and(|previous| previous != *replacement)
                {
                    return Err(format!(
                        "A generator input requires conflicting edits ({input})"
                    ));
                }
                let input_uri = uri(&origin.path).ok_or("Cannot form a generator input URI")?;
                if workspace
                    .files
                    .get(&input)
                    .is_some_and(|(_, previous)| *previous != origin.text)
                    || generated
                        .inputs
                        .insert(input.clone(), (input_uri, origin.text.clone()))
                        .is_some_and(|(_, previous)| previous != origin.text)
                {
                    return Err(format!("Generator input changed during rename ({input})"));
                }
            }
            generated.outputs.insert(source.clone());
        }

        for (input, mut spans) in input_edits {
            cancel.check()?;
            let input_path = Path::new(&input);
            if let Some(original) = edits.get(&input) {
                spans.extend(original.clone());
            }
            let mut outputs: BTreeSet<_> = generated.outputs.iter().map(PathBuf::from).collect();
            for source in &generated.outputs {
                if let Some(package) = self.projects.package(Path::new(source))? {
                    outputs.extend(package.generated_outputs_reading(input_path));
                }
            }
            let input_text = Arc::clone(&generated.inputs[&input].1);
            for output in outputs {
                cancel.check()?;
                let output_key = normalized(&output);
                let disk;
                let text = if let Some((_, text)) = workspace.files.get(&output_key) {
                    text.as_ref()
                } else if let Some(open) = documents.get(Path::new(&output_key)) {
                    open.document.text.as_ref()
                } else {
                    disk = nia_source::read_source_text(&output).map_err(|error| {
                        format!("Cannot read generated file {} ({error})", output.display())
                    })?;
                    &disk
                };
                let package = self
                    .projects
                    .package(&output)?
                    .ok_or("Generated source has no producer package")?;
                let map = package
                    .generated_source_map(&output, text)
                    .map_err(|error| {
                        format!(
                            "Cannot read generated source map for {} ({error})",
                            output.display()
                        )
                    })?
                    .ok_or_else(|| {
                        format!("Missing generated source map for {}", output.display())
                    })?;
                generated.observe(&output_key, map.snapshot())?;
                for (&span, replacement) in &spans {
                    for copy in map
                        .input_copies(&package.source_root(), input_path, &input_text, span)
                        .map_err(|error| {
                            format!(
                                "Cannot project rename through {} ({error})",
                                output.display()
                            )
                        })?
                    {
                        if edits.get(&output_key).and_then(|spans| spans.get(&copy))
                            != Some(replacement)
                        {
                            return Err(format!(
                                "Generator input edit would change another generated occurrence ({})",
                                output.display()
                            ));
                        }
                    }
                }
            }
            edits.entry(input).or_default().extend(spans);
        }
        Ok(generated)
    }
}
