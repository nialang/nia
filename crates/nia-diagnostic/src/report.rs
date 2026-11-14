// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic diagnostic ordering and source-owned causal forests.

use std::cmp::Ordering;
use std::collections::HashSet;

use super::*;

/// Sorts diagnostics for presentation, removes exact duplicates, and applies
/// the configured display limit.
///
/// Deduplication happens before limiting so suppression counts remain stable.
/// The full diagnostic participates in equality: sharing a primary location
/// does not make two different candidate explanations interchangeable.
pub fn build_diagnostic_report<T: DiagnosticReportItem>(
    diagnostics: &[T],
    config: DiagnosticReportConfig,
) -> DiagnosticReport<'_, T> {
    build_diagnostic_report_with_downstream(diagnostics, config, 0)
}

/// Builds a report while retaining the number of downstream diagnostics
/// suppressed by an earlier semantic phase.
pub fn build_diagnostic_report_with_downstream<T: DiagnosticReportItem>(
    diagnostics: &[T],
    config: DiagnosticReportConfig,
    suppressed_downstream: usize,
) -> DiagnosticReport<'_, T> {
    let mut entries = diagnostics.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| compare_report_items(*left, *right));

    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let mut suppressed_duplicates = 0;
    for entry in entries {
        let key = DiagnosticDedupeKey::from_item(entry);
        if !seen.insert(key) {
            suppressed_duplicates += 1;
            continue;
        }
        selected.push(entry);
    }

    // Build the ownership graph from stable source identity, rather than from
    // presentation order. A consequence may itself own another consequence;
    // the only invalid edges are missing, ambiguous, or cyclic ones.
    let mut source_positions: HashMap<(&str, &str, Span), Vec<usize>> = HashMap::new();
    for (index, entry) in selected.iter().enumerate() {
        let diagnostic = entry.report_diagnostic();
        if report_entry_kind(diagnostic) != DiagnosticReportEntryKind::Root {
            continue;
        }
        let Some(path) = entry.report_source_path() else {
            continue;
        };
        let Some(span) = diagnostic.primary_span() else {
            continue;
        };
        source_positions
            .entry((path, diagnostic.code.as_str(), span))
            .or_default()
            .push(index);
    }
    let mut relationships = Vec::with_capacity(selected.len());
    let mut children = vec![Vec::new(); selected.len()];
    let mut parents = vec![None; selected.len()];
    for (index, entry) in selected.iter().enumerate() {
        let diagnostic = entry.report_diagnostic();
        let parent = diagnostic.cause.as_ref().and_then(|cause| {
            let key = (cause.source_path.as_str(), cause.code.as_str(), cause.span);
            let positions = source_positions.get(&key)?;
            (positions.len() == 1).then_some(positions[0])
        });
        parents[index] = parent.filter(|parent| *parent != index);
    }
    let cyclic = causal_cycle_nodes(&parents);
    for (index, entry) in selected.iter().enumerate() {
        let diagnostic = entry.report_diagnostic();
        if let Some(parent) = parents[index].filter(|_| !cyclic[index]) {
            children[parent].push(index);
            relationships.push((DiagnosticReportEntryKind::Related, Some(parent)));
        } else if diagnostic.cause.is_some() {
            relationships.push((DiagnosticReportEntryKind::Independent, None));
        } else {
            relationships.push((report_entry_kind(diagnostic), None));
        }
    }

    let mut ordered_indices = Vec::with_capacity(selected.len());
    let mut visited = vec![false; selected.len()];
    for (index, (kind, _)) in relationships.iter().enumerate() {
        // Every forest component starts at either an owned source root or an
        // independent diagnostic whose cause could not be resolved. Visit
        // those components in the stable priority order above, and emit each
        // causal subtree parent-first.
        if !visited[index] && *kind != DiagnosticReportEntryKind::Related {
            append_causal_subtree(index, &children, &mut visited, &mut ordered_indices);
        }
    }

    let suppressed_by_limit = ordered_indices.len().saturating_sub(config.max_diagnostics);
    ordered_indices.truncate(config.max_diagnostics);
    let mut retained_positions = HashMap::with_capacity(ordered_indices.len());
    for (report_index, selected_index) in ordered_indices.iter().enumerate() {
        retained_positions.insert(*selected_index, report_index);
    }
    let mut entry_kinds = Vec::with_capacity(ordered_indices.len());
    let mut entry_parents = Vec::with_capacity(ordered_indices.len());
    let mut ordered_entries = Vec::with_capacity(ordered_indices.len());
    for selected_index in ordered_indices {
        let (kind, parent) = relationships[selected_index];
        entry_kinds.push(kind);
        entry_parents.push(parent.and_then(|parent| retained_positions.get(&parent).copied()));
        ordered_entries.push(selected[selected_index]);
    }
    selected = ordered_entries;

    let error_count = selected
        .iter()
        .filter(|entry| entry.report_diagnostic().severity() == Severity::Error)
        .count();
    let warning_count = selected
        .iter()
        .filter(|entry| entry.report_diagnostic().severity() == Severity::Warning)
        .count();

    DiagnosticReport {
        entries: selected,
        entry_kinds,
        entry_parents,
        error_count,
        warning_count,
        suppressed_duplicates,
        suppressed_downstream,
        suppressed_by_limit,
    }
}

