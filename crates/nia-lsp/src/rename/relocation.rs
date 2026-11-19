// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects module declarations and qualified names onto a new parent.

use super::*;
use nia_syntax::{GreenNode, SyntaxKind};

pub(super) struct NamespaceRewrite {
    pub(super) before: Site,
    pub(super) old_range: Span,
    destination: Site,
    name: Span,
    qualified: bool,
}

impl NamespaceRewrite {
    pub(super) fn translated(
        &self,
        original: Span,
        offsets: &EditOffsets<'_>,
        moves: &Moves,
    ) -> Option<Site> {
        let relative = if original == self.before.span {
            self.name
        } else if self.qualified {
            return None;
        } else if original.end <= self.before.span.start {
            let start = self.name.start - (self.before.span.start - original.start);
            Span::new(start, start + original.len())
        } else if self.before.span.end <= original.start {
            let start = self.name.end + original.start - self.before.span.end;
            Span::new(start, start + original.len())
        } else {
            return None;
        };
        let edit = offsets.binding_span(&self.destination.source, self.destination.span);
        Some(Site {
            source: relocated(&self.destination.source, moves).map_or_else(
                || self.destination.source.clone(),
                |path| normalized(&path).into(),
            ),
            span: Span::new(edit.start + relative.start, edit.start + relative.end),
        })
    }

    pub(super) fn remove_new_prefixes(
        &self,
        bindings: &mut Bindings,
        offsets: &EditOffsets<'_>,
        moves: &Moves,
    ) {
        if self.qualified
            && let Some(name) = self.translated(self.before.span, offsets, moves)
        {
            let prefix = Span::new(name.span.start - self.name.start, name.span.start);
            if let Some(entries) = bindings.get_mut(name.source.as_ref()) {
                entries.retain(|span, _| !contains(prefix, *span));
            }
        }
    }
}

pub(super) fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

