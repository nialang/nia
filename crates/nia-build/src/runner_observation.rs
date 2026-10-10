// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded observations reported by the Nia build runner.

use std::{
    fs,
    io::{self, Read},
    path::Path,
};

use nia_compat::formats::RUNNER_OBSERVATIONS;

use crate::{
    ConfigureManifest, ConfigureObservation, ObservationKind, ObservationPolicy,
    configure_observation::{
        MAX_DIRECTORY_BYTES, MAX_FILE_CONTENT_BYTES, directory_observation, read_directory_facts,
        validate_probe_result,
    },
};

const MAGIC: &[u8; 8] = RUNNER_OBSERVATIONS.magic;
const MAX_EVIDENCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 16 * 1024;
const MAX_ENV_BYTES: usize = 32 * 1024;

/// Decodes runner facts and verifies that their source values remain current.
///
/// Each record is `(kind:u8, path_len:u32, value_len:u32, path, value)`. Kinds
/// are file contents (0), file presence (1), directory contents (2), directory
/// presence (3), and environment value (4). Presence values are one byte: 0
/// absent, 1 file, 2 directory, 3 another entry kind. Environment values are
/// `[0]` for absent and `[1, utf8...]` for present.
pub(crate) fn decode_and_validate_runner_observations(
    root: &Path,
    encoded: &[u8],
) -> io::Result<ConfigureManifest> {
    if encoded.len() < 16 || encoded.len() > MAX_EVIDENCE_BYTES || &encoded[..8] != MAGIC {
        return Err(invalid_evidence());
    }
    if u32::from_le_bytes(encoded[8..12].try_into().unwrap()) != RUNNER_OBSERVATIONS.protocol_epoch
    {
        return Err(invalid_evidence());
    }

    let count = u32::from_le_bytes(encoded[12..16].try_into().unwrap()) as usize;
    if count > MAX_RECORDS {
        return Err(invalid_evidence());
    }
    let mut cursor = 16usize;
    let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
    for _ in 0..count {
        let kind = *encoded.get(cursor).ok_or_else(invalid_evidence)?;
        cursor += 1;
        let path_len = read_u32(encoded, &mut cursor)? as usize;
        let value_len = read_u32(encoded, &mut cursor)? as usize;
        let path_end = cursor.checked_add(path_len).ok_or_else(invalid_evidence)?;
        let value_end = path_end
            .checked_add(value_len)
            .ok_or_else(invalid_evidence)?;
        if value_end > encoded.len() {
            return Err(invalid_evidence());
        }
        let path =
            std::str::from_utf8(&encoded[cursor..path_end]).map_err(|_| invalid_evidence())?;
        let value = &encoded[path_end..value_end];
        cursor = value_end;

        let observation = match kind {
            0 => validate_file_contents(root, path, value)?,
            1 => validate_presence(root, path, value, ObservationKind::FilePresence)?,
            2 => validate_directory_contents(root, path, value)?,
            3 => validate_presence(root, path, value, ObservationKind::DirectoryPresence)?,
            4 => validate_environment(path, value)?,
            5 => validate_probe_result(ObservationKind::CommandResult, path, value, root)?,
            6 => validate_probe_result(ObservationKind::GitState, path, value, root)?,
            7 => validate_probe_result(ObservationKind::ToolIdentity, path, value, root)?,
            _ => return Err(invalid_evidence()),
        };
        manifest.push(observation).map_err(|_| invalid_evidence())?;
    }
    if cursor != encoded.len() {
        return Err(invalid_evidence());
    }
    Ok(manifest)
}

fn validate_directory_contents(
    root: &Path,
    path: &str,
    encoded: &[u8],
) -> io::Result<ConfigureObservation> {
    if encoded.len() > MAX_DIRECTORY_BYTES {
        return Err(invalid_evidence());
    }
    let mut facts = Vec::new();
    let mut cursor = 0usize;
    let count = read_u32(encoded, &mut cursor)? as usize;
    if count > MAX_RECORDS {
        return Err(invalid_evidence());
    }
    for _ in 0..count {
        let name_len = read_u32(encoded, &mut cursor)? as usize;
        let end = cursor.checked_add(name_len).ok_or_else(invalid_evidence)?;
        let name = encoded.get(cursor..end).ok_or_else(invalid_evidence)?;
        cursor = end;
        let kind = *encoded.get(cursor).ok_or_else(invalid_evidence)?;
        cursor += 1;
        if name.is_empty()
            || !matches!(kind, 1..=7 | 255)
            || name.contains(&0)
            || name.contains(&b'/')
            || matches!(name, b"." | b"..")
        {
            return Err(invalid_evidence());
        }
        let size = read_u64(encoded, &mut cursor)?;
        facts.push((name.to_vec(), kind, size));
    }
    if cursor != encoded.len() {
        return Err(invalid_evidence());
    }
    let relative = Path::new(path);
    let current = read_directory_facts(root, relative)?;
    facts.sort();
    if facts.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(invalid_evidence());
    }
    if facts != current {
        return Err(changed_evidence(path));
    }
    directory_observation(relative.to_path_buf(), &facts)
}

