// SPDX-License-Identifier: GPL-3.0-or-later
//! Git loose-object encoding and validation.

use std::io::{self, Read};

use flate2::read::ZlibDecoder;
use sha1::{Digest, Sha1};

use super::MAX_OBJECT_BYTES;
use crate::invalid_packet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitObject {
    pub(crate) kind: ObjectKind,
    pub(crate) data: Vec<u8>,
    pub(crate) id: [u8; 20],
}

pub(crate) fn decode_loose_object(compressed: &[u8]) -> io::Result<GitObject> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut bytes = Vec::new();
    (&mut decoder)
        .take((MAX_OBJECT_BYTES + 128) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_OBJECT_BYTES + 128 {
        return Err(invalid_packet("Git object exceeds size limit"));
    }
    if decoder.total_in() as usize != compressed.len() {
        return Err(invalid_packet("Git object has trailing compressed data"));
    }
    let separator = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| invalid_packet("Git object header is missing its terminator"))?;
    let header = std::str::from_utf8(&bytes[..separator])
        .map_err(|_| invalid_packet("Git object header is not ASCII"))?;
    let (kind, declared_size) = header
        .split_once(' ')
        .ok_or_else(|| invalid_packet("Git object header is malformed"))?;
    let kind = match kind {
        "commit" => ObjectKind::Commit,
        "tree" => ObjectKind::Tree,
        "blob" => ObjectKind::Blob,
        "tag" => ObjectKind::Tag,
        _ => return Err(invalid_packet("unsupported Git object type")),
    };
    let declared_size = declared_size
        .parse::<usize>()
        .map_err(|_| invalid_packet("Git object size is malformed"))?;
    let data = &bytes[separator + 1..];
    if data.len() != declared_size || declared_size > MAX_OBJECT_BYTES {
        return Err(invalid_packet("Git object size does not match its header"));
    }
    let mut hash = Sha1::new();
    hash.update(&bytes[..separator + 1]);
    hash.update(data);
    let id: [u8; 20] = hash.finalize().into();
    Ok(GitObject {
        kind,
        data: data.to_vec(),
        id,
    })
}

#[cfg(test)]
pub(crate) fn encode_loose_object(kind: ObjectKind, data: &[u8]) -> io::Result<Vec<u8>> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write as _;

    if data.len() > MAX_OBJECT_BYTES {
        return Err(invalid_packet("Git object exceeds size limit"));
    }
    let kind = match kind {
        ObjectKind::Commit => "commit",
        ObjectKind::Tree => "tree",
        ObjectKind::Blob => "blob",
        ObjectKind::Tag => "tag",
    };
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    write!(encoder, "{kind} {}\0", data.len())?;
    encoder.write_all(data)?;
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::{ObjectKind, decode_loose_object, encode_loose_object};
    use std::io;

    #[test]
    fn loose_objects_validate_type_size_and_git_object_identity() {
        let data = b"tree 0000000000000000000000000000000000000000\n";
        let encoded = encode_loose_object(ObjectKind::Commit, data).unwrap();
        let decoded = decode_loose_object(&encoded).unwrap();
        assert_eq!(decoded.kind, ObjectKind::Commit);
        assert_eq!(decoded.data, data);
        assert_eq!(decoded.id.len(), 20);

        let mut corrupt = encoded;
        corrupt.push(0);
        assert_eq!(
            decode_loose_object(&corrupt).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
