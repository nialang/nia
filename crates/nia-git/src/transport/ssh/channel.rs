// SPDX-License-Identifier: GPL-3.0-or-later
//! One upload-pack session channel, including flow control and process exit.

use std::io::{self, Read, Write};

use super::cipher::{SshAes128CtrDirection, write_encrypted_ssh_packet};
use super::wire::{SshReader, encode_ssh_string};
use super::{SSH_CHANNEL_MAX_PACKET, SSH_CHANNEL_WINDOW, SshTransportKeys};
use crate::{invalid_packet, transport::MAX_RESPONSE_BYTES};

pub(super) struct SshExecChannel<'a, S: Read + Write> {
    stream: &'a mut S,
    outbound: &'a mut SshAes128CtrDirection,
    inbound: &'a mut SshAes128CtrDirection,
    remote_channel: u32,
    remote_window: u32,
    remote_max_packet: usize,
    local_window: u32,
    pending: Vec<u8>,
    pending_offset: usize,
    received_bytes: usize,
    exit_status: Option<u32>,
    eof: bool,
    closed: bool,
    eof_sent: bool,
    close_sent: bool,
}

impl<'a, S: Read + Write> SshExecChannel<'a, S> {
    pub(super) fn new(
        stream: &'a mut S,
        keys: &'a mut SshTransportKeys,
        remote_channel: u32,
        remote_window: u32,
        remote_max_packet: u32,
    ) -> io::Result<Self> {
        if remote_max_packet == 0 {
            return Err(invalid_packet("SSH server channel packet limit is zero"));
        }
        Ok(Self {
            stream,
            outbound: &mut keys.outbound,
            inbound: &mut keys.inbound,
            remote_channel,
            remote_window,
            remote_max_packet: usize::try_from(remote_max_packet)
                .map_err(|_| invalid_packet("SSH channel packet limit overflows usize"))?
                .min(SSH_CHANNEL_MAX_PACKET),
            local_window: SSH_CHANNEL_WINDOW,
            pending: Vec::new(),
            pending_offset: 0,
            received_bytes: 0,
            exit_status: None,
            eof: false,
            closed: false,
            eof_sent: false,
            close_sent: false,
        })
    }

    pub(super) fn finish_writing(&mut self) -> io::Result<()> {
        if !self.eof_sent && !self.closed {
            self.send_channel_message(96)?;
            self.eof_sent = true;
        }
        Ok(())
    }

    fn send_channel_message(&mut self, message: u8) -> io::Result<()> {
        let mut payload = vec![message];
        payload.extend_from_slice(&self.remote_channel.to_be_bytes());
        write_encrypted_ssh_packet(self.stream, self.outbound, &payload)
    }

    fn send_close(&mut self) -> io::Result<()> {
        if !self.close_sent {
            self.send_channel_message(97)?;
            self.close_sent = true;
        }
        Ok(())
    }

    fn credit_local_window(&mut self, amount: usize) -> io::Result<()> {
        if amount == 0 || self.closed {
            return Ok(());
        }
        let amount = u32::try_from(amount)
            .map_err(|_| invalid_packet("SSH channel read length overflows uint32"))?;
        self.local_window = self
            .local_window
            .checked_add(amount)
            .ok_or_else(|| invalid_packet("SSH local channel window overflows"))?;
        let mut adjust = vec![93];
        adjust.extend_from_slice(&self.remote_channel.to_be_bytes());
        adjust.extend_from_slice(&amount.to_be_bytes());
        write_encrypted_ssh_packet(self.stream, self.outbound, &adjust)
    }

