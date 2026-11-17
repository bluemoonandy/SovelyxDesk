//! Apple pasteboard wire format, independently implemented from the documented protocol.
//! References and source revisions: reports/native-apple-clipboard-p2-b2.md.
//! This module has no sockets: callers must use the authenticated HPS Records channel.
use crate::error::{Error, Result};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_TEXT: usize = super::MAX_TEXT;
const MAX_ARCHIVE: usize = 8 * 1024 * 1024;
const MAX_COMPRESSED: usize = 4 * 1024 * 1024;
const SERVER_LIMIT: usize = 100 * 1024 * 1024;
const UTF8: &[u8] = b"public.utf8-plain-text";
const UTF16: &[u8] = b"public.utf16-plain-text";
fn invalid(reason: &'static str) -> Error {
    Error::Clipboard(format!("Apple native clipboard: {reason}"))
}
fn number(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("four-byte field"))
}
fn put(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn counted(out: &mut Vec<u8>, bytes: &[u8]) {
    put(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

pub fn monitor(enabled: bool) -> Vec<u8> {
    vec![0x15, 0, 0, if enabled { 1 } else { 2 }, 0, 0, 0, 0]
}
pub fn fetch(echo: u32) -> Vec<u8> {
    let mut out = vec![0x0b, 0, 0, 0];
    put(&mut out, echo);
    out
}

pub fn encode_text(text: &str) -> Result<Zeroizing<Vec<u8>>> {
    super::validate(text, true)?;
    if text.is_empty() {
        return Err(invalid("empty text is not shared"));
    }
    let mut archive = Zeroizing::new(Vec::with_capacity(text.len() + 64));
    put(&mut archive, 1);
    counted(&mut archive, UTF8);
    put(&mut archive, 0);
    put(&mut archive, 0);
    counted(&mut archive, text.as_bytes());
    let mut compressor = Compress::new(Compression::fast(), true);
    let mut compressed = Zeroizing::new(vec![0; archive.len() + archive.len() / 100 + 128]);
    compressor
        .compress(&archive, &mut compressed, FlushCompress::Sync)
        .map_err(|_| invalid("compression failed"))?;
    if compressor.total_in() != archive.len() as u64 {
        return Err(invalid("compression exceeded its bound"));
    }
    compressed.truncate(compressor.total_out() as usize);
    let mut out = Zeroizing::new(vec![0x1f, 0, 0, 0]);
    put(&mut out, 0);
    put(&mut out, archive.len() as u32);
    put(&mut out, compressed.len() as u32);
    out.extend_from_slice(&compressed);
    Ok(out)
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let result = self
            .0
            .get(..n)
            .ok_or_else(|| invalid("truncated archive"))?;
        self.0 = &self.0[n..];
        Ok(result)
    }
    fn num(&mut self) -> Result<usize> {
        Ok(number(self.take(4)?) as usize)
    }
    fn field(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = self.num()?;
        if n > max {
            return Err(invalid("archive field exceeds limit"));
        }
        self.take(n)
    }
}
fn archive_text(archive: &[u8]) -> Result<Option<Zeroizing<String>>> {
    let mut reader = Reader(archive);
    let mut preferred = None;
    let mut fallback = None;
    let mut items = 0;
    while !reader.0.is_empty() {
        items += 1;
        if items > 16 {
            return Err(invalid("too many pasteboard items"));
        }
        let flavors = reader.num()?;
        if flavors > 64 {
            return Err(invalid("too many pasteboard flavors"));
        }
        for _ in 0..flavors {
            let kind = reader.field(4096)?;
            let _flags = reader.num()?;
            let tags = reader.num()?;
            if tags > 64 {
                return Err(invalid("too many pasteboard tags"));
            }
            for _ in 0..tags {
                reader.field(4096)?;
                reader.field(4096)?;
            }
            let data = reader.field(MAX_ARCHIVE)?;
            if data.is_empty() {
                continue;
            }
            if kind == UTF8 && preferred.is_none() {
                if data.len() > MAX_TEXT {
                    return Err(invalid("text exceeds 1 MiB"));
                }
                let text = std::str::from_utf8(data).map_err(|_| invalid("invalid UTF-8"))?;
                super::validate(text, true)?;
                preferred = Some(Zeroizing::new(super::normalize(text)));
            } else if kind == UTF16 && fallback.is_none() {
                if data.len() > MAX_TEXT * 2 + 2 || !data.len().is_multiple_of(2) {
                    return Err(invalid("invalid UTF-16 size"));
                }
                let data = data.strip_prefix(&[0xfe, 0xff]).unwrap_or(data);
                let units = Zeroizing::new(
                    data.chunks_exact(2)
                        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                        .collect::<Vec<_>>(),
                );
                let text = Zeroizing::new(
                    String::from_utf16(&units).map_err(|_| invalid("invalid UTF-16"))?,
                );
                super::validate(&text, true)?;
                fallback = Some(Zeroizing::new(super::normalize(&text)));
            }
        }
    }
    Ok(preferred.or(fallback))
}
fn inflate(compressed: &[u8], size: usize) -> Result<Zeroizing<Vec<u8>>> {
    if size > MAX_ARCHIVE || compressed.len() > MAX_COMPRESSED {
        return Err(invalid("payload exceeds limit"));
    }
    if size == 0 && compressed.is_empty() {
        return Ok(Zeroizing::new(Vec::new()));
    }
    let mut out = Zeroizing::new(vec![0; size + 1]);
    let mut inflater = Decompress::new(true);
    let status = inflater
        .decompress(compressed, &mut out, FlushDecompress::Sync)
        .map_err(|_| invalid("invalid zlib stream"))?;
    if inflater.total_out() != size as u64
        || inflater.total_in() != compressed.len() as u64
        || (status != Status::StreamEnd && !compressed.ends_with(&[0, 0, 0xff, 0xff]))
    {
        return Err(invalid("zlib size or flush mismatch"));
    }
    out.truncate(size);
    Ok(out)
}

pub struct Packet {
    pub echo: u32,
    pub promise: bool,
    pub compressed: usize,
    pub uncompressed: usize,
    pub text: Result<Option<Zeroizing<String>>>,
}
#[derive(Default)]
pub struct Framer {
    header: Vec<u8>,
    body: Zeroizing<Vec<u8>>,
    remaining: Option<usize>,
    discard: bool,
    started: Option<Instant>,
}
impl Framer {
    pub fn active(&self) -> bool {
        !self.header.is_empty()
    }
    pub fn expired(&self, now: Instant) -> bool {
        self.started
            .is_some_and(|start| now.saturating_duration_since(start) > Duration::from_secs(10))
    }
    /// Consumes exactly one message, preserving a following opcode in the same record.
    /// Unsupported large pasteboards are drained without allocating their payload.
    pub fn feed(&mut self, input: &[u8]) -> Result<(usize, Option<Packet>)> {
        let mut used = 0;
        if self.header.len() < 16 {
            if self.header.is_empty() {
                self.started = Some(Instant::now());
            }
            let n = (16 - self.header.len()).min(input.len());
            self.header.extend_from_slice(&input[..n]);
            used += n;
            if self.header.len() < 16 {
                return Ok((used, None));
            }
            if self.header[0] != 0x1f {
                return Err(invalid("invalid pasteboard opcode"));
            }
            let packed = number(&self.header[12..16]) as usize;
            let unpacked = number(&self.header[8..12]) as usize;
            if packed > SERVER_LIMIT || unpacked > SERVER_LIMIT {
                return Err(invalid("implausible pasteboard framing"));
            }
            self.discard = packed > MAX_COMPRESSED
                || unpacked > MAX_ARCHIVE
                || self.header[1] != 0
                || self.header[3] != 0
                || self.header[2] & !1 != 0;
            self.remaining = Some(packed);
        }
        let remaining = self.remaining.unwrap_or(0);
        let n = remaining.min(input.len() - used);
        if !self.discard {
            self.body.extend_from_slice(&input[used..used + n]);
        }
        used += n;
        self.remaining = Some(remaining - n);
        if remaining != n {
            return Ok((used, None));
        }
        let echo = number(&self.header[4..8]);
        let uncompressed = number(&self.header[8..12]) as usize;
        let compressed = number(&self.header[12..16]) as usize;
        let promise = self.header[2] & 1 != 0;
        let text = if self.discard {
            Err(invalid("unsupported pasteboard size or flags"))
        } else if promise {
            Ok(None)
        } else {
            inflate(&self.body, uncompressed).and_then(|archive| archive_text(&archive))
        };
        self.body.zeroize();
        self.header.clear();
        self.remaining = None;
        self.started = None;
        self.discard = false;
        Ok((
            used,
            Some(Packet {
                echo,
                promise,
                compressed,
                uncompressed,
                text,
            }),
        ))
    }
}

pub enum Command {
    Configure {
        epoch: u64,
        enabled: bool,
        receive: bool,
        send: bool,
    },
    Fetch {
        epoch: u64,
        sequence: u32,
    },
    Send {
        epoch: u64,
        text: Zeroizing<String>,
    },
}
pub enum Event {
    Changed {
        epoch: u64,
    },
    SessionChanged {
        epoch: u64,
    },
    Snapshot {
        epoch: u64,
        sequence: u32,
        text: Option<Zeroizing<String>>,
    },
    Sent {
        epoch: u64,
    },
    Problem {
        epoch: u64,
        reason: &'static str,
    },
    Trace {
        opcode: u8,
        detail: &'static str,
        value: u32,
        packed: usize,
        unpacked: usize,
    },
}
impl Event {
    pub fn epoch(&self) -> Option<u64> {
        match self {
            Self::Changed { epoch }
            | Self::SessionChanged { epoch }
            | Self::Snapshot { epoch, .. }
            | Self::Sent { epoch }
            | Self::Problem { epoch, .. } => Some(*epoch),
            Self::Trace { .. } => None,
        }
    }
}
struct Pending {
    epoch: u64,
    echo: u32,
    sequence: u32,
    started: Instant,
}
#[derive(Default)]
pub struct Transport {
    epoch: u64,
    enabled: bool,
    receive: bool,
    send: bool,
    echo: u32,
    pending: Option<Pending>,
    dirty: bool,
    changed_queued: bool,
    zero_tainted: bool,
    events: VecDeque<Event>,
}
impl Transport {
    fn event(&mut self, event: Event) {
        if self.events.len() == 16 {
            if matches!(event, Event::Trace { .. }) {
                return;
            }
            let index = self
                .events
                .iter()
                .position(|event| matches!(event, Event::Trace { .. }))
                .unwrap_or(0);
            if matches!(self.events.remove(index), Some(Event::Changed { .. })) {
                self.changed_queued = false;
            }
        }
        self.events.push_back(event);
    }
    fn changed(&mut self) {
        if self.enabled && self.receive && !self.changed_queued && self.pending.is_none() {
            self.changed_queued = true;
            self.event(Event::Changed { epoch: self.epoch });
        }
    }
    pub fn take(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn command(&mut self, command: Command) -> Result<Option<Zeroizing<Vec<u8>>>> {
        match command {
            Command::Configure {
                epoch,
                enabled,
                receive,
                send,
            } => {
                self.epoch = epoch;
                self.enabled = enabled;
                self.receive = receive;
                self.send = send;
                self.dirty = false;
                self.changed_queued = false;
                self.events.clear();
                self.event(Event::Trace {
                    opcode: 0x15,
                    detail: "monitor",
                    value: if enabled { 1 } else { 2 },
                    packed: 0,
                    unpacked: 0,
                });
                self.changed();
                Ok(Some(Zeroizing::new(monitor(enabled))))
            }
            Command::Fetch { epoch, sequence } => {
                self.changed_queued = false;
                if !self.enabled || !self.receive || epoch != self.epoch || self.pending.is_some() {
                    return Ok(None);
                }
                self.echo = self
                    .echo
                    .checked_add(1)
                    .ok_or_else(|| invalid("request counter exhausted"))?;
                self.pending = Some(Pending {
                    epoch,
                    echo: self.echo,
                    sequence,
                    started: Instant::now(),
                });
                self.dirty = false;
                self.event(Event::Trace {
                    opcode: 0x0b,
                    detail: "fetch",
                    value: self.echo,
                    packed: 0,
                    unpacked: 0,
                });
                Ok(Some(Zeroizing::new(fetch(self.echo))))
            }
            Command::Send { epoch, text } => {
                if !self.enabled || !self.send || epoch != self.epoch {
                    return Ok(None);
                }
                let packet = encode_text(&text)?;
                Ok(Some(packet))
            }
        }
    }
    pub fn trace_transport(&mut self, opcode: u8, phase: &'static str, bytes: usize) {
        self.event(Event::Trace {
            opcode,
            detail: phase,
            value: bytes as u32,
            packed: 0,
            unpacked: 0,
        });
    }
    pub fn sent(&mut self, epoch: u64) {
        self.event(Event::Sent { epoch });
    }
    pub fn status(&mut self, command: u16) {
        if self.enabled || self.pending.is_some() {
            self.event(Event::Trace {
                opcode: 0x14,
                detail: "status",
                value: u32::from(command),
                packed: 0,
                unpacked: 0,
            });
        }
        if command == 2 {
            self.dirty = true;
            self.changed();
        }
        if matches!(command, 6 | 8 | 17) {
            self.session_changed();
        }
    }
    pub fn session_changed(&mut self) {
        if self.enabled {
            self.enabled = false;
            self.event(Event::SessionChanged { epoch: self.epoch });
        }
    }
    pub fn tick(&mut self, now: Instant) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| now.saturating_duration_since(p.started) > Duration::from_secs(5))
        {
            self.pending = None;
            self.zero_tainted = true;
            self.changed_queued = false;
            self.event(Event::Problem {
                epoch: self.epoch,
                reason: "fetch_timeout",
            });
        }
        if self.dirty {
            self.changed();
        }
    }
    pub fn packet(&mut self, packet: Packet) {
        self.event(Event::Trace {
            opcode: 0x1f,
            detail: if packet.promise { "promise" } else { "reply" },
            value: packet.echo,
            packed: packet.compressed,
            unpacked: packet.uncompressed,
        });
        let Some(pending) = self.pending.as_ref() else {
            self.changed();
            return;
        };
        // Zero is observed as a reserved value in some implementations. Accept it only
        // with one unambiguous request; after a timeout require an actual matching echo.
        if packet.echo != pending.echo && (packet.echo != 0 || self.zero_tainted) {
            self.event(Event::Problem {
                epoch: self.epoch,
                reason: "echo_mismatch",
            });
            return;
        }
        let pending = self.pending.take().unwrap();
        if pending.epoch == self.epoch && self.enabled && self.receive && !self.dirty {
            match packet.text {
                Ok(text) => self.event(Event::Snapshot {
                    epoch: self.epoch,
                    sequence: pending.sequence,
                    text,
                }),
                Err(_) => self.event(Event::Problem {
                    epoch: self.epoch,
                    reason: "invalid_archive",
                }),
            }
        }
        if self.dirty || pending.epoch != self.epoch {
            self.changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decode(packet: &[u8]) -> Packet {
        let mut f = Framer::default();
        f.feed(packet).unwrap().1.unwrap()
    }
    fn configured() -> Transport {
        let mut t = Transport::default();
        t.command(Command::Configure {
            epoch: 1,
            enabled: true,
            receive: true,
            send: true,
        })
        .unwrap();
        while t.take().is_some() {}
        t
    }
    fn request(t: &mut Transport, sequence: u32) -> u32 {
        t.command(Command::Fetch { epoch: 1, sequence }).unwrap();
        t.echo
    }
    #[test]
    fn fixed_messages_match_documented_bytes() {
        assert_eq!(monitor(true), [0x15, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(monitor(false), [0x15, 0, 0, 2, 0, 0, 0, 0]);
        assert_eq!(fetch(0x12345678), [0x0b, 0, 0, 0, 0x12, 0x34, 0x56, 0x78]);
    }
    #[test]
    fn unicode_archive_and_sync_flush_roundtrip() {
        for text in [
            "English",
            "中文🍎\n    code();\n\tindent",
            "literal $(not-a-command) ' \\",
        ] {
            let p = encode_text(text).unwrap();
            assert_eq!(&p[..8], &[0x1f, 0, 0, 0, 0, 0, 0, 0]);
            assert!(p.ends_with(&[0, 0, 0xff, 0xff]));
            assert_eq!(decode(&p).text.unwrap().unwrap().as_str(), text);
        }
        assert!(encode_text("").is_err());
        assert!(encode_text("bad\0text").is_err());
        assert!(encode_text(&"x".repeat(MAX_TEXT + 1)).is_err());
    }
    #[test]
    fn every_header_split_and_bytewise_payload_preserve_following_message() {
        let mut p = encode_text("中文🍎\ncode").unwrap();
        let length = p.len();
        p.extend_from_slice(&[0x14, 0, 0, 4, 0, 1, 0, 2]);
        for split in 1..length {
            let mut f = Framer::default();
            let (n, first) = f.feed(&p[..split]).unwrap();
            assert_eq!(n, split);
            assert!(first.is_none());
            let (n, packet) = f.feed(&p[split..]).unwrap();
            assert_eq!(n, length - split);
            assert_eq!(
                packet.unwrap().text.unwrap().unwrap().as_str(),
                "中文🍎\ncode"
            );
            assert!(!f.active());
        }
        let mut f = Framer::default();
        let mut result = None;
        for byte in &p[..length] {
            let (_, done) = f.feed(std::slice::from_ref(byte)).unwrap();
            if done.is_some() {
                result = done;
            }
        }
        assert!(result.is_some());
    }
    #[test]
    fn archive_counts_tags_utf16_and_trailing_items_are_checked() {
        let mut archive = Vec::new();
        put(&mut archive, 1);
        counted(&mut archive, UTF16);
        put(&mut archive, 0);
        put(&mut archive, 1);
        counted(&mut archive, b"com.apple.ostype");
        counted(&mut archive, b"utxt");
        let data = "中文🍎"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>();
        counted(&mut archive, &data);
        assert_eq!(archive_text(&archive).unwrap().unwrap().as_str(), "中文🍎");
        archive.push(0);
        assert!(archive_text(&archive).is_err());
        assert!(archive_text(&u32::MAX.to_be_bytes()).is_err());
        assert!(archive_text(&[0, 0, 0, 0]).unwrap().is_none());
    }
    #[test]
    fn malformed_zlib_and_size_mismatch_never_produce_text() {
        let mut p = encode_text("private fixture").unwrap();
        p[11] = p[11].wrapping_add(1);
        assert!(decode(&p).text.is_err());
        let mut p = encode_text("fixture").unwrap();
        *p.last_mut().unwrap() ^= 1;
        assert!(decode(&p).text.is_err());
        let mut p = encode_text("fixture").unwrap();
        p[2] = 1;
        assert!(decode(&p).text.unwrap().is_none());
        let mut p = encode_text("fixture").unwrap();
        p[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(Framer::default().feed(&p).is_err());
    }
    #[test]
    fn oversized_pasteboards_are_drained_with_bounded_memory() {
        let mut header = vec![0x1f, 0, 0, 0];
        put(&mut header, 0);
        put(&mut header, MAX_ARCHIVE as u32 + 1);
        put(&mut header, 100);
        let mut f = Framer::default();
        assert!(f.feed(&header).unwrap().1.is_none());
        assert!(f.feed(&[1; 99]).unwrap().1.is_none());
        let (used, done) = f.feed(&[1, 0x14]).unwrap();
        assert_eq!(used, 1);
        assert!(done.unwrap().text.is_err());
        assert!(f.body.capacity() < 1024);
    }
    #[test]
    fn disabled_and_wrong_epoch_never_send_or_fetch() {
        let mut t = Transport::default();
        assert!(
            t.command(Command::Fetch {
                epoch: 0,
                sequence: 1
            })
            .unwrap()
            .is_none()
        );
        assert!(
            t.command(Command::Send {
                epoch: 0,
                text: Zeroizing::new("text".into())
            })
            .unwrap()
            .is_none()
        );
        let mut t = configured();
        assert!(
            t.command(Command::Send {
                epoch: 2,
                text: Zeroizing::new("text".into())
            })
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn request_receipt_carries_exact_local_sequence_and_epoch() {
        let mut t = configured();
        let echo = request(&mut t, 77);
        let mut p = decode(&encode_text("remote").unwrap());
        p.echo = echo;
        t.packet(p);
        assert!(t.events.iter().any(|event|matches!(event,Event::Snapshot{epoch:1,sequence:77,text:Some(text)} if text.as_str()=="remote")));
    }
    #[test]
    fn changed_during_fetch_coalesces_and_drops_older_snapshot() {
        let mut t = configured();
        request(&mut t, 10);
        for _ in 0..50 {
            t.status(2);
        }
        t.packet(decode(&encode_text("old").unwrap()));
        assert!(
            !t.events
                .iter()
                .any(|event| matches!(event, Event::Snapshot { .. }))
        );
        assert!(
            t.events
                .iter()
                .any(|event| matches!(event, Event::Changed { epoch: 1 }))
        );
    }
    #[test]
    fn disconnect_disable_reenable_does_not_mislabel_an_old_reply() {
        let mut t = configured();
        request(&mut t, 10);
        t.command(Command::Configure {
            epoch: 2,
            enabled: true,
            receive: true,
            send: false,
        })
        .unwrap();
        t.packet(decode(&encode_text("old session").unwrap()));
        assert!(
            !t.events
                .iter()
                .any(|event| matches!(event, Event::Snapshot { .. }))
        );
        assert!(
            t.events
                .iter()
                .any(|event| matches!(event, Event::Changed { epoch: 2 }))
        );
    }
    #[test]
    fn pointer_visibility_is_not_a_session_change() {
        let mut t = configured();
        t.status(11);
        t.status(12);
        assert!(t.enabled);
        assert!(
            !t.events
                .iter()
                .any(|e| matches!(e, Event::SessionChanged { .. }))
        );
    }
    #[test]
    fn timeout_late_zero_echo_is_rejected_and_status17_rearms_via_ui() {
        let mut t = configured();
        request(&mut t, 10);
        t.tick(Instant::now() + Duration::from_secs(6));
        request(&mut t, 11);
        t.packet(decode(&encode_text("late").unwrap()));
        assert!(
            !t.events
                .iter()
                .any(|event| matches!(event, Event::Snapshot { .. }))
        );
        t.status(17);
        assert!(
            t.events
                .iter()
                .any(|event| matches!(event, Event::SessionChanged { epoch: 1 }))
        );
    }
}