pub(super) fn collect_relocation(
    analysis: &Analysis,
    workspace: &WorkspaceIndex,
    symbol: Symbol,
    source: &Path,
    destination: &Path,
    cancel: &Cancellation,
) -> Result<RenamePlan, String> {
    let new_name = destination
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("Invalid module name")?;
    validate_new_name(symbol, new_name)?;
    let parent = analysis
        .destination_module(symbol, destination)
        .ok_or("A module destination must have a unique declaring parent in its package")?;
    let (declaring, name_span) = analysis
        .definitions
        .get(&symbol)
        .ok_or("Module declaration is unavailable")?;
    let before = Site {
        source: Arc::from(declaring.as_str()),
        span: *name_span,
    };
    let old_file = analysis
        .file(Path::new(declaring))
        .ok_or("Module declaration source is unavailable")?;
    let item = declaration_item(old_file.syntax().tree.green_root(), *name_span)
        .ok_or("Could not locate the module declaration")?;
    let old_range = declaration_range(
        &old_file.text,
        item.token_span().ok_or("Missing module declaration")?,
    );
    let new_file = analysis
        .file(Path::new(parent))
        .ok_or("Destination module source is unavailable")?;
    let insertion = Span::new(new_file.text.len(), new_file.text.len());
    let newline = if new_file.text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut after = String::new();
    if !new_file.text.is_empty() && !new_file.text.ends_with(['\n', '\r']) {
        after.push_str(newline);
    }
    after.push_str(&old_file.text[old_range.start..name_span.start]);
    let name = Span::new(after.len(), after.len() + new_name.len());
    after.push_str(new_name);
    after.push_str(&old_file.text[name_span.end..old_range.end]);
    if !after.ends_with(['\n', '\r']) {
        after.push_str(newline);
    }

    for (path, file) in [(declaring.as_str(), old_file), (parent, new_file)] {
        if workspace.files.get(path).map(|(_, text)| text) != Some(&file.text) {
            return Err("Module declaration changed during the file operation".into());
        }
    }
    let moves = module_move_to(source, destination.to_path_buf(), workspace)?;
    let mut plan = RenamePlan {
        moves,
        namespaces: vec![NamespaceRewrite {
            before: before.clone(),
            old_range,
            destination: Site {
                source: Arc::from(parent),
                span: insertion,
            },
            name,
            qualified: false,
        }],
        ..RenamePlan::default()
    };
    plan.edits
        .entry(declaring.clone())
        .or_default()
        .insert(old_range, String::new());
    plan.edits
        .entry(parent.to_owned())
        .or_default()
        .insert(insertion, after);
    let matches = |binding: &Binding| {
        binding.alias.as_ref().or(binding.declaration.as_ref()) == Some(&before)
    };
    let mut uses = BTreeSet::new();
    for entry in &workspace.entries {
        cancel.check()?;
        if workspace.entry_may_reach(entry, declaring)
            && let Coverage::Unavailable(error) = &entry.coverage
        {
            return Err(format!("Cannot verify move in {} ({error})", entry.request));
        }
        for (file, entries) in &entry.bindings {
            cancel.check()?;
            for (&span, bindings) in entries {
                if (file != declaring || span != *name_span) && bindings.iter().any(matches) {
                    uses.insert(Site {
                        source: Arc::from(file.as_str()),
                        span,
                    });
                }
            }
        }
    }
    let mut tokens = BTreeMap::new();
    for site in uses {
        cancel.check()?;
        if workspace.entries.iter().any(|entry| {
            entry
                .bindings
                .get(site.source.as_ref())
                .and_then(|entries| entries.get(&site.span))
                .is_some_and(|bindings| !bindings.iter().all(matches))
        }) {
            return Err(format!(
                "A module path resolves differently between entries in {}",
                site.source
            ));
        }
        let (_, text) = &workspace.files[site.source.as_ref()];
        let file_tokens = tokens
            .entry(site.source.clone())
            .or_insert_with(|| tokenize(text));
        let range =
            path_prefix(file_tokens, site.span).ok_or("Cannot locate a qualified module path")?;
        if text[range.start..range.end].contains("//") {
            return Err(format!(
                "A module path contains a comment in {}",
                site.source
            ));
        }
        let prefix = analysis
            .rooted_module_path(&site.source, parent)
            .ok_or_else(|| {
                format!(
                    "Destination module has no visible path from {}",
                    site.source
                )
            })?;
        let replacement = format!("{prefix}::{new_name}");
        let name = Span::new(prefix.len() + 2, replacement.len());
        let edits = plan.edits.entry(site.source.to_string()).or_default();
        if edits
            .keys()
            .any(|span| span.start < range.end && range.start < span.end)
        {
            return Err(format!("Module paths overlap in {}", site.source));
        }
        edits.insert(range, replacement);
        plan.namespaces.push(NamespaceRewrite {
            before: site.clone(),
            old_range: range,
            destination: Site {
                source: site.source,
                span: range,
            },
            name,
            qualified: true,
        });
    }
    Ok(plan)
}

fn declaration_item(root: &GreenNode, name: Span) -> Option<&GreenNode> {
    root.child_nodes().find(|item| {
        item.child(&SyntaxKind::ModuleDecl)
            .and_then(|decl| decl.token(&TokenKind::Ident))
            .is_some_and(|token| token.span() == name)
    })
}

fn declaration_range(text: &str, item: Span) -> Span {
    let mut start = text[..item.start].rfind('\n').map_or(0, |line| line + 1);
    if !text[start..item.start].trim().is_empty() {
        return item;
    }
    while let Some(previous) = text[..start].strip_suffix('\n') {
        let line = previous.rfind('\n').map_or(0, |line| line + 1);
        if !previous[line..].trim_start().starts_with("///") {
            break;
        }
        start = line;
    }
    let line_end = text[item.end..]
        .find('\n')
        .map_or(text.len(), |line| item.end + line + 1);
    let tail = text[item.end..line_end].trim();
    Span::new(
        start,
        if tail.is_empty() || tail.starts_with("//") {
            line_end
        } else {
            item.end
        },
    )
}

fn path_prefix(tokens: &[nia_lexer::Token], name: Span) -> Option<Span> {
    let mut index = tokens
        .binary_search_by_key(&name, |token| token.span)
        .ok()?;
    while index >= 2
        && tokens[index - 1].kind == TokenKind::ColonColon
        && matches!(
            tokens[index - 2].kind,
            TokenKind::Ident | TokenKind::Root | TokenKind::Super
        )
    {
        index -= 2;
    }
    Some(Span::new(tokens[index].span.start, name.end))
}
