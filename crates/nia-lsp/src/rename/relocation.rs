// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects module declarations and qualified names onto a new parent.

use super::*;
use nia_syntax::{GreenNode, SyntaxKind};

pub(super) struct NamespaceRewrite {
    pub(super) before: Site,
    pub(super) old_range: Span,
    destination: Site,
    name: Span,
    form: NamespaceForm,
}

enum NamespaceForm {
    Declaration,
    Path,
    Import { suffix_end: usize },
    Parent,
}

impl NamespaceRewrite {
    pub(super) fn destination_parent(&self) -> Option<&str> {
        matches!(self.form, NamespaceForm::Declaration).then_some(self.destination.source.as_ref())
    }

    pub(super) fn translated(
        &self,
        original: Span,
        offsets: &EditOffsets<'_>,
        moves: &Moves,
    ) -> Option<Site> {
        let relative = if matches!(self.form, NamespaceForm::Parent) {
            return None;
        } else if original == self.before.span {
            self.name
        } else if matches!(self.form, NamespaceForm::Declaration)
            && original.end <= self.before.span.start
        {
            let start = self.name.start - (self.before.span.start - original.start);
            Span::new(start, start + original.len())
        } else if self.before.span.end <= original.start
            && match self.form {
                NamespaceForm::Declaration => true,
                NamespaceForm::Path | NamespaceForm::Parent => false,
                NamespaceForm::Import { suffix_end } => original.end <= suffix_end,
            }
        {
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
        let prefix = match self.form {
            NamespaceForm::Declaration => return,
            NamespaceForm::Parent => self.name.end,
            _ => self.name.start,
        };
        let source = relocated(&self.destination.source, moves).map_or_else(
            || self.destination.source.to_string(),
            |path| normalized(&path),
        );
        let edit = offsets.binding_span(&self.destination.source, self.destination.span);
        if let Some(entries) = bindings.get_mut(&source) {
            entries.retain(|span, _| !contains(Span::new(edit.start, edit.start + prefix), *span));
        }
    }
}

pub(super) fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

pub(super) struct RelocationDestination<'a> {
    pub(super) file: &'a Path,
    pub(super) parent: &'a str,
    pub(super) suffix: &'a str,
    pub(super) crosses_package: bool,
}

pub(super) fn collect_relocation(
    analysis: &Analysis,
    workspace: &WorkspaceIndex,
    symbol: Symbol,
    source: &Path,
    destination: RelocationDestination<'_>,
    cancel: &Cancellation,
    mut resolve_root: impl FnMut(&Site) -> Result<Option<String>, String>,
) -> Result<RenamePlan, String> {
    let new_name = destination
        .file
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("Invalid module name")?;
    validate_new_name(symbol, new_name)?;
    let parent = destination.parent;
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
    let (_, new_text) = workspace
        .files
        .get(parent)
        .ok_or("Destination module source is unavailable")?;
    let insertion = Span::new(new_text.len(), new_text.len());
    let newline = if new_text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut after = String::new();
    if !new_text.is_empty() && !new_text.ends_with(['\n', '\r']) {
        after.push_str(newline);
    }
    after.push_str(&old_file.text[old_range.start..name_span.start]);
    let name = Span::new(after.len(), after.len() + new_name.len());
    after.push_str(new_name);
    after.push_str(&old_file.text[name_span.end..old_range.end]);
    if !after.ends_with(['\n', '\r']) {
        after.push_str(newline);
    }

    if workspace.files.get(declaring).map(|(_, text)| text) != Some(&old_file.text) {
        return Err("Module declaration changed during the file operation".into());
    }
    let moves = module_move_to(source, destination.file.to_path_buf(), workspace)?;
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
            form: NamespaceForm::Declaration,
        }],
        ..RenamePlan::default()
    };
    if destination.crosses_package {
        for (file, mut sites) in bindings(analysis, cancel)? {
            if relocated(&file, &plan.moves).is_some() {
                plan.transferred.insert(file, sites);
            } else if file == *declaring
                && let Some(declaration) = sites.remove(name_span)
            {
                plan.transferred
                    .entry(file)
                    .or_default()
                    .insert(*name_span, declaration);
            }
        }
    }
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
    let mut syntax = BTreeMap::new();
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
        let parsed = syntax
            .entry(site.source.clone())
            .or_insert_with(|| nia_syntax::parse(text, None));
        let root = parsed
            .as_ref()
            .map_err(|_| "Cannot parse module use")?
            .tree
            .green_root();
        let local_prefix =
            if destination.crosses_package && relocated(&site.source, &plan.moves).is_some() {
                Some(format!("root{}", destination.suffix))
            } else {
                analysis.rooted_module_path(&site.source, parent)
            };
        let prefix = match local_prefix {
            Some(prefix) => prefix,
            None => resolve_root(&site)?
                .map(|root| root + destination.suffix)
                .ok_or_else(|| {
                    format!(
                        "Destination module has no visible path from {}",
                        site.source
                    )
                })?,
        };
        let replacement = format!("{prefix}::{new_name}");
        if let Some(group) = grouped_import(root, site.span) {
            move_grouped_import(&mut plan, text, site, &replacement, new_name.len(), group)?;
            continue;
        }
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
            form: NamespaceForm::Path,
        });
    }
    retain_parent_paths(analysis, source, declaring, &mut plan)?;
    Ok(plan)
}

