//! Independent, bounded Apple 0x22 framing. Protocol evidence is pinned in the report.
use crate::error::{Error, Result};
use zeroize::Zeroizing;
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub const CHUNK: usize = 64 * 1024;
pub const START_SEND: u16 = 1;
pub const START_RECEIVE: u16 = 2;
pub const PAUSE: u16 = 3;
pub const CONTINUE: u16 = 4;
pub const STOP: u16 = 5;
pub const INFO: u16 = 100;
pub const ITEM: u16 = 101;
pub const DATA: u16 = 102;
pub const COMPRESSED: u16 = 103;
pub const END: u16 = 104;
pub const DONE: u16 = 200;
pub fn failed(reason: &str) -> Error {
    Error::Clipboard(format!("Native file transfer: {reason}"))
}
pub struct Message {
    pub command: u16,
    pub session: u32,
    pub body: Zeroizing<Vec<u8>>,
}
impl Message {
    pub fn new(command: u16, session: u32, body: Vec<u8>) -> Self {
        Self {
            command,
            session,
            body: Zeroizing::new(body),
        }
    }
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>> {
        if self.session == 0 || self.body.len() + 8 > 1024 * 1024 {
            return Err(failed("invalid outgoing file message"));
        }
        let mut out = Zeroizing::new(vec![0x22, 0]);
        out.extend_from_slice(&((self.body.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&self.command.to_be_bytes());
        out.extend_from_slice(&self.session.to_be_bytes());
        out.extend_from_slice(&self.body);
        Ok(out)
    }
}
#[derive(Default)]
pub struct Framer {
    header: Vec<u8>,
    body: Zeroizing<Vec<u8>>,
    remaining: Option<usize>,
    started: Option<std::time::Instant>,
    progress: Option<std::time::Instant>,
}
impl Framer {
    pub fn expired(&self, now: std::time::Instant) -> bool {
        self.started.is_some_and(|at| {
            now.saturating_duration_since(at) > std::time::Duration::from_secs(60)
        }) || self.progress.is_some_and(|at| {
            now.saturating_duration_since(at) > std::time::Duration::from_secs(10)
        })
    }
    pub fn active(&self) -> bool {
        !self.header.is_empty()
    }
    pub fn feed(&mut self, input: &[u8]) -> Result<(usize, Option<Message>)> {
        let mut used = 0;
        if !input.is_empty() {
            self.progress = Some(std::time::Instant::now());
        }
        if self.header.is_empty() {
            self.started = Some(std::time::Instant::now());
        }
        if self.header.len() < 14 {
            let n = (14 - self.header.len()).min(input.len());
            self.header.extend_from_slice(&input[..n]);
            used += n;
            if self.header.len() >= 6 {
                let size = u32::from_be_bytes(self.header[2..6].try_into().unwrap()) as usize;
                if self.header[0] != 0x22 || !(8..=MAX_FRAME).contains(&size) {
                    return Err(failed("invalid file frame length"));
                }
            }
            if self.header.len() < 14 {
                return Ok((used, None));
            }
            let size = u32::from_be_bytes(self.header[2..6].try_into().unwrap()) as usize;
            self.remaining = Some(size - 8);
        }
        let left = self.remaining.unwrap();
        let n = left.min(input.len() - used);
        self.body.extend_from_slice(&input[used..used + n]);
        used += n;
        self.remaining = Some(left - n);
        if left != n {
            return Ok((used, None));
        }
        // Consume a known-length bad message without confusing it with a video/control opcode.
        let valid = self.header[1] == 0 && self.header[6..8] == [0, 1];
        let session = u32::from_be_bytes(self.header[10..14].try_into().unwrap());
        let command = u16::from_be_bytes(self.header[8..10].try_into().unwrap());
        self.header.clear();
        self.remaining = None;
        self.started = None;
        self.progress = None;
        let body = std::mem::take(&mut self.body);
        if !valid || session == 0 {
            return Ok((used, None));
        }
        Ok((
            used,
            Some(Message {
                command,
                session,
                body,
            }),
        ))
    }
}
pub fn start(command: u16, session: u32, path: &str) -> Result<Message> {
    if path.len() > 32768 || path.contains('\0') || !path.starts_with('/') {
        return Err(failed("invalid remote path"));
    }
    let mut body = Vec::new();
    body.extend_from_slice(&(if command == START_SEND { 1u32 } else { 0 }).to_be_bytes());
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&(path.len() as u16).to_be_bytes());
    body.extend_from_slice(path.as_bytes());
    body.push(0);
    Ok(Message::new(command, session, body))
}
pub fn end(session: u32, status: u32) -> Message {
    Message::new(END, session, status.to_le_bytes().to_vec())
}
pub fn data(session: u32, bytes: &[u8]) -> Message {
    let mut body = (bytes.len() as u32).to_be_bytes().to_vec();
    body.extend_from_slice(bytes);
    Message::new(DATA, session, body)
}
pub fn data_body(body: &[u8]) -> Result<&[u8]> {
    if body.len() < 4 {
        return Err(failed("truncated data record"));
    }
    let n = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
    body.get(4..4 + n)
        .ok_or_else(|| failed("invalid data record length"))
}
pub fn done(body: &[u8]) -> Result<&str> {
    if body.len() < 4 || body[..2] != [0, 0] {
        return Err(failed("Mac rejected the transfer"));
    }
    let n = u16::from_be_bytes(body[2..4].try_into().unwrap()) as usize;
    std::str::from_utf8(
        body.get(4..4 + n)
            .ok_or_else(|| failed("invalid receiver result"))?,
    )
    .map_err(|_| failed("invalid receiver name"))
}