fn validate_file_contents(
    root: &Path,
    path: &str,
    bytes: &[u8],
) -> io::Result<ConfigureObservation> {
    if bytes.len() > MAX_FILE_CONTENT_BYTES {
        return Err(invalid_evidence());
    }
    let relative = Path::new(path);
    let resolved = crate::configure_observation::resolve_observation_path(root, relative)?;
    let mut current = Vec::new();
    fs::File::open(resolved)?
        .take((MAX_FILE_CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut current)?;
    if current.len() > MAX_FILE_CONTENT_BYTES || current != bytes {
        return Err(changed_evidence(path));
    }
    ConfigureObservation::new(
        relative.to_path_buf(),
        ObservationKind::FileContents,
        *blake3::hash(bytes).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn validate_presence(
    root: &Path,
    path: &str,
    state: &[u8],
    kind: ObservationKind,
) -> io::Result<ConfigureObservation> {
    if state.len() != 1 || state[0] > 3 {
        return Err(invalid_evidence());
    }
    let observation = crate::configure_observation::observe_presence(root, path.into(), kind)?;
    let expected = [
        state[0],
        u8::from(kind == ObservationKind::DirectoryPresence),
    ];
    if observation.fingerprint() != blake3::hash(&expected).as_bytes() {
        return Err(changed_evidence(path));
    }
    Ok(observation)
}

fn validate_environment(name: &str, encoded: &[u8]) -> io::Result<ConfigureObservation> {
    if name.is_empty()
        || name.len() > MAX_ENV_BYTES
        || name.contains('\0')
        || name.contains('=')
        || encoded.len() > MAX_ENV_BYTES
        || encoded.is_empty()
        || encoded[0] > 1
        || (encoded[0] == 0 && encoded.len() != 1)
    {
        return Err(invalid_evidence());
    }
    if encoded[0] == 1 && std::str::from_utf8(&encoded[1..]).is_err() {
        return Err(invalid_evidence());
    }
    let matches = match (encoded[0], std::env::var(name)) {
        (0, Err(std::env::VarError::NotPresent)) => true,
        (1, Ok(value)) => value.as_bytes() == &encoded[1..],
        _ => false,
    };
    if !matches {
        return Err(changed_evidence(name));
    }
    ConfigureObservation::new(
        name,
        ObservationKind::EnvironmentValue,
        *blake3::hash(encoded).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> io::Result<u32> {
    let end = cursor.checked_add(4).ok_or_else(invalid_evidence)?;
    let value = u32::from_le_bytes(
        bytes
            .get(*cursor..end)
            .ok_or_else(invalid_evidence)?
            .try_into()
            .unwrap(),
    );
    *cursor = end;
    Ok(value)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> io::Result<u64> {
    let end = cursor.checked_add(8).ok_or_else(invalid_evidence)?;
    let value = u64::from_le_bytes(
        bytes
            .get(*cursor..end)
            .ok_or_else(invalid_evidence)?
            .try_into()
            .unwrap(),
    );
    *cursor = end;
    Ok(value)
}

fn invalid_evidence() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "runner observation evidence is invalid",
    )
}

fn changed_evidence(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("runner observation changed before validation: {path}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn encode(records: &[(u8, &str, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::from(MAGIC.as_slice());
        bytes.extend_from_slice(&RUNNER_OBSERVATIONS.protocol_epoch.to_le_bytes());
        bytes.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for (kind, path, value) in records {
            bytes.push(*kind);
            bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
            bytes.extend_from_slice(path.as_bytes());
            bytes.extend_from_slice(value);
        }
        bytes
    }

    fn temp_root() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("nia-runner-observations-{nonce}"))
    }

    fn directory_value(facts: &[crate::configure_observation::DirectoryFact]) -> Vec<u8> {
        let mut value = Vec::new();
        value.extend_from_slice(&(facts.len() as u32).to_le_bytes());
        for (name, kind, size) in facts {
            value.extend_from_slice(&(name.len() as u32).to_le_bytes());
            value.extend_from_slice(name);
            value.push(*kind);
            value.extend_from_slice(&size.to_le_bytes());
        }
        value
    }

    #[test]
    fn directory_evidence_is_order_independent_and_tracks_type_and_size() {
        let root = temp_root();
        fs::create_dir_all(root.join("inputs/child")).unwrap();
        fs::write(root.join("inputs/value"), b"abc").unwrap();
        let mut facts = read_directory_facts(&root, Path::new("inputs")).unwrap();
        assert!(facts.iter().any(|fact| fact == &(b"value".to_vec(), 6, 3)));
        assert!(facts.iter().any(|fact| fact.0 == b"child" && fact.1 == 3));
        facts.reverse();
        let value = directory_value(&facts);
        let evidence = encode(&[(2, "inputs", &value)]);
        let manifest = decode_and_validate_runner_observations(&root, &evidence).unwrap();
        assert_eq!(
            manifest.validate(&root),
            crate::ObservationValidation::Match
        );
        let mut wrong_kind = facts.clone();
        wrong_kind
            .iter_mut()
            .find(|fact| fact.0 == b"value")
            .unwrap()
            .1 = 5;
        assert!(
            decode_and_validate_runner_observations(
                &root,
                &encode(&[(2, "inputs", &directory_value(&wrong_kind))])
            )
            .is_err()
        );
        fs::write(root.join("inputs/value"), b"longer").unwrap();
        assert!(decode_and_validate_runner_observations(&root, &evidence).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_duplicate_facts_and_malformed_directory_records() {
        let root = temp_root();
        fs::create_dir_all(root.join("inputs")).unwrap();
        fs::write(root.join("inputs/value"), b"abc").unwrap();
        assert!(
            decode_and_validate_runner_observations(
                &root,
                &encode(&[(0, "inputs/value", b"abc"), (0, "inputs/value", b"abc")])
            )
            .is_err()
        );
        let facts = read_directory_facts(&root, Path::new("inputs")).unwrap();
        let value = directory_value(&facts);
        for length in 0..value.len() {
            assert!(
                decode_and_validate_runner_observations(
                    &root,
                    &encode(&[(2, "inputs", &value[..length])])
                )
                .is_err(),
                "accepted directory prefix of length {length}"
            );
        }
        let mut duplicate = facts.clone();
        duplicate.extend(facts);
        assert!(
            decode_and_validate_runner_observations(
                &root,
                &encode(&[(2, "inputs", &directory_value(&duplicate))])
            )
            .is_err()
        );
        for name in [b".".as_slice(), b"..", b"a/b", b"\0", b""] {
            let invalid = directory_value(&[(name.to_vec(), 6, 3)]);
            assert!(
                decode_and_validate_runner_observations(&root, &encode(&[(2, "inputs", &invalid)]))
                    .is_err()
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn directory_evidence_preserves_non_utf8_names_and_symlink_metadata() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let root = temp_root();
        fs::create_dir_all(root.join("inputs")).unwrap();
        let name = std::ffi::OsStr::from_bytes(b"raw-\xff");
        fs::write(root.join("inputs").join(name), b"abc").unwrap();
        symlink("missing", root.join("inputs/link")).unwrap();
        let facts = read_directory_facts(&root, Path::new("inputs")).unwrap();
        assert!(facts.contains(&(b"raw-\xff".to_vec(), 6, 3)));
        assert!(facts.contains(&(b"link".to_vec(), 5, 7)));
        let evidence = encode(&[(2, "inputs", &directory_value(&facts))]);
        let manifest = decode_and_validate_runner_observations(&root, &evidence).unwrap();
        assert_eq!(
            manifest.validate(&root),
            crate::ObservationValidation::Match
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_malformed_environment_names_and_absence_values() {
        let root = temp_root();
        for name in ["", "NIA=INVALID", "NIA\0INVALID"] {
            assert!(
                decode_and_validate_runner_observations(&root, &encode(&[(4, name, &[0])]))
                    .is_err()
            );
        }
        assert!(
            decode_and_validate_runner_observations(
                &root,
                &encode(&[(4, "NIA_RUNNER_UNUSED_ABSENT", &[0, 42])])
            )
            .is_err()
        );
    }

    #[test]
    fn validates_runner_file_contents_and_absent_paths() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("settings.nia"), b"enabled").unwrap();
        let bytes = encode(&[(0, "settings.nia", b"enabled"), (1, "optional.nia", &[0])]);
        let manifest = decode_and_validate_runner_observations(&root, &bytes).unwrap();
        assert_eq!(manifest.observations().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_facts_changed_after_the_runner_observed_them() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("settings.nia"), b"new").unwrap();
        let bytes = encode(&[(0, "settings.nia", b"old")]);
        assert!(decode_and_validate_runner_observations(&root, &bytes).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_trailing_bytes_and_oversized_evidence() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let mut bytes = encode(&[]);
        bytes.push(0);
        assert!(decode_and_validate_runner_observations(&root, &bytes).is_err());
        assert!(
            decode_and_validate_runner_observations(&root, &vec![0; MAX_EVIDENCE_BYTES + 1])
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
