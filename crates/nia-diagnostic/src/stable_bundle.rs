// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fmt, io::Cursor};

use nia_span::Span;

use crate::{
    DebugField, Diagnostic, DiagnosticCode, DiagnosticLabel, LabelStyle, RelatedDiagnostic,
    SpanSource, codes,
};

const MAGIC: &[u8; 8] = b"NIADB001";
const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEQUENCE_LEN: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StableDiagnosticBundleError {
    UnregisteredCode,
    InconsistentCode,
    InvalidSpan,
    TooLarge,
}

impl fmt::Display for StableDiagnosticBundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnregisteredCode => "stable diagnostics require a registered code",
            Self::InconsistentCode => "diagnostic severity or category does not match its code",
            Self::InvalidSpan => "diagnostic span is outside its source",
            Self::TooLarge => "stable diagnostic bundle exceeds its size limit",
        })
    }
}

impl std::error::Error for StableDiagnosticBundleError {}

pub fn encode_stable_diagnostic_bundle(
    diagnostics: &[Diagnostic],
    source_len: usize,
) -> Result<Vec<u8>, StableDiagnosticBundleError> {
    let mut encoded = MAGIC.to_vec();
    write_len(&mut encoded, diagnostics.len())?;
    for diagnostic in diagnostics {
        let code = codes::ALL
            .iter()
            .find(|code| code.code == diagnostic.code.as_str())
            .copied()
            .ok_or(StableDiagnosticBundleError::UnregisteredCode)?;
        if !diagnostic.code.is_registered() {
            return Err(StableDiagnosticBundleError::UnregisteredCode);
        }
        if diagnostic.severity != code.severity || diagnostic.category != code.category {
            return Err(StableDiagnosticBundleError::InconsistentCode);
        }
        write_string(&mut encoded, code.code)?;
        write_string(&mut encoded, &diagnostic.summary)?;
        write_len(&mut encoded, diagnostic.labels.len())?;
        for label in diagnostic.labels.iter() {
            write_span(&mut encoded, label.span, source_len)?;
            encoded.push(match label.span_source {
                SpanSource::Source => 0,
                SpanSource::Fallback => 1,
                SpanSource::Generated => 2,
            });
            encoded.push(match label.style {
                LabelStyle::Primary => 0,
                LabelStyle::Secondary => 1,
            });
            write_optional_string(&mut encoded, label.message.as_deref())?;
        }
        write_strings(&mut encoded, &diagnostic.notes)?;
        write_strings(&mut encoded, &diagnostic.help)?;
        write_len(&mut encoded, diagnostic.related.len())?;
        for related in diagnostic.related.iter() {
            write_span(&mut encoded, related.span, source_len)?;
            write_string(&mut encoded, &related.message)?;
        }
        write_len(&mut encoded, diagnostic.debug.len())?;
        for field in diagnostic.debug.iter() {
            write_string(&mut encoded, &field.key)?;
            write_string(&mut encoded, &field.value)?;
        }
    }
    if encoded.len() > MAX_BUNDLE_BYTES {
        return Err(StableDiagnosticBundleError::TooLarge);
    }
    Ok(encoded)
}

pub fn decode_stable_diagnostic_bundle(
    encoded: &[u8],
    source_len: usize,
) -> Option<Vec<Diagnostic>> {
    if encoded.len() > MAX_BUNDLE_BYTES || !encoded.starts_with(MAGIC) {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[MAGIC.len()..]);
    let diagnostics_len = read_len(&mut cursor)?;
    let mut diagnostics = Vec::with_capacity(diagnostics_len);
    for _ in 0..diagnostics_len {
        let code_text = read_string(&mut cursor)?;
        let code = codes::ALL.iter().find(|code| code.code == code_text)?;
        let summary = read_string(&mut cursor)?;
        let labels_len = read_len(&mut cursor)?;
        let mut labels = Vec::with_capacity(labels_len);
        for _ in 0..labels_len {
            let span = read_span(&mut cursor, source_len)?;
            let span_source = match read_u8(&mut cursor)? {
                0 => SpanSource::Source,
                1 => SpanSource::Fallback,
                2 => SpanSource::Generated,
                _ => return None,
            };
            let style = match read_u8(&mut cursor)? {
                0 => LabelStyle::Primary,
                1 => LabelStyle::Secondary,
                _ => return None,
            };
            labels.push(DiagnosticLabel {
                span,
                span_source,
                style,
                message: read_optional_string(&mut cursor)?,
            });
        }
        let notes = read_strings(&mut cursor)?;
        let help = read_strings(&mut cursor)?;
        let related_len = read_len(&mut cursor)?;
        let mut related = Vec::with_capacity(related_len);
        for _ in 0..related_len {
            related.push(RelatedDiagnostic {
                span: read_span(&mut cursor, source_len)?,
                message: read_string(&mut cursor)?,
            });
        }
        let debug_len = read_len(&mut cursor)?;
        let mut debug = Vec::with_capacity(debug_len);
        for _ in 0..debug_len {
            debug.push(DebugField {
                key: read_string(&mut cursor)?,
                value: read_string(&mut cursor)?,
            });
        }
        diagnostics.push(Diagnostic {
            severity: code.severity,
            category: code.category,
            code: DiagnosticCode::registered(*code),
            summary,
            labels: Box::new(labels),
            notes: Box::new(notes),
            help: Box::new(help),
            related: Box::new(related),
            debug: Box::new(debug),
        });
    }
    (usize::try_from(cursor.position()).ok()? + MAGIC.len() == encoded.len()).then_some(diagnostics)
}

