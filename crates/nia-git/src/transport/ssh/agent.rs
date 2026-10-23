// SPDX-License-Identifier: GPL-3.0-or-later
//! SSH agent discovery, identity selection, and signing.

use super::identity::SshSigner;
use super::wire::MAX_SSH_PACKET_BYTES;
use super::wire::{SshReader, encode_ssh_string};
use crate::invalid_packet;
use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::rc::Rc;
#[cfg(any(unix, windows))]
use std::time::Duration;

trait SshAgentStream: Read + Write {}
const MAX_AGENT_IDENTITIES: u32 = 1024;
#[cfg(any(unix, windows))]
const AGENT_IO_TIMEOUT: Duration = Duration::from_secs(30);
impl<T: Read + Write> SshAgentStream for T {}

/// The agent connection shared by its identities. `None` after an I/O or
/// framing error: a reply could still be in flight, and reading it as the
/// answer to the next request would pair a signature with the wrong key.
type SharedAgentStream = Rc<RefCell<Option<Box<dyn SshAgentStream>>>>;

/// One agent key. Identities from the same agent share its connection, so
/// authentication can offer them one at a time in the agent's order.
pub(super) struct SshAgentIdentity {
    stream: SharedAgentStream,
    public_key: Vec<u8>,
    signature_algorithm: Vec<u8>,
    signature_flags: u32,
}

/// Lists the running agent's supported identities in the order the agent
/// reports them. No agent, or an agent without supported keys, yields none.
pub(super) fn agent_identities() -> io::Result<Vec<SshAgentIdentity>> {
    let Some(mut stream) = connect_agent_stream() else {
        return Ok(Vec::new());
    };
    let response = agent_request(&mut *stream, &[11])?;
    let public_keys = parse_agent_identities(&response)?;
    let stream = Rc::new(RefCell::new(Some(stream)));
    public_keys
        .into_iter()
        .map(|public_key| {
            let (_, signature_algorithm, signature_flags) = agent_signing_algorithm(&public_key)?;
            Ok(SshAgentIdentity {
                stream: Rc::clone(&stream),
                public_key,
                signature_algorithm,
                signature_flags,
            })
        })
        .collect()
}

impl SshAgentIdentity {
    fn sign(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
        let mut request = vec![13];
        encode_ssh_string(&self.public_key, &mut request)?;
        encode_ssh_string(message, &mut request)?;
        request.extend_from_slice(&self.signature_flags.to_be_bytes());
        let mut connection = self.stream.borrow_mut();
        let Some(stream) = connection.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "SSH agent connection failed during an earlier request",
            ));
        };
        let response = match agent_request(&mut **stream, &request) {
            Ok(response) => response,
            Err(error) => {
                *connection = None;
                return Err(error);
            }
        };
        drop(connection);
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

impl SshSigner for SshAgentIdentity {
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

/// Returns the supported public keys from an identities reply, keeping the
/// agent's order; `ssh-add` order is the user's preference.
pub(super) fn parse_agent_identities(response: &[u8]) -> io::Result<Vec<Vec<u8>>> {
    if response.first() == Some(&5) {
        return Ok(Vec::new());
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
    Ok(public_keys)
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
    let stream = UnixStream::connect(socket).ok()?;
    set_agent_io_timeout(&stream, AGENT_IO_TIMEOUT).ok()?;
    Some(Box::new(stream))
}

#[cfg(unix)]
fn set_agent_io_timeout(
    stream: &std::os::unix::net::UnixStream,
    timeout: Duration,
) -> io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))
}

#[cfg(windows)]
fn connect_agent_stream() -> Option<Box<dyn SshAgentStream>> {
    use std::os::windows::fs::OpenOptionsExt;

    let socket =
        std::env::var_os("SSH_AUTH_SOCK").unwrap_or_else(|| r"\\.\pipe\openssh-ssh-agent".into());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED)
        .open(socket)
        .ok()?;
    Some(Box::new(WindowsAgentStream {
        file,
        timeout: AGENT_IO_TIMEOUT,
    }))
}

#[cfg(windows)]
struct WindowsAgentStream {
    file: std::fs::File,
    timeout: Duration,
}