fn causal_cycle_nodes(parents: &[Option<usize>]) -> Vec<bool> {
    let mut state = vec![0u8; parents.len()];
    let mut cyclic = vec![false; parents.len()];
    for start in 0..parents.len() {
        if state[start] != 0 {
            continue;
        }
        let mut path = Vec::new();
        let mut current = start;
        let cycle_entry = loop {
            if state[current] != 0 {
                break (state[current] == 1).then_some(current);
            }
            state[current] = 1;
            path.push(current);
            let Some(parent) = parents[current] else {
                break None;
            };
            current = parent;
        };
        if let Some(current) = cycle_entry
            && let Some(cycle_start) = path.iter().position(|index| *index == current)
        {
            for index in &path[cycle_start..] {
                cyclic[*index] = true;
            }
        }
        for index in path {
            state[index] = 2;
        }
    }
    cyclic
}

fn append_causal_subtree(
    index: usize,
    children: &[Vec<usize>],
    visited: &mut [bool],
    ordered: &mut Vec<usize>,
) {
    let mut pending = vec![index];
    while let Some(index) = pending.pop() {
        if visited[index] {
            continue;
        }
        visited[index] = true;
        ordered.push(index);
        pending.extend(children[index].iter().rev().copied());
    }
}

fn report_entry_kind(diagnostic: &Diagnostic) -> DiagnosticReportEntryKind {
    match diagnostic.primary_span_source() {
        Some(SpanSource::Source) => DiagnosticReportEntryKind::Root,
        Some(SpanSource::Generated | SpanSource::Fallback) | None => {
            DiagnosticReportEntryKind::Independent
        }
    }
}

fn report_entry_kind_rank(diagnostic: &Diagnostic) -> u8 {
    if diagnostic.cause.is_some() {
        return 1;
    }
    match report_entry_kind(diagnostic) {
        DiagnosticReportEntryKind::Root => 0,
        DiagnosticReportEntryKind::Related => 1,
        DiagnosticReportEntryKind::Independent => 2,
    }
}

