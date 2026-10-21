// SPDX-License-Identifier: GPL-3.0-or-later
//! SSH agent discovery, identity selection, and signing.

use super::identity::SshSigner;
use super::wire::MAX_SSH_PACKET_BYTES;
use super::wire::{SshReader, encode_ssh_string};
use crate::invalid_packet;
use std::io::{self, Read, Write};

trait SshAgentStream: Read + Write {}
const MAX_AGENT_IDENTITIES: u32 = 1024;
impl<T: Read + Write> SshAgentStream for T {}

pub(super) struct SshAgent {
    stream: Box<dyn SshAgentStream>,
    public_key: Vec<u8>,
    signature_algorithm: Vec<u8>,
    signature_flags: u32,
}

impl SshAgent {
    pub(super) fn connect() -> io::Result<Option<Self>> {
        let Some(mut stream) = connect_agent_stream() else {
            return Ok(None);
        };
        let response = agent_request(&mut *stream, &[11])?;
        let public_key = parse_agent_identities(&response)?;
        let Some(public_key) = public_key else {
            return Ok(None);
        };
        let (_, signature_algorithm, signature_flags) = agent_signing_algorithm(&public_key)?;
        Ok(Some(Self {
            stream,
            public_key,
            signature_algorithm,
            signature_flags,
        }))
    }

    fn sign(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
        let mut request = vec![13];
        encode_ssh_string(&self.public_key, &mut request)?;
        encode_ssh_string(message, &mut request)?;
        request.extend_from_slice(&self.signature_flags.to_be_bytes());
        let response = agent_request(&mut *self.stream, &request)?;
        if response.first() != Some(&14) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH agent refused the public-key signing request",
            ));
        }
        let mut reader = SshReader::new(&response[1..]);
        let signature_blob = reader.string()?;
        reader.finish()?;
        let mut signature_reader = SshReader::new(signature_blob);
        if signature_reader.string()? != self.signature_algorithm {
            return Err(invalid_packet(
                "SSH agent returned a signature with the wrong algorithm",
            ));
        }
        let signature = signature_reader.string()?;
        signature_reader.finish()?;
        if signature.is_empty() {
            return Err(invalid_packet(
                "SSH agent returned an empty public-key signature",
            ));
        }
        Ok(signature.to_vec())
    }
}

impl SshSigner for SshAgent {
    fn public_key_blob(&self) -> &[u8] {
        &self.public_key
    }

    fn signature_algorithm(&self) -> &[u8] {
        &self.signature_algorithm
    }

    fn sign_message(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
        self.sign(message)
    }
}

pub(super) fn agent_signing_algorithm(blob: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>, u32)> {
    let mut reader = SshReader::new(blob);
    let algorithm = reader.string()?;
    let result = match algorithm {
        b"ssh-ed25519" if reader.string()?.len() == 32 => {
            (algorithm.to_vec(), algorithm.to_vec(), 0)
        }
        b"ecdsa-sha2-nistp256" => {
            let curve = reader.string()?;
            let point = reader.string()?;
            if curve != b"nistp256" || point.len() != 65 || point[0] != 4 {
                return Err(invalid_packet("SSH agent returned an invalid ECDSA key"));
            }
            (algorithm.to_vec(), algorithm.to_vec(), 0)
        }
        b"ssh-rsa" => {
            let exponent = reader.string()?;
            let modulus = reader.string()?;
            if exponent.is_empty() || modulus.len() < 128 {
                return Err(invalid_packet("SSH agent returned an invalid RSA key"));
            }
            (algorithm.to_vec(), b"rsa-sha2-512".to_vec(), 4)
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SSH agent identity key type is unsupported",
            ));
        }
    };
    reader.finish()?;
    Ok(result)
}

pub(super) fn parse_agent_identities(response: &[u8]) -> io::Result<Option<Vec<u8>>> {
    if response.first() == Some(&5) {
        return Ok(None);
    }
    let Some(message) = response.first().copied() else {
        return Err(invalid_packet(
            "SSH agent returned an empty identities reply",
        ));
    };
    if message != 12 {
        return Err(invalid_packet(
            "SSH agent returned an unexpected identities reply",
        ));
    }
    let mut reader = SshReader::new(&response[1..]);
    let count = reader.u32()?;
    if count > MAX_AGENT_IDENTITIES {
        return Err(invalid_packet("SSH agent returned too many identities"));
    }
    let mut public_keys = Vec::new();
    for _ in 0..count {
        let key = reader.string()?;
        let _comment = reader.string()?;
        if agent_signing_algorithm(key).is_ok() {
            public_keys.push(key.to_vec());
        }
    }
    reader.finish()?;
    let selected = public_keys
        .iter()
        .find(|key| {
            agent_signing_algorithm(key).is_ok_and(|(algorithm, _, _)| algorithm == b"ssh-ed25519")
        })
        .or_else(|| {
            public_keys.iter().find(|key| {
                agent_signing_algorithm(key)
                    .is_ok_and(|(algorithm, _, _)| algorithm == b"ecdsa-sha2-nistp256")
            })
        })
        .or_else(|| {
            public_keys.iter().find(|key| {
                agent_signing_algorithm(key).is_ok_and(|(algorithm, _, _)| algorithm == b"ssh-rsa")
            })
        });
    Ok(selected.cloned())
}

