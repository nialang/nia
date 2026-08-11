// SPDX-License-Identifier: GPL-3.0-or-later
use std::{collections::BTreeMap, fmt, io::Cursor};

use nia_compat::formats::STABLE_PROGRAM_DIAGNOSTIC_BUNDLE;
use nia_diagnostic::{
    StableDiagnosticBundleError, decode_stable_diagnostic_bundle, encode_stable_diagnostic_bundle,
};
use nia_source::SourcePath;

use crate::ProgramDiagnostic;

const MAX_STORE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEQUENCE_LEN: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StableProgramDiagnosticBundleError {
    UnknownSource,
    TooLarge,
    Diagnostic(StableDiagnosticBundleError),
}

impl fmt::Display for StableProgramDiagnosticBundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSource => {
                formatter.write_str("stable program diagnostics require a current source owner")
            }
            Self::TooLarge => formatter.write_str("stable program diagnostic bundle is too large"),
            Self::Diagnostic(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StableProgramDiagnosticBundleError {}

pub(crate) fn encode_stable_program_diagnostic_bundle(
    diagnostics: &[ProgramDiagnostic],
    source_lengths: &BTreeMap<String, usize>,
) -> Result<Vec<u8>, StableProgramDiagnosticBundleError> {
    let mut used_sources = BTreeMap::new();
    for diagnostic in diagnostics {
        let path = diagnostic.path.as_str();
        let source_len = source_lengths
            .get(path)
            .copied()
            .ok_or(StableProgramDiagnosticBundleError::UnknownSource)?;
        used_sources.insert(path.to_owned(), source_len);
    }

    let mut encoded = STABLE_PROGRAM_DIAGNOSTIC_BUNDLE.magic.to_vec();
    write_len(&mut encoded, used_sources.len())?;
    let mut source_indices = BTreeMap::new();
    for (index, (path, source_len)) in used_sources.iter().enumerate() {
        source_indices.insert(path.as_str(), index);
        write_string(&mut encoded, path)?;
        write_u64(&mut encoded, *source_len as u64)?;
    }

    write_len(&mut encoded, diagnostics.len())?;
    for diagnostic in diagnostics {
        let path = diagnostic.path.as_str();
        let source_index = source_indices
            .get(path)
            .copied()
            .ok_or(StableProgramDiagnosticBundleError::UnknownSource)?;
        let source_len = used_sources[path];
        let bundle = encode_stable_diagnostic_bundle(
            std::slice::from_ref(&diagnostic.diagnostic),
            source_len,
        )
        .map_err(StableProgramDiagnosticBundleError::Diagnostic)?;
        write_u64(&mut encoded, source_index as u64)?;
        write_len(&mut encoded, bundle.len())?;
        extend(&mut encoded, &bundle)?;
    }
    Ok(encoded)
}

pub(crate) fn decode_stable_program_diagnostic_bundle(
    encoded: &[u8],
    source_lengths: &BTreeMap<String, usize>,
) -> Option<Vec<ProgramDiagnostic>> {
    if encoded.len() > MAX_STORE_BYTES
        || !encoded.starts_with(STABLE_PROGRAM_DIAGNOSTIC_BUNDLE.magic)
    {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[STABLE_PROGRAM_DIAGNOSTIC_BUNDLE.magic.len()..]);
    let source_count = read_len(&mut cursor)?;
    let mut sources = Vec::with_capacity(source_count);
    let mut previous_path: Option<String> = None;
    for _ in 0..source_count {
        let path = read_string(&mut cursor)?;
        if SourcePath::new(&path).as_str() != path
            || previous_path
                .as_ref()
                .is_some_and(|previous| previous >= &path)
        {
            return None;
        }
        let source_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
        if source_lengths.get(&path).copied() != Some(source_len) {
            return None;
        }
        previous_path = Some(path.clone());
        sources.push((SourcePath::from_normalized_unchecked(path), source_len));
    }

    let diagnostic_count = read_len(&mut cursor)?;
    let mut diagnostics = Vec::with_capacity(diagnostic_count);
    for _ in 0..diagnostic_count {
        let source_index = usize::try_from(read_u64(&mut cursor)?).ok()?;
        let (path, source_len) = sources.get(source_index)?;
        let bundle_len = read_len(&mut cursor)?;
        let start = usize::try_from(cursor.position()).ok()?;
        let end = start.checked_add(bundle_len)?;
        let bundle = cursor.get_ref().get(start..end)?;
        cursor.set_position(u64::try_from(end).ok()?);
        let mut decoded = decode_stable_diagnostic_bundle(bundle, *source_len)?;
        if decoded.len() != 1 {
            return None;
        }
        diagnostics.push(ProgramDiagnostic {
            path: path.clone(),
            diagnostic: decoded.pop()?,
        });
    }
    (usize::try_from(cursor.position()).ok()? + STABLE_PROGRAM_DIAGNOSTIC_BUNDLE.magic.len()
        == encoded.len())
    .then_some(diagnostics)
}

fn write_string(
    encoded: &mut Vec<u8>,
    value: &str,
) -> Result<(), StableProgramDiagnosticBundleError> {
    write_len(encoded, value.len())?;
    extend(encoded, value.as_bytes())
}

fn read_string(cursor: &mut Cursor<&[u8]>) -> Option<String> {
    let len = read_len(cursor)?;
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(len)?;
    let value = cursor.get_ref().get(start..end)?;
    cursor.set_position(u64::try_from(end).ok()?);
    String::from_utf8(value.to_vec()).ok()
}

fn write_len(encoded: &mut Vec<u8>, len: usize) -> Result<(), StableProgramDiagnosticBundleError> {
    if len > MAX_SEQUENCE_LEN {
        return Err(StableProgramDiagnosticBundleError::TooLarge);
    }
    write_u64(encoded, len as u64)
}

fn read_len(cursor: &mut Cursor<&[u8]>) -> Option<usize> {
    let len = usize::try_from(read_u64(cursor)?).ok()?;
    let position = usize::try_from(cursor.position()).ok()?;
    let remaining = cursor.get_ref().len().checked_sub(position)?;
    (len <= MAX_SEQUENCE_LEN && len <= remaining).then_some(len)
}

fn write_u64(encoded: &mut Vec<u8>, value: u64) -> Result<(), StableProgramDiagnosticBundleError> {
    extend(encoded, &value.to_le_bytes())
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(8)?;
    let bytes: [u8; 8] = cursor.get_ref().get(start..end)?.try_into().ok()?;
    cursor.set_position(u64::try_from(end).ok()?);
    Some(u64::from_le_bytes(bytes))
}

fn extend(encoded: &mut Vec<u8>, bytes: &[u8]) -> Result<(), StableProgramDiagnosticBundleError> {
    if encoded.len().saturating_add(bytes.len()) > MAX_STORE_BYTES {
        return Err(StableProgramDiagnosticBundleError::TooLarge);
    }
    encoded.extend_from_slice(bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use nia_diagnostic::{Diagnostic, SpanSource, codes};
    use nia_span::Span;

    use super::*;

    #[test]
    fn program_bundle_roundtrips_multiple_sources_in_report_order() {
        let mut first = Diagnostic::user_warning(codes::UNUSED_IMPORT, "unused import")
            .primary(Span::new(3, 7), "unused here")
            .secondary_fallback(Span::new(0, 2), "context")
            .note("note")
            .help("help")
            .related(Span::new(8, 9), "related")
            .debug("owner", 3)
            .finish();
        first.labels[1].span_source = SpanSource::Generated;
        let diagnostics = vec![
            ProgramDiagnostic {
                path: SourcePath::new("src/main.nia"),
                diagnostic: first,
            },
            ProgramDiagnostic {
                path: SourcePath::new("src/lib.nia"),
                diagnostic: Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    Span::new(1, 4),
                    "bad type",
                ),
            },
        ];
        let source_lengths = BTreeMap::from([
            ("src/lib.nia".to_owned(), 12),
            ("src/main.nia".to_owned(), 16),
        ]);

        let encoded = encode_stable_program_diagnostic_bundle(&diagnostics, &source_lengths)
            .expect("encode program diagnostics");
        assert_eq!(
            decode_stable_program_diagnostic_bundle(&encoded, &source_lengths),
            Some(diagnostics)
        );
    }

    #[test]
    fn program_bundle_rejects_unknown_or_changed_source_owner() {
        let diagnostics = vec![ProgramDiagnostic {
            path: SourcePath::new("src/main.nia"),
            diagnostic: Diagnostic::user_error_at(codes::TYPE_CHECK, Span::new(1, 4), "bad type"),
        }];
        assert_eq!(
            encode_stable_program_diagnostic_bundle(&diagnostics, &BTreeMap::new()),
            Err(StableProgramDiagnosticBundleError::UnknownSource)
        );

        let source_lengths = BTreeMap::from([("src/main.nia".to_owned(), 8)]);
        let encoded = encode_stable_program_diagnostic_bundle(&diagnostics, &source_lengths)
            .expect("encode program diagnostics");
        let changed_lengths = BTreeMap::from([("src/main.nia".to_owned(), 9)]);
        assert_eq!(
            decode_stable_program_diagnostic_bundle(&encoded, &changed_lengths),
            None
        );
    }
}