fn compare_report_items<T: DiagnosticReportItem>(left: &T, right: &T) -> Ordering {
    let left_diagnostic = left.report_diagnostic();
    let right_diagnostic = right.report_diagnostic();
    // Category and severity apply to every component, including failures
    // without a source span. Ownership and stage are fallback priorities for
    // unrelated entries; explicit cause edges always take precedence.
    diagnostic_priority(left_diagnostic)
        .cmp(&diagnostic_priority(right_diagnostic))
        .then_with(|| {
            report_entry_kind_rank(left_diagnostic).cmp(&report_entry_kind_rank(right_diagnostic))
        })
        .then_with(|| span_source_rank(left_diagnostic).cmp(&span_source_rank(right_diagnostic)))
        .then_with(|| {
            diagnostic_stage_priority(left_diagnostic)
                .cmp(&diagnostic_stage_priority(right_diagnostic))
        })
        .then_with(|| left.report_source_path().cmp(&right.report_source_path()))
        .then_with(|| {
            left_diagnostic
                .primary_span()
                .unwrap_or_default()
                .start
                .cmp(&right_diagnostic.primary_span().unwrap_or_default().start)
        })
        .then_with(|| {
            left_diagnostic
                .primary_span()
                .unwrap_or_default()
                .end
                .cmp(&right_diagnostic.primary_span().unwrap_or_default().end)
        })
        .then_with(|| {
            left_diagnostic
                .code
                .as_str()
                .cmp(right_diagnostic.code.as_str())
        })
        .then_with(|| left_diagnostic.summary.cmp(&right_diagnostic.summary))
        // Concurrent producers may emit different explanations at the same
        // code and location. Compare structured payloads, never rendered text
        // or hash iteration order, to keep truncation and JSON parents stable.
        .then_with(|| {
            (
                &left_diagnostic.labels,
                &left_diagnostic.notes,
                &left_diagnostic.help,
                &left_diagnostic.suggestions,
                &left_diagnostic.related,
                &left_diagnostic.cause,
                &left_diagnostic.debug,
                &left_diagnostic.source_path,
            )
                .cmp(&(
                    &right_diagnostic.labels,
                    &right_diagnostic.notes,
                    &right_diagnostic.help,
                    &right_diagnostic.suggestions,
                    &right_diagnostic.related,
                    &right_diagnostic.cause,
                    &right_diagnostic.debug,
                    &right_diagnostic.source_path,
                ))
        })
}

fn diagnostic_stage_priority(diagnostic: &Diagnostic) -> u8 {
    use codes::DiagnosticStage;

    match diagnostic.code.def().stage {
        DiagnosticStage::Compiler => 0,
        DiagnosticStage::Parse => 1,
        DiagnosticStage::Load => 2,
        DiagnosticStage::TargetConfig => 3,
        DiagnosticStage::Resolution => 4,
        DiagnosticStage::TypeNormalization => 5,
        DiagnosticStage::ItemSignature => 6,
        DiagnosticStage::LocalResolution => 7,
        DiagnosticStage::TypeCheck => 8,
        DiagnosticStage::BodyIr => 9,
        DiagnosticStage::Const => 10,
        DiagnosticStage::StaticCheck => 11,
        DiagnosticStage::FunctionIr => 12,
        DiagnosticStage::BackendIr => 13,
        DiagnosticStage::Llvm => 14,
        DiagnosticStage::Linker => 15,
    }
}

fn diagnostic_priority(diagnostic: &Diagnostic) -> (u8, u8) {
    let category = match diagnostic.category() {
        DiagnosticCategory::Internal => 0,
        DiagnosticCategory::User => 1,
    };
    let severity = match diagnostic.severity() {
        Severity::Error => 0,
        Severity::Warning => 1,
    };
    (category, severity)
}

fn span_source_rank(diagnostic: &Diagnostic) -> u8 {
    match diagnostic.primary_span_source() {
        Some(SpanSource::Source) => 0,
        Some(SpanSource::Generated) => 1,
        Some(SpanSource::Fallback) => 2,
        None => 3,
    }
}

/// Two entries are one diagnostic when they say the same thing about the
/// same file, whether the file came from the reporting module or from the
/// diagnostic naming it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DiagnosticDedupeKey<'a> {
    path: Option<&'a str>,
    diagnostic: std::borrow::Cow<'a, Diagnostic>,
}