pub(super) fn agent_request<S: Read + Write + ?Sized>(
    stream: &mut S,
    payload: &[u8],
) -> io::Result<Vec<u8>> {
    if payload.is_empty() || payload.len() > MAX_SSH_PACKET_BYTES {
        return Err(invalid_packet("SSH agent request exceeds protocol limit"));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| invalid_packet("SSH agent request exceeds uint32 length"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| invalid_packet("SSH agent response length overflows usize"))?;
    if length == 0 || length > MAX_SSH_PACKET_BYTES {
        return Err(invalid_packet("SSH agent response exceeds protocol limit"));
    }
    let mut response = vec![0; length];
    stream.read_exact(&mut response)?;
    Ok(response)
}

#[cfg(unix)]
fn connect_agent_stream() -> Option<Box<dyn SshAgentStream>> {
    use std::os::unix::net::UnixStream;

    let socket = std::env::var_os("SSH_AUTH_SOCK")?;
    UnixStream::connect(socket)
        .ok()
        .map(|stream| Box::new(stream) as Box<dyn SshAgentStream>)
}

#[cfg(windows)]
fn connect_agent_stream() -> Option<Box<dyn SshAgentStream>> {
    let socket =
        std::env::var_os("SSH_AUTH_SOCK").unwrap_or_else(|| r"\\.\pipe\openssh-ssh-agent".into());
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(socket)
        .ok()
        .map(|stream| Box::new(stream) as Box<dyn SshAgentStream>)
}

#[cfg(not(any(unix, windows)))]
fn connect_agent_stream() -> Option<Box<dyn SshAgentStream>> {
    None
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    struct TestStream {
        incoming: Cursor<Vec<u8>>,
        outgoing: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for TestStream {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.incoming.read(output)
        }
    }

    impl Write for TestStream {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.outgoing.lock().unwrap().extend_from_slice(input);
            Ok(input.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn agent_sign_request_uses_selected_key_and_parses_signature() {
        let mut public_key = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut public_key).unwrap();
        encode_ssh_string(&[8; 32], &mut public_key).unwrap();
        let mut signature_blob = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut signature_blob).unwrap();
        encode_ssh_string(&[9; 64], &mut signature_blob).unwrap();
        let mut response = vec![14];
        encode_ssh_string(&signature_blob, &mut response).unwrap();
        let mut incoming = Vec::new();
        incoming.extend_from_slice(&(response.len() as u32).to_be_bytes());
        incoming.extend_from_slice(&response);
        let outgoing = Arc::new(Mutex::new(Vec::new()));
        let stream = TestStream {
            incoming: Cursor::new(incoming),
            outgoing: Arc::clone(&outgoing),
        };
        let (_, signature_algorithm, signature_flags) =
            agent_signing_algorithm(&public_key).unwrap();
        let mut agent = SshAgent {
            stream: Box::new(stream),
            public_key: public_key.clone(),
            signature_algorithm,
            signature_flags,
        };
        assert_eq!(agent.sign(b"session-bound request").unwrap(), [9; 64]);
        let outgoing = outgoing.lock().unwrap();
        assert_eq!(
            u32::from_be_bytes(outgoing[..4].try_into().unwrap()) as usize,
            outgoing.len() - 4
        );
        let mut request = SshReader::new(&outgoing[4..]);
        assert_eq!(request.take(1).unwrap(), [13]);
        assert_eq!(request.string().unwrap(), public_key);
        assert_eq!(request.string().unwrap(), b"session-bound request");
        assert_eq!(request.u32().unwrap(), 0);
        request.finish().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_agent_identity_reply_selects_ed25519_and_rejects_bad_frames() {
        let mut ed25519 = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut ed25519).unwrap();
        encode_ssh_string(&[7; 32], &mut ed25519).unwrap();
        let mut rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut rsa).unwrap();
        encode_ssh_string(&[1, 2, 3], &mut rsa).unwrap();

        let mut identities = vec![12];
        identities.extend_from_slice(&2u32.to_be_bytes());
        encode_ssh_string(&rsa, &mut identities).unwrap();
        encode_ssh_string(b"rsa fixture", &mut identities).unwrap();
        encode_ssh_string(&ed25519, &mut identities).unwrap();
        encode_ssh_string(b"ed25519 fixture", &mut identities).unwrap();
        assert_eq!(parse_agent_identities(&identities).unwrap(), Some(ed25519));

        let mut ecdsa = Vec::new();
        encode_ssh_string(b"ecdsa-sha2-nistp256", &mut ecdsa).unwrap();
        encode_ssh_string(b"nistp256", &mut ecdsa).unwrap();
        encode_ssh_string(&[vec![4], vec![7; 64]].concat(), &mut ecdsa).unwrap();
        let mut rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut rsa).unwrap();
        encode_ssh_string(&[1, 0, 1], &mut rsa).unwrap();
        encode_ssh_string(&[0x7f; 256], &mut rsa).unwrap();
        let mut identities = vec![12];
        identities.extend_from_slice(&2u32.to_be_bytes());
        encode_ssh_string(&rsa, &mut identities).unwrap();
        encode_ssh_string(b"rsa fixture", &mut identities).unwrap();
        encode_ssh_string(&ecdsa, &mut identities).unwrap();
        encode_ssh_string(b"ecdsa fixture", &mut identities).unwrap();
        assert_eq!(
            parse_agent_identities(&identities).unwrap(),
            Some(ecdsa.clone())
        );
        assert_eq!(
            agent_signing_algorithm(&rsa).unwrap(),
            (b"ssh-rsa".to_vec(), b"rsa-sha2-512".to_vec(), 4)
        );
        assert_eq!(parse_agent_identities(&[5]).unwrap(), None);
        assert!(parse_agent_identities(&[12, 0, 0, 0]).is_err());
    }

    #[test]
    fn ssh_agent_prefers_ecdsa_then_rsa_and_uses_rsa_sha2_flags() {
        let mut rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut rsa).unwrap();
        encode_ssh_string(&[1, 0, 1], &mut rsa).unwrap();
        encode_ssh_string(&[0x7f; 256], &mut rsa).unwrap();
        let mut identities = vec![12];
        identities.extend_from_slice(&1u32.to_be_bytes());
        encode_ssh_string(&rsa, &mut identities).unwrap();
        encode_ssh_string(b"rsa fixture", &mut identities).unwrap();
        assert_eq!(
            parse_agent_identities(&identities).unwrap(),
            Some(rsa.clone())
        );
        assert_eq!(
            agent_signing_algorithm(&rsa).unwrap(),
            (b"ssh-rsa".to_vec(), b"rsa-sha2-512".to_vec(), 4)
        );
    }

    #[test]
    fn ssh_agent_request_uses_bounded_length_framing() {
        use std::io::Cursor;

        struct TestStream {
            incoming: Cursor<Vec<u8>>,
            outgoing: Vec<u8>,
        }
        impl Read for TestStream {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.incoming.read(output)
            }
        }
        impl Write for TestStream {
            fn write(&mut self, input: &[u8]) -> io::Result<usize> {
                self.outgoing.extend_from_slice(input);
                Ok(input.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut stream = TestStream {
            incoming: Cursor::new(vec![0, 0, 0, 1, 5]),
            outgoing: Vec::new(),
        };
        assert_eq!(agent_request(&mut stream, &[11]).unwrap(), [5]);
        assert_eq!(stream.outgoing, [0, 0, 0, 1, 11]);
        assert!(agent_request(&mut stream, &[]).is_err());
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;
    use std::io::{Cursor, Read, Write};

    struct PropertyStream {
        response: Cursor<Vec<u8>>,
        requests: Vec<u8>,
    }

    impl Read for PropertyStream {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.response.read(output)
        }
    }

    impl Write for PropertyStream {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.requests.extend_from_slice(input);
            Ok(input.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    proptest! {
        #[test]
        fn arbitrary_agent_messages_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let _ = parse_agent_identities(&bytes);
            let _ = agent_signing_algorithm(&bytes);
            let mut identities = vec![12];
            identities.extend_from_slice(&bytes);
            let _ = parse_agent_identities(&identities);
            let mut stream = PropertyStream {
                response: Cursor::new(bytes),
                requests: Vec::new(),
            };
            let _ = agent_request(&mut stream, &[11]);
        }

        #[test]
        fn agent_request_round_trip(payload in prop::collection::vec(any::<u8>(), 1..4096)) {
            let mut response = (payload.len() as u32).to_be_bytes().to_vec();
            response.extend_from_slice(&payload);
            let mut stream = PropertyStream {
                response: Cursor::new(response),
                requests: Vec::new(),
            };
            prop_assert_eq!(agent_request(&mut stream, &[11]).unwrap(), payload);
            prop_assert_eq!(stream.requests, [0, 0, 0, 1, 11]);
        }
    }
}