fn write_span(
    encoded: &mut Vec<u8>,
    span: Span,
    source_len: usize,
) -> Result<(), StableDiagnosticBundleError> {
    if span.start > span.end || span.end > source_len {
        return Err(StableDiagnosticBundleError::InvalidSpan);
    }
    write_u64(encoded, span.start as u64);
    write_u64(encoded, span.end as u64);
    Ok(())
}

fn read_span(cursor: &mut Cursor<&[u8]>, source_len: usize) -> Option<Span> {
    let start = usize::try_from(read_u64(cursor)?).ok()?;
    let end = usize::try_from(read_u64(cursor)?).ok()?;
    (start <= end && end <= source_len).then_some(Span::new(start, end))
}

fn write_strings(
    encoded: &mut Vec<u8>,
    strings: &[String],
) -> Result<(), StableDiagnosticBundleError> {
    write_len(encoded, strings.len())?;
    for string in strings {
        write_string(encoded, string)?;
    }
    Ok(())
}

fn read_strings(cursor: &mut Cursor<&[u8]>) -> Option<Vec<String>> {
    let len = read_len(cursor)?;
    (0..len).map(|_| read_string(cursor)).collect()
}

fn write_optional_string(
    encoded: &mut Vec<u8>,
    value: Option<&str>,
) -> Result<(), StableDiagnosticBundleError> {
    encoded.push(u8::from(value.is_some()));
    if let Some(value) = value {
        write_string(encoded, value)?;
    }
    Ok(())
}

fn read_optional_string(cursor: &mut Cursor<&[u8]>) -> Option<Option<String>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => read_string(cursor).map(Some),
        _ => None,
    }
}

fn write_string(encoded: &mut Vec<u8>, value: &str) -> Result<(), StableDiagnosticBundleError> {
    write_len(encoded, value.len())?;
    encoded.extend_from_slice(value.as_bytes());
    Ok(())
}

fn read_string(cursor: &mut Cursor<&[u8]>) -> Option<String> {
    let len = read_len(cursor)?;
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(len)?;
    let value = cursor.get_ref().get(start..end)?;
    cursor.set_position(u64::try_from(end).ok()?);
    String::from_utf8(value.to_vec()).ok()
}

fn write_len(encoded: &mut Vec<u8>, len: usize) -> Result<(), StableDiagnosticBundleError> {
    if len > MAX_SEQUENCE_LEN {
        return Err(StableDiagnosticBundleError::TooLarge);
    }
    write_u64(encoded, len as u64);
    Ok(())
}

fn read_len(cursor: &mut Cursor<&[u8]>) -> Option<usize> {
    let len = usize::try_from(read_u64(cursor)?).ok()?;
    let position = usize::try_from(cursor.position()).ok()?;
    let remaining = cursor.get_ref().len().checked_sub(position)?;
    (len <= MAX_SEQUENCE_LEN && len <= remaining).then_some(len)
}

fn write_u64(encoded: &mut Vec<u8>, value: u64) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(8)?;
    let bytes: [u8; 8] = cursor.get_ref().get(start..end)?.try_into().ok()?;
    cursor.set_position(u64::try_from(end).ok()?);
    Some(u64::from_le_bytes(bytes))
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Option<u8> {
    let position = usize::try_from(cursor.position()).ok()?;
    let value = *cursor.get_ref().get(position)?;
    cursor.set_position(u64::try_from(position.checked_add(1)?).ok()?);
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_bundle_roundtrips_complete_diagnostics() {
        let mut diagnostic = Diagnostic::user_warning(codes::UNUSED_IMPORT, "unused import")
            .primary(Span::new(2, 5), "unused here")
            .secondary_fallback(Span::new(0, 1), "fallback context")
            .note("note")
            .help("help")
            .related(Span::new(6, 8), "related")
            .debug("owner", 7)
            .finish();
        diagnostic.labels[1].span_source = SpanSource::Generated;
        let diagnostics = vec![diagnostic];

        let encoded = encode_stable_diagnostic_bundle(&diagnostics, 16).expect("encode bundle");
        assert_eq!(
            decode_stable_diagnostic_bundle(&encoded, 16),
            Some(diagnostics)
        );
    }

    #[test]
    fn stable_bundle_rejects_invalid_spans_and_corruption() {
        let diagnostic = Diagnostic::user_error_at(codes::TYPE_CHECK, Span::new(2, 9), "bad type");
        assert_eq!(
            encode_stable_diagnostic_bundle(&[diagnostic], 8),
            Err(StableDiagnosticBundleError::InvalidSpan)
        );

        let mut encoded = encode_stable_diagnostic_bundle(&[], 0).expect("encode empty bundle");
        encoded.push(0);
        assert_eq!(decode_stable_diagnostic_bundle(&encoded, 0), None);
    }
}