fn retain_parent_paths(
    analysis: &Analysis,
    source: &Path,
    declaring: &str,
    plan: &mut RenamePlan,
) -> Result<(), String> {
    let file = analysis
        .file(source)
        .ok_or("Moving module source is unavailable")?;
    let source = normalized(source);
    let old_parent = analysis
        .rooted_module_path(&source, declaring)
        .ok_or("Previous module parent is unavailable")?;
    let edits = plan.edits.entry(source.clone()).or_default();
    let tokens = tokenize(&file.text);
    let root = file.syntax().tree.green_root();
    for [token, separator] in tokens.array_windows() {
        if token.kind != TokenKind::Super
            || separator.kind != TokenKind::ColonColon
            || grouped_import(root, token.span).is_some()
            || edits.keys().any(|edit| contains(*edit, token.span))
        {
            continue;
        }
        let site = Site {
            source: Arc::from(source.as_str()),
            span: token.span,
        };
        edits.insert(token.span, old_parent.clone());
        plan.namespaces.push(NamespaceRewrite {
            before: site.clone(),
            old_range: token.span,
            destination: site,
            name: Span::new(0, old_parent.len()),
            form: NamespaceForm::Parent,
        });
    }
    if edits.is_empty() {
        plan.edits.remove(&source);
    }
    Ok(())
}

struct GroupedImport {
    directive: Span,
    header_end: usize,
    suffix_end: usize,
    siblings: Option<(Span, Span)>,
}

fn grouped_import(root: &GreenNode, name: Span) -> Option<GroupedImport> {
    let mut path = vec![root];
    while let Some(child) = path
        .last()?
        .child_nodes()
        .find(|child| contains(child.span(), name))
    {
        path.push(child);
    }
    let directive = path
        .iter()
        .position(|node| matches!(node.kind(), SyntaxKind::UsingStmt | SyntaxKind::UsingDecl))?;
    let tree = path[directive]
        .child(&SyntaxKind::UsingTree)?
        .token_span()?;
    let item = if path[directive - 1].kind() == &SyntaxKind::Item {
        path[directive - 1]
    } else {
        path[directive]
    };
    let mut groups = path
        .windows(2)
        .filter(|pair| pair[0].kind() == &SyntaxKind::UsingGroup)
        .rev();
    let deepest = groups.next()?;
    let selector = deepest[1].token_span()?;
    let siblings = std::iter::once(deepest).chain(groups).find_map(|pair| {
        let selected = pair[1].token_span()?;
        let siblings = pair[0]
            .child_nodes()
            .filter_map(GreenNode::token_span)
            .filter(|span| *span != selected);
        siblings
            .min_by_key(|span| span.start.abs_diff(selected.start))
            .map(|neighbor| (selected, neighbor))
    });
    Some(GroupedImport {
        directive: item.token_span()?,
        header_end: tree.start,
        suffix_end: selector.end,
        siblings,
    })
}

fn move_grouped_import(
    plan: &mut RenamePlan,
    text: &str,
    site: Site,
    replacement: &str,
    name_len: usize,
    group: GroupedImport,
) -> Result<(), String> {
    let mut rewritten = String::new();
    let erase;
    let destination;
    if let Some((selector, neighbor)) = group.siblings {
        let (gap, removed) = if selector.start < neighbor.start {
            (
                &text[selector.end..neighbor.start],
                Span::new(selector.start, neighbor.start),
            )
        } else {
            (
                &text[neighbor.end..selector.start],
                Span::new(neighbor.end, selector.end),
            )
        };
        if !matches!(gap.trim(), "," | "") || gap.contains("//") {
            return Err("Cannot move a grouped import across a comment".into());
        }
        erase = removed;
        let line = text[..group.directive.start]
            .rfind('\n')
            .map_or(0, |line| line + 1);
        let indent = &text[line..group.directive.start];
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let ending = text[group.directive.end..]
            .find('\n')
            .map_or(text.len(), |line| group.directive.end + line);
        if text[group.directive.end..ending]
            .trim_start()
            .starts_with("//")
        {
            let ending = group.directive.end
                + text[group.directive.end..ending]
                    .trim_end_matches('\r')
                    .len();
            destination = Span::new(ending, ending);
        } else {
            destination = Span::new(group.directive.end, group.directive.end);
        };
        rewritten.push_str(newline);
        if indent.trim().is_empty() {
            rewritten.push_str(indent);
        }
    } else {
        erase = group.directive;
        destination = group.directive;
    }
    if text[erase.start..site.span.start].contains("//")
        || text[group.suffix_end..erase.end.max(group.suffix_end)].contains("//")
    {
        return Err("Cannot drop a comment while moving a grouped import".into());
    }
    rewritten.push_str(&text[group.directive.start..group.header_end]);
    rewritten.push_str(replacement);
    let name = Span::new(rewritten.len() - name_len, rewritten.len());
    rewritten.push_str(&text[site.span.end..group.suffix_end]);
    rewritten.push(';');
    let edits = plan.edits.entry(site.source.to_string()).or_default();
    if erase != destination && edits.insert(erase, String::new()).is_some()
        || edits.insert(destination, rewritten).is_some()
    {
        return Err("Grouped import moves require overlapping edits".into());
    }
    plan.namespaces.push(NamespaceRewrite {
        before: site.clone(),
        old_range: erase,
        destination: Site {
            source: site.source,
            span: destination,
        },
        name,
        form: NamespaceForm::Import {
            suffix_end: group.suffix_end,
        },
    });
    Ok(())
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
