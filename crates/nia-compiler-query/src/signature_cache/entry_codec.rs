// SPDX-License-Identifier: GPL-3.0-or-later
//! Versioned cache entry envelopes and checksum validation.

use super::*;

pub(crate) fn encode_check_certificate(
    identity: CheckCertificateIdentity<'_>,
    certificate: &CachedCheckCertificate,
) -> io::Result<Vec<u8>> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(CHECK_CERTIFICATE.magic);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.input.parts());
    write_string(
        &mut encoded,
        identity.entry.source_identity().normalized_path(),
    );
    encoded.push(identity.scope.tag());
    write_u64(&mut encoded, certificate.checked_body_count as u64);
    write_u64(&mut encoded, certificate.reachable_body_count as u64);
    let diagnostics =
        encode_stable_program_diagnostic_bundle(&certificate.diagnostics, identity.source_lengths)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_u64(&mut encoded, diagnostics.len() as u64);
    encoded.extend_from_slice(&diagnostics);
    if encoded.len().saturating_add(16) > MAX_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "check certificate exceeds its size limit",
        ));
    }
    let checksum = checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    Ok(encoded)
}

pub(crate) fn decode_check_certificate(
    encoded: &[u8],
    identity: CheckCertificateIdentity<'_>,
) -> Option<CachedCheckCertificate> {
    if encoded.len() < CHECK_CERTIFICATE.magic.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    // The trailing checksum authenticates the complete identity and payload. Validate it before
    // parsing lengths so corrupted bytes cannot steer deeper decoding.
    let expected_checksum = checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != CHECK_CERTIFICATE.magic
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.input.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.entry.source_identity().normalized_path()
        || read_u8(&mut cursor)? != identity.scope.tag()
    {
        return None;
    }
    let checked_body_count = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let reachable_body_count = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let diagnostics_len = read_len(&mut cursor, encoded.len())?;
    let diagnostics_start = usize::try_from(cursor.position()).ok()?;
    let diagnostics_end = diagnostics_start.checked_add(diagnostics_len)?;
    let diagnostics = decode_stable_program_diagnostic_bundle(
        cursor.get_ref().get(diagnostics_start..diagnostics_end)?,
        identity.source_lengths,
    )?;
    cursor.set_position(u64::try_from(diagnostics_end).ok()?);
    (usize::try_from(cursor.position()).ok()? == checksum_offset).then_some(
        CachedCheckCertificate {
            checked_body_count,
            reachable_body_count,
            diagnostics,
        },
    )
}

pub(crate) fn encode_entry(
    identity: SignatureTypeResolutionIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(SIGNATURE_TYPE_RESOLUTION.magic);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    encoded.push(signature_set_tag(identity.set));
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

pub(crate) fn decode_entry<'a>(
    encoded: &'a [u8],
    identity: SignatureTypeResolutionIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < SIGNATURE_TYPE_RESOLUTION.magic.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != SIGNATURE_TYPE_RESOLUTION.magic
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || read_u8(&mut cursor)? != signature_set_tag(identity.set)
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

pub(crate) fn encode_type_lowering_entry(
    identity: SignatureTypeLoweringIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(SIGNATURE_TYPE_LOWERING.magic);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    encoded.push(signature_set_tag(identity.set));
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = type_lowering_checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

pub(crate) fn decode_type_lowering_entry<'a>(
    encoded: &'a [u8],
    identity: SignatureTypeLoweringIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < SIGNATURE_TYPE_LOWERING.magic.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = type_lowering_checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != SIGNATURE_TYPE_LOWERING.magic
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || read_u8(&mut cursor)? != signature_set_tag(identity.set)
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

pub(crate) fn encode_item_signatures_entry(
    identity: SignatureItemSignaturesIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(SIGNATURE_ITEM_SIGNATURES.magic);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    encoded.push(signature_set_tag(identity.set));
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = item_signatures_checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

pub(crate) fn decode_item_signatures_entry<'a>(
    encoded: &'a [u8],
    identity: SignatureItemSignaturesIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < SIGNATURE_ITEM_SIGNATURES.magic.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = item_signatures_checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != SIGNATURE_ITEM_SIGNATURES.magic
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || read_u8(&mut cursor)? != signature_set_tag(identity.set)
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

pub(crate) fn encode_extension_validation_diagnostics_entry(
    identity: ExtensionValidationDiagnosticsIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(EXTENSION_VALIDATION_DIAGNOSTICS.magic);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = extension_validation_diagnostics_checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

pub(crate) fn decode_extension_validation_diagnostics_entry<'a>(
    encoded: &'a [u8],
    identity: ExtensionValidationDiagnosticsIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < EXTENSION_VALIDATION_DIAGNOSTICS.magic.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = extension_validation_diagnostics_checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != EXTENSION_VALIDATION_DIAGNOSTICS.magic
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

pub(crate) fn encode_executable_value_ref_edges_entry(
    identity: ExecutableValueRefEdgesIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(EXECUTABLE_VALUE_REF_EDGES.magic);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    write_u64(&mut encoded, identity.owner.0);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = executable_value_ref_edges_checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

pub(crate) fn decode_executable_value_ref_edges_entry<'a>(
    encoded: &'a [u8],
    identity: ExecutableValueRefEdgesIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < EXECUTABLE_VALUE_REF_EDGES.magic.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = executable_value_ref_edges_checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != EXECUTABLE_VALUE_REF_EDGES.magic
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || DefId(read_u64(&mut cursor)?) != identity.owner
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}