    fn receive_packet(&mut self) -> io::Result<()> {
        let packet = self.inbound.read_packet(&mut *self.stream)?;
        self.received_bytes = self
            .received_bytes
            .checked_add(packet.payload.len())
            .filter(|total| *total <= MAX_RESPONSE_BYTES)
            .ok_or_else(|| invalid_packet("SSH channel traffic exceeds size limit"))?;
        let Some(message) = packet.payload.first().copied() else {
            return Err(invalid_packet("SSH channel packet is empty"));
        };
        let mut reader = SshReader::new(&packet.payload[1..]);
        // This connection opens exactly one channel, with local id zero.
        if matches!(message, 93..=98) && reader.u32()? != 0 {
            return Err(invalid_packet(
                "SSH channel message has the wrong recipient",
            ));
        }
        match message {
            94 | 95 => {
                if message == 95 && reader.u32()? != 1 {
                    return Err(invalid_packet("SSH extended data type is unsupported"));
                }
                let data = reader.string()?;
                reader.finish()?;
                if self.eof
                    || data.len() > self.local_window as usize
                    || data.len() > SSH_CHANNEL_MAX_PACKET
                {
                    return Err(invalid_packet(
                        "SSH channel data exceeds its window or follows EOF",
                    ));
                }
                self.local_window -= data.len() as u32;
                if message == 94 {
                    self.pending.extend_from_slice(data);
                } else {
                    self.credit_local_window(data.len())?;
                }
            }
            93 => {
                let adjustment = reader.u32()?;
                reader.finish()?;
                self.remote_window = self
                    .remote_window
                    .checked_add(adjustment)
                    .ok_or_else(|| invalid_packet("SSH remote channel window overflows"))?;
            }
            96 => {
                reader.finish()?;
                if self.eof {
                    return Err(invalid_packet("SSH channel sent duplicate EOF"));
                }
                self.eof = true;
            }
            97 => {
                reader.finish()?;
                self.eof = true;
                self.closed = true;
                self.send_close()?;
            }
            98 => {
                let request = reader.string()?;
                let want_reply = reader.boolean()?;
                match request {
                    b"exit-status" => {
                        let status = reader.u32()?;
                        reader.finish()?;
                        if want_reply || self.exit_status.is_some() {
                            return Err(invalid_packet(
                                "SSH exit status is repeated or requests a reply",
                            ));
                        }
                        self.exit_status = Some(status);
                        if status != 0 {
                            return Err(io::Error::other(format!(
                                "SSH git-upload-pack exited with status {status}"
                            )));
                        }
                    }
                    b"exit-signal" => {
                        let signal = reader.string()?;
                        let _core_dumped = reader.boolean()?;
                        let _description = reader.string()?;
                        let _language = reader.string()?;
                        reader.finish()?;
                        if want_reply || self.exit_status.is_some() {
                            return Err(invalid_packet(
                                "SSH exit signal is repeated or requests a reply",
                            ));
                        }
                        return Err(io::Error::other(format!(
                            "SSH git-upload-pack terminated by signal {}",
                            String::from_utf8_lossy(signal)
                        )));
                    }
                    _ if want_reply => self.send_channel_message(100)?,
                    _ => {}
                }
            }
            1 => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "SSH server disconnected during Git transfer",
                ));
            }
            2 => {
                reader.string()?;
                reader.finish()?;
            }
            4 => {
                reader.boolean()?;
                reader.string()?;
                reader.string()?;
                reader.finish()?;
            }
            _ => {
                return Err(invalid_packet(
                    "SSH server sent an unexpected channel message",
                ));
            }
        }
        Ok(())
    }

    fn wait_for_remote_window(&mut self) -> io::Result<()> {
        while self.remote_window == 0 && !self.closed {
            self.receive_packet()?;
        }
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "SSH server closed the channel while awaiting window space",
            ));
        }
        Ok(())
    }
}

impl<S: Read + Write> Read for SshExecChannel<'_, S> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        while self.pending_offset == self.pending.len() {
            self.pending.clear();
            self.pending_offset = 0;
            if self.closed {
                return Ok(0);
            }
            self.receive_packet()?;
        }
        let amount = output.len().min(self.pending.len() - self.pending_offset);
        output[..amount]
            .copy_from_slice(&self.pending[self.pending_offset..self.pending_offset + amount]);
        self.pending_offset += amount;
        self.credit_local_window(amount)?;
        Ok(amount)
    }
}

