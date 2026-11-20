// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::{Arc, Mutex, OnceLock};

use lsp_types::{
    DocumentChanges, OneOf, Position, Range, TextDocumentContentChangeEvent, TextDocumentEdit,
    WorkspaceEdit,
};
use nia_span::Span;

pub(crate) fn text_workspace_edit(edits: Vec<TextDocumentEdit>, versioned: bool) -> WorkspaceEdit {
    if versioned {
        return WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(edits)),
            ..WorkspaceEdit::default()
        };
    }
    WorkspaceEdit::new(
        edits
            .into_iter()
            .map(|document| {
                let edits = document.edits.into_iter().map(|edit| match edit {
                    OneOf::Left(edit) => edit,
                    OneOf::Right(edit) => edit.text_edit,
                });
                (document.text_document.uri, edits.collect())
            })
            .collect(),
    )
}

#[derive(Debug, Clone)]
pub(crate) struct Document {
    pub(crate) version: i32,
    pub(crate) text: Arc<str>,
    pub(crate) lines: Lines,
    syntax: Arc<DocumentSyntax>,
    pub(crate) highlights: Arc<OnceLock<Vec<crate::highlight::Highlight>>>,
}

#[derive(Debug, Default)]
struct DocumentSyntax {
    parsed: OnceLock<Option<Arc<nia_syntax::Parse>>>,
    previous: Mutex<Option<Arc<nia_syntax::Parse>>>,
}

impl Document {
    pub(crate) fn new(version: i32, text: String) -> Self {
        Self {
            version,
            lines: Lines::new(&text),
            text: text.into(),
            syntax: Arc::default(),
            highlights: Arc::default(),
        }
    }

    pub(crate) fn syntax(&self) -> Option<&nia_syntax::Parse> {
        self.syntax
            .parsed
            .get_or_init(|| {
                let previous = self
                    .syntax
                    .previous
                    .lock()
                    .expect("syntax seed lock")
                    .take();
                if let Some(previous) = previous {
                    let old = previous.tree.source();
                    let edit = nia_syntax::TextEdit::between(old, &self.text);
                    if edit.span.len() < old.len()
                        && let Ok(parsed) = previous.reparse(&edit, None)
                    {
                        return Some(Arc::new(parsed));
                    }
                }
                nia_syntax::parse(&self.text, None).ok().map(Arc::new)
            })
            .as_deref()
    }

    pub(crate) fn format(
        &self,
        options: lsp_types::FormattingOptions,
        range: Option<Range>,
    ) -> Result<Option<Vec<lsp_types::TextEdit>>, String> {
        let indentation = if options.insert_spaces {
            let width = u8::try_from(options.tab_size)
                .ok()
                .and_then(std::num::NonZeroU8::new)
                .ok_or("Format indentation must use a tab width from 1 to 255")?;
            nia_syntax::Indentation::Spaces(width)
        } else {
            nia_syntax::Indentation::Tabs
        };
        let range = if let Some(range) = range {
            let (Some(start), Some(end)) = (
                self.lines.offset(&self.text, range.start),
                self.lines.offset(&self.text, range.end),
            ) else {
                return Err("Formatting range is outside the document".into());
            };
            if range.start > range.end {
                return Err("Formatting range is reversed".into());
            }
            if start == end {
                return Ok(Some(Vec::new()));
            }
            let last_line = self.lines.line_index(end);
            let end = if self.lines.line_start(last_line) == end {
                end
            } else {
                self.lines
                    .next_line_start(last_line)
                    .unwrap_or(self.text.len())
            };
            Some(Span::new(
                self.lines.line_start(self.lines.line_index(start)),
                end,
            ))
        } else {
            None
        };
        let parsed = self
            .syntax()
            .ok_or("Could not load the source syntax tree")?;
        match nia_syntax::format_parsed(parsed, indentation, range) {
            Ok(formatted) => Ok(Some(
                formatted
                    .edits
                    .into_iter()
                    .map(|edit| {
                        lsp_types::TextEdit::new(
                            self.lines.range(&self.text, edit.span),
                            edit.replacement,
                        )
                    })
                    .collect(),
            )),
            Err(nia_syntax::FormatError::Syntax(_)) => Ok(None),
            Err(nia_syntax::FormatError::InvalidTree) => {
                Err("Formatting could not preserve the source syntax tree".into())
            }
        }
    }

