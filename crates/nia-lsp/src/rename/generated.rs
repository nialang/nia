// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects verifiable generated renames back to the producer's inputs.

use super::*;
use nia_build::source_map::GeneratedSourceSnapshot;

#[derive(Default)]
pub(super) struct GeneratedEdits {
    pub(super) outputs: BTreeSet<String>,
    pub(super) sources: BTreeMap<String, (lsp_types::Uri, Arc<str>)>,
    snapshots: BTreeMap<String, GeneratedSourceSnapshot>,
}

impl GeneratedEdits {
    fn observe(
        snapshots: &mut BTreeMap<String, GeneratedSourceSnapshot>,
        source: &str,
        snapshot: &GeneratedSourceSnapshot,
    ) -> Result<(), String> {
        if snapshots
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
        let mut pending: Edits = edits
            .iter()
            .filter(|(source, _)| workspace.managed_source(Path::new(source)))
            .map(|(source, spans)| (source.clone(), spans.clone()))
            .collect();
        if pending.is_empty() && workspace.configuration.plans.is_empty() {
            return Ok(generated);
        }

        let mut projected = edits.clone();
        while let Some((source, spans)) = pending.pop_first() {
            cancel.check()?;
            let path = Path::new(&source);
            let text = current_text(workspace, documents, path)?;
            let package = self
                .projects
                .package(path)?
                .ok_or("Generated source has no producer package")?;
            let map = package
                .generated_source_map(path, &text)
                .map_err(|error| {
                    format!("Cannot read generated source map for {source} ({error})")
                })?
                .ok_or_else(|| format!("Missing generated source map for {source}"))?;
            GeneratedEdits::observe(&mut generated.snapshots, &source, map.snapshot())?;
            generated.outputs.insert(source.clone());
            let output_uri = uri(path).ok_or("Cannot form a generated source URI")?;
            if generated
                .sources
                .insert(source.clone(), (output_uri, text.clone()))
                .is_some_and(|(_, before)| before != text)
            {
                return Err(format!("Generated source changed during rename ({source})"));
            }
            for (span, replacement) in spans {
                cancel.check()?;
                let origin = map
                    .origin(
                        &package.source_root(),
                        &package.output_root(),
                        span,
                        |input| load_input(documents, input),
                    )
                    .map_err(|error| {
                        format!("Cannot resolve generated source for {source} ({error})")
                    })?;
                let Some(origin) = origin.filter(|origin| {
                    origin.editable
                        && text.get(span.start..span.end).is_some_and(identifier)
                        && identifier(&replacement)
                }) else {
                    return Err(format!(
                        "A generated range has no editable source map ({source})"
                    ));
                };
                let input = normalized(&origin.path);
                let input_path = Path::new(&input);
                let intermediate = workspace.managed_source(input_path);
                if !intermediate && !workspace.editable_source(input_path) {
                    return Err(format!(
                        "A rename would edit a generator input outside the workspace ({input})"
                    ));
                }
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
                let input_uri = uri(input_path).ok_or("Cannot form a generator input URI")?;
                if workspace
                    .files
                    .get(&input)
                    .is_some_and(|(_, previous)| *previous != origin.text)
                    || generated
                        .sources
                        .get(&input)
                        .is_some_and(|(_, previous)| *previous != origin.text)
                {
                    return Err(format!("Generator input changed during rename ({input})"));
                }
                generated
                    .sources
                    .insert(input.clone(), (input_uri, origin.text));
                match projected
                    .entry(input.clone())
                    .or_default()
                    .entry(origin.span)
                {
                    std::collections::btree_map::Entry::Occupied(previous) => {
                        if previous.get() != &replacement {
                            return Err(format!(
                                "A generator input requires conflicting edits ({input})"
                            ));
                        }
                    }
                    std::collections::btree_map::Entry::Vacant(next) => {
                        next.insert(replacement.clone());
                        if intermediate {
                            pending
                                .entry(input)
                                .or_default()
                                .insert(origin.span, replacement);
                        }
                    }
                }
            }
        }

        // An edit can affect the generator even when the rename starts in a
        // handwritten input and no generated binding was selected.
        for (input, spans) in &projected {
            cancel.check()?;
            let input_path = Path::new(input);
            let mut outputs = BTreeSet::new();
            for owner in workspace.configuration.plans.keys() {
                if let Some(package) = self.projects.package(&owner.join("nia.toml"))? {
                    outputs.extend(package.generated_outputs_reading(input_path));
                }
            }
            if outputs.is_empty() {
                continue;
            }
            let input_text = match generated.sources.get(input) {
                Some((_, text)) => text.clone(),
                None => current_text(workspace, documents, input_path)?,
            };
            for output in outputs {
                cancel.check()?;
                let output_key = normalized(&output);
                let text = current_text(workspace, documents, &output)?;
                let package = self
                    .projects
                    .package(&output)?
                    .ok_or("Generated source has no producer package")?;
                let map = package
                    .generated_source_map(&output, &text)
                    .map_err(|error| {
                        format!("Cannot read generated source map for {output_key} ({error})")
                    })?
                    .ok_or_else(|| format!("Missing generated source map for {output_key}"))?;
                GeneratedEdits::observe(&mut generated.snapshots, &output_key, map.snapshot())?;
                for (&span, replacement) in spans {
                    let copies = map
                        .input_copies(
                            &package.source_root(),
                            &package.output_root(),
                            input_path,
                            &input_text,
                            span,
                        )
                        .map_err(|error| {
                            format!("Cannot project rename through {output_key} ({error})")
                        })?;
                    for copy in copies {
                        if projected
                            .get(&output_key)
                            .and_then(|spans| spans.get(&copy))
                            != Some(replacement)
                        {
                            return Err(format!(
                                "Generator input edit would change another generated occurrence ({output_key})"
                            ));
                        }
                    }
                }
            }
        }
        *edits = projected;
        Ok(generated)
    }
}

fn load_input(documents: &Documents, path: &Path) -> std::io::Result<Arc<str>> {
    documents.get(Path::new(&normalized(path))).map_or_else(
        || nia_source::read_source_text(path).map(Arc::from),
        |open| Ok(open.document.text.clone()),
    )
}

fn current_text(
    workspace: &WorkspaceIndex,
    documents: &Documents,
    path: &Path,
) -> Result<Arc<str>, String> {
    match workspace.files.get(&normalized(path)) {
        Some((_, text)) => Ok(text.clone()),
        None => load_input(documents, path)
            .map_err(|error| format!("Cannot read generated file {} ({error})", path.display())),
    }
}