#[cfg(windows)]
impl Read for WindowsAgentStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows_sys::Win32::Storage::FileSystem::ReadFile;
        use windows_sys::Win32::System::IO::OVERLAPPED;

        if buffer.is_empty() {
            return Ok(0);
        }
        let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let handle = self.file.as_raw_handle();
        let mut overlapped = OVERLAPPED::default();
        let mut transferred = 0;
        // SAFETY: the buffer and OVERLAPPED remain live until completion below.
        let complete = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                length,
                &mut transferred,
                &mut overlapped,
            )
        };
        if complete != 0 {
            return Ok(transferred as usize);
        }
        // SAFETY: GetLastError reads the calling thread's last Win32 error.
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        wait_agent_io(handle, &mut overlapped, &mut transferred, self.timeout)?;
        Ok(transferred as usize)
    }
}

#[cfg(windows)]
impl Write for WindowsAgentStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows_sys::Win32::Storage::FileSystem::WriteFile;
        use windows_sys::Win32::System::IO::OVERLAPPED;

        if buffer.is_empty() {
            return Ok(0);
        }
        let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let handle = self.file.as_raw_handle();
        let mut overlapped = OVERLAPPED::default();
        let mut transferred = 0;
        // SAFETY: the buffer and OVERLAPPED remain live until completion below.
        let complete = unsafe {
            WriteFile(
                handle,
                buffer.as_ptr(),
                length,
                &mut transferred,
                &mut overlapped,
            )
        };
        if complete != 0 {
            return Ok(transferred as usize);
        }
        // SAFETY: GetLastError reads the calling thread's last Win32 error.
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        wait_agent_io(handle, &mut overlapped, &mut transferred, self.timeout)?;
        Ok(transferred as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
fn wait_agent_io(
    handle: std::os::windows::io::RawHandle,
    overlapped: &mut windows_sys::Win32::System::IO::OVERLAPPED,
    transferred: &mut u32,
    timeout: Duration,
) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{
        ERROR_IO_INCOMPLETE, ERROR_TIMEOUT, GetLastError, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, GetOverlappedResultEx};

    let timeout_ms = u32::try_from(timeout.as_millis()).expect("agent timeout fits u32");
    // SAFETY: this is the outstanding operation on an overlapped pipe handle.
    if unsafe { GetOverlappedResultEx(handle, overlapped, transferred, timeout_ms, 0) } != 0 {
        return Ok(());
    }
    // SAFETY: GetLastError reads the calling thread's last Win32 error.
    let error = unsafe { GetLastError() };
    // The wait failing does not mean the operation finished: a timeout
    // reports WAIT_TIMEOUT, and other failures can leave it pending too. The
    // kernel may still write into the caller's buffer and OVERLAPPED, both of
    // which leave scope when this returns, so cancel the operation and wait
    // for it to settle on every failure path. Waiting on an operation that
    // already completed returns immediately.
    // SAFETY: cancel this operation, then block until the kernel has
    // finished with the buffer and OVERLAPPED storage.
    unsafe {
        CancelIoEx(handle, overlapped);
        GetOverlappedResult(handle, overlapped, transferred, 1);
    }
    if matches!(error, WAIT_TIMEOUT | ERROR_TIMEOUT | ERROR_IO_INCOMPLETE) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "SSH agent I/O timed out",
        ));
    }
    Err(io::Error::from_raw_os_error(error as i32))
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
    fn agent_io_error_disables_the_shared_connection() {
        let mut public_key = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut public_key).unwrap();
        encode_ssh_string(&[8; 32], &mut public_key).unwrap();
        // A truncated reply: the length promises more bytes than arrive.
        let stream: Box<dyn SshAgentStream> = Box::new(TestStream {
            incoming: Cursor::new(vec![0, 0, 0, 9, 14]),
            outgoing: Arc::new(Mutex::new(Vec::new())),
        });
        let shared = Rc::new(RefCell::new(Some(stream)));
        let identity = |shared: &SharedAgentStream| SshAgentIdentity {
            stream: Rc::clone(shared),
            public_key: public_key.clone(),
            signature_algorithm: b"ssh-ed25519".to_vec(),
            signature_flags: 0,
        };
        let (mut first, mut second) = (identity(&shared), identity(&shared));
        assert_eq!(
            first.sign(b"one").unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        // The second identity must not read whatever the agent sends next
        // as the answer to its own request.
        assert!(shared.borrow().is_none());
        assert_eq!(
            second.sign(b"two").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
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
        let mut agent = SshAgentIdentity {
            stream: Rc::new(RefCell::new(Some(Box::new(stream)))),
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
    fn ssh_agent_identity_reply_keeps_agent_order_and_rejects_bad_frames() {
        let mut ed25519 = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut ed25519).unwrap();
        encode_ssh_string(&[7; 32], &mut ed25519).unwrap();
        let mut ecdsa = Vec::new();
        encode_ssh_string(b"ecdsa-sha2-nistp256", &mut ecdsa).unwrap();
        encode_ssh_string(b"nistp256", &mut ecdsa).unwrap();
        encode_ssh_string(&[vec![4], vec![7; 64]].concat(), &mut ecdsa).unwrap();
        let mut rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut rsa).unwrap();
        encode_ssh_string(&[1, 0, 1], &mut rsa).unwrap();
        encode_ssh_string(&[0x7f; 256], &mut rsa).unwrap();
        let mut short_rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut short_rsa).unwrap();
        encode_ssh_string(&[1, 2, 3], &mut short_rsa).unwrap();

        // The agent's order wins over key type, and unsupported keys are
        // skipped rather than failing the whole reply.
        let mut identities = vec![12];
        identities.extend_from_slice(&4u32.to_be_bytes());
        for (key, comment) in [
            (&rsa, "rsa fixture"),
            (&short_rsa, "short rsa fixture"),
            (&ecdsa, "ecdsa fixture"),
            (&ed25519, "ed25519 fixture"),
        ] {
            encode_ssh_string(key, &mut identities).unwrap();
            encode_ssh_string(comment.as_bytes(), &mut identities).unwrap();
        }
        assert_eq!(
            parse_agent_identities(&identities).unwrap(),
            [rsa.clone(), ecdsa, ed25519]
        );
        assert_eq!(
            agent_signing_algorithm(&rsa).unwrap(),
            (b"ssh-rsa".to_vec(), b"rsa-sha2-512".to_vec(), 4)
        );
        assert!(parse_agent_identities(&[5]).unwrap().is_empty());
        assert!(parse_agent_identities(&[12, 0, 0, 0]).is_err());
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

    #[cfg(unix)]
    #[test]
    fn agent_request_times_out_when_the_agent_does_not_reply() {
        use std::os::unix::net::UnixStream;

        let (mut client, _agent) = UnixStream::pair().unwrap();
        set_agent_io_timeout(&client, Duration::from_millis(25)).unwrap();

        let error = agent_request(&mut client, &[11]).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_agent_pipe_times_out_when_the_agent_does_not_reply() {
        use std::{
            os::windows::fs::OpenOptionsExt,
            sync::atomic::{AtomicU64, Ordering},
        };
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX},
            System::Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
            },
        };

        static NEXT_PIPE: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            r"\\.\pipe\nia-agent-timeout-{}-{}",
            std::process::id(),
            NEXT_PIPE.fetch_add(1, Ordering::Relaxed)
        );
        let wide_name = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        // SAFETY: wide_name is nul-terminated and remains live for the call.
        let server = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(server, INVALID_HANDLE_VALUE);
        struct PipeHandle(windows_sys::Win32::Foundation::HANDLE);
        impl Drop for PipeHandle {
            fn drop(&mut self) {
                // SAFETY: this guard uniquely owns the pipe handle.
                unsafe { CloseHandle(self.0) };
            }
        }
        let _server = PipeHandle(server);

        let mut client = WindowsAgentStream {
            file: std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(&name)
                .unwrap(),
            timeout: Duration::from_millis(25),
        };
        // SAFETY: server is a live, synchronous named-pipe instance.
        let connected = unsafe { ConnectNamedPipe(server, std::ptr::null_mut()) };
        if connected == 0 {
            // SAFETY: GetLastError reads the calling thread's last Win32 error.
            assert_eq!(unsafe { GetLastError() }, ERROR_PIPE_CONNECTED);
        }

        let error = agent_request(&mut client, &[11]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // Only the cancellation path produces this message. A raw
        // WAIT_TIMEOUT error would have the same kind while leaving the read
        // pending on freed stack storage.
        assert_eq!(error.to_string(), "SSH agent I/O timed out", "{error:?}");
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