    pub(crate) fn changed(
        &self,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Option<Self> {
        if version <= self.version {
            return None;
        }
        let mut text = self.text.to_string();
        let mut lines = self.lines.clone();
        for change in changes {
            if let Some(range) = change.range {
                let start = lines.offset(&text, range.start)?;
                let end = lines.offset(&text, range.end)?;
                text.get(start..end)?;
                text.replace_range(start..end, &change.text);
            } else {
                text = change.text;
            }
            lines = Lines::new(&text);
        }
        if text == *self.text {
            return Some(Self {
                version,
                ..self.clone()
            });
        }
        // Retain only the already materialized parser result. A run of edits
        // without syntax requests never builds a chain of old documents.
        let previous = self.syntax.parsed.get().and_then(|parsed| parsed.clone());
        Some(Self {
            version,
            text: text.into(),
            lines,
            syntax: Arc::new(DocumentSyntax {
                parsed: OnceLock::new(),
                previous: Mutex::new(previous),
            }),
            highlights: Arc::default(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Lines(Vec<usize>);

impl Lines {
    pub(crate) fn new(text: &str) -> Self {
        let mut starts = vec![0];
        let mut bytes = text.bytes().enumerate().peekable();
        while let Some((index, byte)) = bytes.next() {
            match byte {
                b'\r' if matches!(bytes.peek(), Some((_, b'\n'))) => {}
                b'\n' | b'\r' => starts.push(index + 1),
                _ => {}
            }
        }
        Self(starts)
    }

    pub(crate) fn offset(&self, text: &str, position: Position) -> Option<usize> {
        let line = position.line as usize;
        let start = *self.0.get(line)?;
        let end = self.0.get(line + 1).copied().unwrap_or(text.len());
        let line_text = text[start..end].trim_end_matches(['\r', '\n']);
        let target = position.character as usize;
        let mut column = 0;
        for (index, ch) in line_text.char_indices() {
            if column >= target {
                return (column == target).then_some(start + index);
            }
            column += ch.len_utf16();
        }
        (column <= target).then_some(start + line_text.len())
    }

    pub(crate) fn line_index(&self, offset: usize) -> usize {
        self.0.partition_point(|start| *start <= offset) - 1
    }

    pub(crate) fn line_start(&self, line: usize) -> usize {
        self.0[line]
    }

    pub(crate) fn next_line_start(&self, line: usize) -> Option<usize> {
        self.0.get(line + 1).copied()
    }

    pub(crate) fn position(&self, text: &str, offset: usize) -> Position {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.line_index(offset);
        let prefix = text[self.0[line]..offset].trim_end_matches(['\r', '\n']);
        Position::new(line as u32, prefix.encode_utf16().count() as u32)
    }

    pub(crate) fn range(&self, text: &str, span: Span) -> Range {
        Range::new(
            self.position(text, span.start),
            self.position(text, span.end),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_formatting_preserves_lone_carriage_returns() {
        let document = Document::new(1, "fun add() i32 {\r1 + 2\r}\r".to_owned());
        let format = lsp_types::FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..Default::default()
        };
        let from = Position::new(1, 0);
        let until = Position::new(2, 0);
        assert_eq!(
            document
                .format(format, Some(Range::new(from, until)))
                .unwrap(),
            Some(vec![lsp_types::TextEdit::new(
                Range::new(from, from),
                "  ".to_owned()
            )]),
        );
    }

    #[test]
    fn applies_utf16_edits_in_order_and_preserves_unedited_lines() {
        let document = Document::new(7, "a😀b\r\nlast\rnext".to_owned());
        let replace = |range, text: &str| TextDocumentContentChangeEvent {
            range: Some(range),
            range_length: None,
            text: text.to_owned(),
        };
        let changes = vec![
            replace(Range::new(Position::new(0, 1), Position::new(0, 3)), "x"),
            replace(Range::new(Position::new(0, 2), Position::new(0, 3)), "y"),
            replace(Range::new(Position::new(2, 0), Position::new(2, 4)), "end"),
        ];
        let changed = document.changed(8, changes).unwrap();
        assert_eq!(&*changed.text, "axy\r\nlast\rend");
        assert_eq!(
            document.lines.offset(&document.text, Position::new(0, 2)),
            None
        );
        assert!(document.changed(7, vec![]).is_none());
    }

    #[test]
    fn edited_syntax_features_match_clean_parses() {
        let original =
            "fun first() i32 {\n  1\n}\nfun second() i32 {\n  _ = \"\u{1f34e}\";\n  2\n}\n";
        let mut current = Document::new(1, original.to_owned());
        current.syntax().unwrap();
        let change = |range, text: &str| TextDocumentContentChangeEvent {
            range,
            range_length: None,
            text: text.to_owned(),
        };
        let at = |line, start, end| {
            Some(Range::new(
                Position::new(line, start),
                Position::new(line, end),
            ))
        };
        let revisions = [
            vec![
                change(at(3, 0, 0), "// comment\n"),
                change(at(5, 7, 9), "x"),
            ],
            vec![change(at(6, 2, 3), "call(")],
            vec![change(None, &original.replace("  2", "  30"))],
        ];
        for (i, changes) in revisions.into_iter().enumerate() {
            current = current.changed(i as i32 + 2, changes).unwrap();
            assert!(current.syntax.previous.lock().unwrap().is_some());
            assert!(current.syntax.parsed.get().is_none());
            let clean = Document::new(current.version, current.text.to_string());
            assert_eq!(current.syntax().unwrap(), clean.syntax().unwrap());
            assert!(current.syntax.previous.lock().unwrap().is_none());
            assert_eq!(
                current.semantic_tokens(None, None).data,
                clean.semantic_tokens(None, None).data
            );
            assert_eq!(
                current.folding_ranges(false, None),
                clean.folding_ranges(false, None)
            );
            let options = lsp_types::FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            };
            assert_eq!(
                current.format(options.clone(), None),
                clean.format(options, None)
            );
        }
        let unchanged = current.changed(5, Vec::new()).unwrap();
        assert!(Arc::ptr_eq(&current.syntax, &unchanged.syntax));
    }
}