impl<S: Read + Write> Write for SshExecChannel<'_, S> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        if self.eof_sent || self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "SSH channel write side is closed",
            ));
        }
        self.wait_for_remote_window()?;
        let amount = input
            .len()
            .min(self.remote_max_packet)
            .min(self.remote_window as usize);
        let mut payload = vec![94];
        payload.extend_from_slice(&self.remote_channel.to_be_bytes());
        encode_ssh_string(&input[..amount], &mut payload)?;
        write_encrypted_ssh_packet(self.stream, self.outbound, &payload)?;
        self.remote_window -= amount as u32;
        Ok(amount)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl<S: Read + Write> Drop for SshExecChannel<'_, S> {
    fn drop(&mut self) {
        let _ = self.send_close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Transcript {
        input: io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Transcript {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.input.read(output)
        }
    }

    impl Write for Transcript {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(input);
            Ok(input.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn direction() -> SshAes128CtrDirection {
        SshAes128CtrDirection::new(&[1; 16], &[2; 16], &[3; 32]).unwrap()
    }

    fn keys() -> SshTransportKeys {
        SshTransportKeys {
            inbound: direction(),
            outbound: direction(),
            session_id: [0; 32],
        }
    }

    fn transcript(packets: &[Vec<u8>]) -> Transcript {
        let mut sender = direction();
        let mut input = Vec::new();
        for packet in packets {
            write_encrypted_ssh_packet(&mut input, &mut sender, packet).unwrap();
        }
        Transcript {
            input: io::Cursor::new(input),
            output: Vec::new(),
        }
    }

    fn message(kind: u8, data: &[u8]) -> Vec<u8> {
        let mut packet = vec![kind, 0, 0, 0, 0];
        packet.extend_from_slice(data);
        packet
    }

    fn data(value: &[u8]) -> Vec<u8> {
        let mut packet = message(94, &[]);
        encode_ssh_string(value, &mut packet).unwrap();
        packet
    }

    fn request(name: &[u8], reply: bool, fields: &[u8]) -> Vec<u8> {
        let mut packet = message(98, &[]);
        encode_ssh_string(name, &mut packet).unwrap();
        packet.push(u8::from(reply));
        packet.extend_from_slice(fields);
        packet
    }

    #[test]
    fn eof_does_not_hide_process_failure_or_transport_truncation() {
        for after_eof in [false, true] {
            let mut packets = vec![data(b"complete pack bytes")];
            if after_eof {
                packets.push(message(96, &[]));
            }
            packets.push(request(b"exit-status", false, &17u32.to_be_bytes()));
            packets.push(message(97, &[]));
            let mut stream = transcript(&packets);
            let mut keys = keys();
            let channel = SshExecChannel::new(&mut stream, &mut keys, 7, 32, 32).unwrap();
            let error = crate::input::read_limited(channel, 1024, "test limit").unwrap_err();
            assert!(error.to_string().contains("status 17"));
        }
        let mut stream = transcript(&[message(96, &[])]);
        let mut keys = keys();
        let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, 32, 32).unwrap();
        assert_eq!(
            channel.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn empty_data_and_one_way_eof_do_not_close_the_channel() {
        let mut stream = transcript(&[
            data(b""),
            data(b"abc"),
            message(96, &[]),
            request(b"exit-status", false, &0u32.to_be_bytes()),
            message(97, &[]),
        ]);
        let mut keys = keys();
        let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, 0, 32).unwrap();
        let mut output = [0; 3];
        assert_eq!(channel.read(&mut output).unwrap(), 3);
        assert_eq!(&output, b"abc");
        assert_eq!(channel.read(&mut output).unwrap(), 0);
        assert_eq!(channel.read(&mut output).unwrap(), 0);
        assert_eq!(
            channel.write(b"x").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn window_wait_buffers_data_and_answers_unknown_requests() {
        let mut stream = transcript(&[
            data(b"early"),
            request(b"unknown", true, b"opaque"),
            message(96, &[]),
            message(93, &4u32.to_be_bytes()),
        ]);
        let mut keys = keys();
        {
            let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, 0, 3).unwrap();
            assert_eq!(channel.write(b"request").unwrap(), 3);
            assert!(channel.eof); // Remote EOF does not close our writing direction.
            let mut output = [0; 5];
            assert_eq!(channel.read(&mut output).unwrap(), 5);
            assert_eq!(&output, b"early");
            assert_eq!(channel.remote_window, 1);
            assert_eq!(channel.local_window, SSH_CHANNEL_WINDOW);
        }
        let mut receiver = direction();
        let mut wire = stream.output.as_slice();
        assert_eq!(
            receiver.read_packet(&mut wire).unwrap().payload,
            [100, 0, 0, 0, 7]
        );
        let sent = receiver.read_packet(&mut wire).unwrap().payload;
        assert_eq!(&sent[..5], &[94, 0, 0, 0, 7]);
        assert_eq!(&sent[9..], b"req");
    }

    #[test]
    fn malformed_channel_messages_fail_in_both_read_and_window_wait() {
        let mut wrong_recipient = request(b"unknown", false, &[]);
        wrong_recipient[4] = 1;
        let mut signal = Vec::new();
        encode_ssh_string(b"TERM", &mut signal).unwrap();
        signal.push(0);
        encode_ssh_string(b"stopped", &mut signal).unwrap();
        encode_ssh_string(b"", &mut signal).unwrap();
        let cases = vec![
            (vec![wrong_recipient], io::ErrorKind::InvalidData),
            (vec![message(96, b"extra")], io::ErrorKind::InvalidData),
            (
                vec![message(96, &[]), data(b"late")],
                io::ErrorKind::InvalidData,
            ),
            (
                vec![message(96, &[]), message(96, &[])],
                io::ErrorKind::InvalidData,
            ),
            (
                vec![request(b"exit-status", false, &[0])],
                io::ErrorKind::InvalidData,
            ),
            (
                vec![request(b"exit-status", true, &0u32.to_be_bytes())],
                io::ErrorKind::InvalidData,
            ),
            (
                vec![
                    request(b"exit-status", false, &0u32.to_be_bytes()),
                    request(b"exit-status", false, &0u32.to_be_bytes()),
                ],
                io::ErrorKind::InvalidData,
            ),
            (
                vec![request(b"exit-signal", false, &signal)],
                io::ErrorKind::Other,
            ),
            (
                vec![message(95, &[0, 0, 0, 2, 0, 0, 0, 0])],
                io::ErrorKind::InvalidData,
            ),
        ];
        for (packets, expected) in cases {
            for writing in [false, true] {
                let mut stream = transcript(&packets);
                let mut keys = keys();
                let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, 0, 32).unwrap();
                let error = if writing {
                    channel.write(b"x")
                } else {
                    channel.read(&mut [0])
                }
                .unwrap_err();
                assert_eq!(
                    error.kind(),
                    expected,
                    "writing={writing}, packets={packets:?}"
                );
            }
        }
    }

    #[test]
    fn local_window_accepts_exact_capacity_and_rejects_one_more_byte() {
        let chunk = data(&vec![0; SSH_CHANNEL_MAX_PACKET]);
        let count = SSH_CHANNEL_WINDOW as usize / SSH_CHANNEL_MAX_PACKET;
        let mut packets = vec![chunk; count];
        packets.push(data(b"x"));
        let mut stream = transcript(&packets);
        let mut keys = keys();
        let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, 0, 32).unwrap();
        for _ in 0..count {
            channel.receive_packet().unwrap();
        }
        assert_eq!(channel.local_window, 0);
        assert_eq!(channel.pending.len(), SSH_CHANNEL_WINDOW as usize);
        assert_eq!(
            channel.receive_packet().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn window_limits_and_ignored_traffic_are_bounded() {
        for packet in [
            data(&vec![0; SSH_CHANNEL_MAX_PACKET + 1]),
            message(93, &1u32.to_be_bytes()),
        ] {
            let mut stream = transcript(&[packet]);
            let mut keys = keys();
            let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, u32::MAX, 32).unwrap();
            assert_eq!(
                channel.read(&mut [0]).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut stream = transcript(&[
            message(95, &[0, 0, 0, 1, 0, 0, 0, 1, b'x']),
            message(97, &[]),
        ]);
        let mut keys = keys();
        let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, 32, u32::MAX).unwrap();
        assert_eq!(channel.remote_max_packet, SSH_CHANNEL_MAX_PACKET);
        channel.received_bytes = MAX_RESPONSE_BYTES - 14;
        channel.receive_packet().unwrap();
        assert_eq!(channel.received_bytes, MAX_RESPONSE_BYTES);
        assert_eq!(channel.local_window, SSH_CHANNEL_WINDOW);
        assert_eq!(
            channel.receive_packet().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(channel.local_window, SSH_CHANNEL_WINDOW);
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn authenticated_channel_messages_preserve_window_invariants(
                packets in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..128), 0..16),
                window in any::<u32>(),
            ) {
                let mut stream = transcript(&packets);
                let mut keys = keys();
                let mut channel = SshExecChannel::new(&mut stream, &mut keys, 7, window, 32).unwrap();
                for _ in &packets {
                    if channel.receive_packet().is_err() || channel.closed {
                        break;
                    }
                    prop_assert_eq!(channel.local_window as usize + channel.pending.len(), SSH_CHANNEL_WINDOW as usize);
                    prop_assert!(channel.received_bytes <= MAX_RESPONSE_BYTES);
                }
            }
        }
    }
}