impl<'a> DiagnosticDedupeKey<'a> {
    fn from_item<T: DiagnosticReportItem>(item: &'a T) -> Self {
        let diagnostic = item.report_diagnostic();
        let diagnostic = if diagnostic.source_path.is_some() {
            let mut unlocated = diagnostic.clone();
            unlocated.source_path = None;
            std::borrow::Cow::Owned(unlocated)
        } else {
            std::borrow::Cow::Borrowed(diagnostic)
        };
        Self {
            path: item.report_source_path(),
            diagnostic,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causal_forests_preserve_every_entry_and_retained_parent() {
        // Exhaust all parent assignments for four distinct source identities:
        // four possible parents, no parent, or a missing external parent.
        // This includes every cycle and every chain/component arrangement at
        // this bound. An independent walk supplies the cycle-membership oracle.
        const N: usize = 4;
        for mut assignment in 0..(N + 2).pow(N as u32) {
            let mut parents = [None; N];
            let mut missing = [false; N];
            for index in 0..N {
                let value = assignment % (N + 2);
                assignment /= N + 2;
                parents[index] = (value < N).then_some(value);
                missing[index] = value == N + 1;
            }
            let cyclic = (0..N)
                .map(|start| {
                    let mut current = parents[start];
                    for _ in 0..N {
                        match current {
                            Some(index) if index == start => return true,
                            Some(index) => current = parents[index],
                            None => return false,
                        }
                    }
                    false
                })
                .collect::<Vec<_>>();
            let diagnostics = (0..N)
                .map(|index| {
                    let path = format!("file-{index}.nia");
                    let mut builder = Diagnostic::user_error(codes::TYPE_CHECK, index.to_string())
                        .in_source(path)
                        .primary(Span::new(0, 1), "failure");
                    if let Some(parent) = parents[index] {
                        builder = builder.caused_by(
                            format!("file-{parent}.nia"),
                            codes::TYPE_CHECK.code,
                            Span::new(0, 1),
                        );
                    } else if missing[index] {
                        builder = builder.caused_by(
                            "missing.nia",
                            codes::TYPE_CHECK.code,
                            Span::new(0, 1),
                        );
                    }
                    builder.finish()
                })
                .collect::<Vec<_>>();
            let report = build_diagnostic_report(
                &diagnostics,
                DiagnosticReportConfig { max_diagnostics: N },
            );
            assert_eq!(report.entries().len(), N);
            let order = report
                .entries()
                .iter()
                .map(|entry| entry.summary.clone())
                .collect::<Vec<_>>();
            let reversed = diagnostics.iter().rev().cloned().collect::<Vec<_>>();
            let reverse_report =
                build_diagnostic_report(&reversed, DiagnosticReportConfig { max_diagnostics: N });
            assert_eq!(
                order,
                reverse_report
                    .entries()
                    .iter()
                    .map(|entry| entry.summary.clone())
                    .collect::<Vec<_>>()
            );
            for limit in 0..=N {
                let report = build_diagnostic_report(
                    &diagnostics,
                    DiagnosticReportConfig {
                        max_diagnostics: limit,
                    },
                );
                assert_eq!(report.entries().len(), limit);
                assert_eq!(report.suppressed_by_limit(), N - limit);
                for (position, entry) in report.entries().iter().enumerate() {
                    let index = entry.summary.parse::<usize>().unwrap();
                    if let Some(parent) = parents[index].filter(|_| !cyclic[index]) {
                        assert_eq!(
                            report.entry_kind(position),
                            Some(DiagnosticReportEntryKind::Related)
                        );
                        let parent_position = report
                            .entry_parent(position)
                            .expect("retained causal parent");
                        assert!(parent_position < position);
                        assert_eq!(
                            report.entries()[parent_position].summary,
                            parent.to_string()
                        );
                    } else {
                        let kind = if parents[index].is_some() || missing[index] {
                            DiagnosticReportEntryKind::Independent
                        } else {
                            DiagnosticReportEntryKind::Root
                        };
                        assert_eq!(report.entry_kind(position), Some(kind));
                        assert_eq!(report.entry_parent(position), None);
                    }
                }
            }
        }
    }

    #[test]
    fn tied_locations_use_structured_evidence_for_stable_ordering() {
        let first = Diagnostic::user_error(codes::TYPE_CHECK, "same summary")
            .in_source("main.nia")
            .primary(Span::new(0, 1), "same label")
            .note("first evidence")
            .finish();
        let mut second = first.clone();
        second.notes[0] = "second evidence".to_string();
        let forward = [first.clone(), second.clone()];
        let reverse = [second, first];
        for limit in 0..=2 {
            let config = DiagnosticReportConfig {
                max_diagnostics: limit,
            };
            assert_eq!(
                render_diagnostics_json(&forward, config),
                render_diagnostics_json(&reverse, config),
            );
        }
    }

    #[test]
    fn deep_causal_chains_do_not_recurse_on_the_thread_stack() {
        let count = 20_000;
        let mut diagnostics = Vec::with_capacity(count);
        for index in 0..count {
            let start = index;
            let mut builder = Diagnostic::user_error(codes::TYPE_CHECK, index.to_string())
                .in_source("main.nia")
                .primary(Span::new(start, start + 1), "failure");
            if index > 0 {
                builder = builder.caused_by(
                    "main.nia",
                    codes::TYPE_CHECK.code,
                    Span::new(start - 1, start),
                );
            }
            diagnostics.push(builder.finish());
        }
        let report = build_diagnostic_report(
            &diagnostics,
            DiagnosticReportConfig {
                max_diagnostics: count,
            },
        );
        assert_eq!(report.entries().len(), count);
        assert_eq!(report.entry_parent(count - 1), Some(count - 2));
    }

    #[test]
    fn explicit_ownership_survives_wrappers_rendering_and_persistence() {
        struct Wrapped<'a>(&'a Diagnostic);
        impl DiagnosticReportItem for Wrapped<'_> {
            fn report_diagnostic(&self) -> &Diagnostic {
                self.0
            }
            fn report_path(&self) -> Option<&str> {
                Some("wrapper.nia")
            }
        }
        let root = Diagnostic::user_error(codes::TYPE_CHECK, "source root")
            .in_source("source.nia")
            .primary(Span::new(0, 1), "root")
            .finish();
        let child = Diagnostic::user_error(codes::PARSE, "consequence")
            .in_source("use.nia")
            .primary(Span::new(0, 1), "use")
            .caused_by("source.nia", codes::TYPE_CHECK.code, Span::new(0, 1))
            .finish();
        let items = [Wrapped(&child), Wrapped(&root)];
        let report = build_diagnostic_report(&items, DiagnosticReportConfig::default());
        assert_eq!(report.entries()[0].0.summary, "source root");
        assert_eq!(report.entry_parent(1), Some(0));
        let json = render_diagnostics_json(&items, DiagnosticReportConfig::default());
        assert!(json.contains("\"kind\":\"related\",\"parent\":0"), "{json}");
        assert!(
            json.contains("\"cause\":{\"path\":\"source.nia\""),
            "{json}"
        );
        assert!(!json.contains("wrapper.nia"), "{json}");
        let sources = HashMap::from([("source.nia".to_string(), "x".to_string())]);
        let text = render_diagnostic_with_sources("wrapper.nia", "wrong source", &root, &sources);
        assert!(text.contains("source.nia:1:1"), "{text}");
        assert!(!text.contains("wrong source"), "{text}");
        let missing_text = render_diagnostic("wrapper.nia", "wrong source", &root);
        assert!(missing_text.contains("source.nia:bytes"), "{missing_text}");
        let diagnostics = [root, child];
        let encoded = encode_stable_diagnostic_bundle(&diagnostics, 1).unwrap();
        let restored = decode_stable_diagnostic_bundle(&encoded, 1).unwrap();
        assert_eq!(
            render_diagnostics_json(&diagnostics, DiagnosticReportConfig::default()),
            render_diagnostics_json(&restored, DiagnosticReportConfig::default())
        );
    }
}
