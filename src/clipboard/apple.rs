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
const MAX_ARCHIVE: usize = 36 * 1024 * 1024;
const MAX_COMPRESSED: usize = 4 * 1024 * 1024;
const SERVER_LIMIT: usize = 100 * 1024 * 1024;
const TIFF: &[u8] = b"public.tiff";
const HTML: &[u8] = b"public.html";
const RTF: &[u8] = b"public.rtf";
const PNG: &[u8] = b"public.png";
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
    encode_flavor(UTF8, text.as_bytes())
}
pub fn encode_image(png: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    super::image::validate_png(png)?;
    encode_flavor(PNG, png)
}
fn encode_flavor(kind: &[u8], data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    encode_flavors(&[(kind, data)])
}
pub fn encode_rich(rich: &super::rich::RichText) -> Result<Zeroizing<Vec<u8>>> {
    rich.validate()?;
    let mut flavors = Vec::new();
    if let Some(plain) = &rich.plain {
        flavors.push((UTF8, plain.as_bytes()));
    }
    if let Some(html) = &rich.html {
        flavors.push((HTML, html.as_bytes()));
    }
    if let Some(rtf) = &rich.rtf {
        flavors.push((RTF, rtf.as_slice()));
    }
    encode_flavors(&flavors)
}
fn encode_flavors(flavors: &[(&[u8], &[u8])]) -> Result<Zeroizing<Vec<u8>>> {
    let mut archive = Zeroizing::new(Vec::new());
    put(&mut archive, flavors.len() as u32);
    for (kind, data) in flavors {
        counted(&mut archive, kind);
        put(&mut archive, 0);
        put(&mut archive, 0);
        counted(&mut archive, data);
    }
    if archive.len() > MAX_ARCHIVE {
        return Err(invalid("archive exceeds limit"));
    }
    compress_archive(&archive)
}
fn compress_archive(archive: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let mut compressor = Compress::new(Compression::fast(), true);
    let mut compressed = Zeroizing::new(vec![0; archive.len() + archive.len() / 100 + 128]);
    compressor
        .compress(archive, &mut compressed, FlushCompress::Sync)
        .map_err(|_| invalid("compression failed"))?;
    if compressor.total_in() != archive.len() as u64 {
        return Err(invalid("compression exceeded its bound"));
    }
    compressed.truncate(compressor.total_out() as usize);
    if compressed.len() > MAX_COMPRESSED {
        return Err(invalid("compressed payload exceeds limit"));
    }
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
#[derive(Default)]
struct ArchiveContent {
    text: Option<Zeroizing<String>>,
    image: Option<Zeroizing<Vec<u8>>>,
    tiff: Option<Zeroizing<Vec<u8>>>,
    rich: Option<super::rich::RichText>,
    files: Vec<String>,
}
fn archive_content(archive: &[u8]) -> Result<ArchiveContent> {
    let mut reader = Reader(archive);
    let mut preferred = None;
    let mut fallback = None;
    let mut image = None;
    let mut file_copy = false;
    let mut files = Vec::new();
    let mut tiff = None;
    let mut rich = super::rich::RichText::default();
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
            file_copy |= kind == b"public.file-url"
                || kind == b"NSFilenamesPboardType"
                || kind.starts_with(b"com.apple.pasteboard.promised-file");
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
            if kind == b"public.file-url" && !data.is_empty() {
                files.push(super::filecopy::urls::decode(data)?);
            }
            if data.is_empty() {
                continue;
            }
            if kind == PNG && image.is_none() {
                super::image::validate_png(data)?;
                image = Some(Zeroizing::new(data.to_vec()));
            } else if kind == TIFF && tiff.is_none() {
                super::image::validate_tiff(data)?;
                tiff = Some(Zeroizing::new(data.to_vec()));
            } else if kind == HTML && rich.html.is_none() {
                rich.html = super::rich::html(data).ok();
            } else if kind == RTF && rich.rtf.is_none() {
                if super::rich::validate_rtf(data).is_ok() {
                    rich.rtf = Some(Zeroizing::new(data.to_vec()));
                }
            } else if kind == UTF8 && preferred.is_none() {
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
    // A file's path/preview is not the file itself. Preserve the destination clipboard.
    if file_copy {
        return Ok(ArchiveContent {
            files,
            ..Default::default()
        });
    }
    Ok(ArchiveContent {
        text: preferred.or(fallback),
        image,
        tiff,
        rich: (rich.html.is_some() || rich.rtf.is_some()).then_some(rich),
        files: Vec::new(),
    })
}
#[cfg(test)]
fn archive_text(archive: &[u8]) -> Result<Option<Zeroizing<String>>> {
    archive_content(archive).map(|content| content.text)
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
    pub image: Option<Zeroizing<Vec<u8>>>,
    pub tiff: Option<Zeroizing<Vec<u8>>>,
    pub rich: Option<super::rich::RichText>,
    pub files: Vec<String>,
}
#[derive(Default)]
pub struct Framer {
    header: Vec<u8>,
    body: Zeroizing<Vec<u8>>,
    remaining: Option<usize>,
    discard: bool,
    started: Option<Instant>,
    progress: Option<Instant>,
}
impl Framer {
    pub fn active(&self) -> bool {
        !self.header.is_empty()
    }
    pub fn expired(&self, now: Instant) -> bool {
        self.started
            .is_some_and(|start| now.saturating_duration_since(start) > Duration::from_secs(60))
            || self
                .progress
                .is_some_and(|last| now.saturating_duration_since(last) > Duration::from_secs(10))
    }
    /// Consumes exactly one message, preserving a following opcode in the same record.
    /// Unsupported large pasteboards are drained without allocating their payload.
    pub fn feed(&mut self, input: &[u8]) -> Result<(usize, Option<Packet>)> {
        let mut used = 0;
        if !input.is_empty() {
            self.progress = Some(Instant::now());
        }
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
        let content = if self.discard {
            Err(invalid("unsupported pasteboard size or flags"))
        } else {
            // A top-level promise can still include materialized text flavors.
            // Only a flavor with no actual data is a promise we cannot apply.
            inflate(&self.body, uncompressed).and_then(|archive| archive_content(&archive))
        };
        let (text, image, tiff, rich, files) = match content {
            Ok(content) => (
                Ok(content.text),
                content.image,
                content.tiff,
                content.rich,
                content.files,
            ),
            Err(error) => (Err(error), None, None, None, Vec::new()),
        };
        self.body.zeroize();
        self.header.clear();
        self.remaining = None;
        self.started = None;
        self.progress = None;
        self.discard = false;
        Ok((
            used,
            Some(Packet {
                echo,
                promise,
                compressed,
                uncompressed,
                text,
                image,
                tiff,
                rich,
                files,
            }),
        ))
    }
}

/// Prepared off the HEVC/UI threads; callers cannot forge an unvalidated wire packet.
pub struct ImageMessage(Zeroizing<Vec<u8>>);
impl ImageMessage {
    pub fn new(png: &[u8]) -> Result<Self> {
        encode_image(png).map(Self)
    }
}
pub struct RichMessage(Zeroizing<Vec<u8>>);
impl RichMessage {
    pub fn new(rich: &super::rich::RichText) -> Result<Self> {
        encode_rich(rich).map(Self)
    }
}
pub struct FileMessage(Zeroizing<Vec<u8>>);
impl FileMessage {
    pub fn new(paths: &[String]) -> Result<Self> {
        if paths.is_empty() || paths.len() > 16 {
            return Err(invalid("invalid file root count"));
        }
        let mut archive = Zeroizing::new(Vec::new());
        for path in paths {
            let uri = super::filecopy::urls::encode(path)?;
            put(&mut archive, 1);
            counted(&mut archive, b"public.file-url");
            put(&mut archive, 0);
            put(&mut archive, 0);
            counted(&mut archive, uri.as_bytes());
        }
        compress_archive(&archive).map(Self)
    }
}
pub enum Command {
    SendFiles {
        epoch: u64,
        message: FileMessage,
    },
    SendRich {
        epoch: u64,
        message: RichMessage,
    },
    SendImage {
        epoch: u64,
        message: ImageMessage,
    },
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
    FilesSnapshot {
        epoch: u64,
        sequence: u32,
        paths: Vec<String>,
    },
    TiffSnapshot {
        epoch: u64,
        sequence: u32,
        tiff: Zeroizing<Vec<u8>>,
    },
    RichSnapshot {
        epoch: u64,
        sequence: u32,
        rich: super::rich::RichText,
    },
    ImageSnapshot {
        epoch: u64,
        sequence: u32,
        png: Zeroizing<Vec<u8>>,
    },
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
    pub fn is_snapshot(&self) -> bool {
        matches!(
            self,
            Self::FilesSnapshot { .. }
                | Self::Snapshot { .. }
                | Self::ImageSnapshot { .. }
                | Self::TiffSnapshot { .. }
                | Self::RichSnapshot { .. }
        )
    }
    pub fn epoch(&self) -> Option<u64> {
        match self {
            Self::FilesSnapshot { epoch, .. }
            | Self::Changed { epoch }
            | Self::SessionChanged { epoch }
            | Self::TiffSnapshot { epoch, .. }
            | Self::RichSnapshot { epoch, .. }
            | Self::ImageSnapshot { epoch, .. }
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
/// Apple extended ServerInit: two-byte prefix, flags, then 16-byte MSB-first command mask.
/// Only capability bits are retained; the host name is neither retained nor logged here.
pub fn file_capability(server_name: &[u8]) -> Option<(bool, bool)> {
    if server_name.len() < 22 || !server_name.starts_with(&[0, 0]) {
        return None;
    }
    let flags = u32::from_be_bytes(server_name[2..6].try_into().unwrap());
    Some((
        server_name[6 + 0x22 / 8] & (0x80 >> (0x22 % 8)) != 0,
        flags & 1 != 0,
    ))
}
#[derive(Default)]
pub struct Transport {
    file_capability: Option<(bool, bool)>,
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
    pub fn file_capability_bits(&self) -> u32 {
        self.file_capability.map_or(0, |(supported, observe_only)| {
            1 | ((supported as u32) << 1) | ((observe_only as u32) << 2)
        })
    }
    pub fn from_server_name(name: &[u8]) -> Self {
        Self {
            file_capability: file_capability(name),
            ..Default::default()
        }
    }
    fn event(&mut self, event: Event) {
        // TIFF can be much larger than text: keep one latest payload, not sixteen images.
        if event.is_snapshot() {
            self.events.retain(|queued| !queued.is_snapshot());
        }
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
                if enabled {
                    let bits = self.file_capability.map_or(0, |(supported, observe_only)| {
                        1 | ((supported as u32) << 1) | ((observe_only as u32) << 2)
                    });
                    self.event(Event::Trace {
                        opcode: 0x22,
                        detail: "file_copy_capability",
                        value: bits,
                        packed: 0,
                        unpacked: 0,
                    });
                }
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
            Command::SendFiles { epoch, message } => {
                if !self.enabled || !self.send || epoch != self.epoch {
                    return Ok(None);
                }
                Ok(Some(message.0))
            }
            Command::SendRich { epoch, message } => {
                if !self.enabled || !self.send || epoch != self.epoch {
                    return Ok(None);
                }
                Ok(Some(message.0))
            }
            Command::SendImage { epoch, message } => {
                if !self.enabled || !self.send || epoch != self.epoch {
                    return Ok(None);
                }
                Ok(Some(message.0))
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
        if command == 17 {
            self.file_capability = None;
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
            let superseded = self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.epoch != self.epoch);
            self.pending = None;
            self.zero_tainted = true;
            self.changed_queued = false;
            if superseded {
                self.event(Event::Trace {
                    opcode: 0x0b,
                    detail: "superseded_fetch_expired",
                    value: 0,
                    packed: 0,
                    unpacked: 0,
                });
                self.changed();
            } else {
                self.event(Event::Problem {
                    epoch: self.epoch,
                    reason: "fetch_timeout",
                });
            }
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
                Ok(_) if !packet.files.is_empty() => self.event(Event::FilesSnapshot {
                    epoch: self.epoch,
                    sequence: pending.sequence,
                    paths: packet.files,
                }),
                Ok(text)
                    if packet.rich.is_some()
                        && text.as_ref().is_some_and(|plain| !plain.is_empty()) =>
                {
                    let mut rich = packet.rich.unwrap();
                    rich.plain = text;
                    self.event(Event::RichSnapshot {
                        epoch: self.epoch,
                        sequence: pending.sequence,
                        rich,
                    });
                }
                Ok(_) if packet.image.is_some() => self.event(Event::ImageSnapshot {
                    epoch: self.epoch,
                    sequence: pending.sequence,
                    png: packet.image.unwrap(),
                }),
                Ok(_) if packet.tiff.is_some() => self.event(Event::TiffSnapshot {
                    epoch: self.epoch,
                    sequence: pending.sequence,
                    tiff: packet.tiff.unwrap(),
                }),
                Ok(text) if packet.rich.is_some() => {
                    let mut rich = packet.rich.unwrap();
                    rich.plain = text;
                    self.event(Event::RichSnapshot {
                        epoch: self.epoch,
                        sequence: pending.sequence,
                        rich,
                    });
                }
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
    fn png_fixture() -> Zeroizing<Vec<u8>> {
        let mut dib = vec![0; 44];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1u32.to_le_bytes());
        dib[8..12].copy_from_slice(&1u32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib[40..43].copy_from_slice(&[0, 0, 255]);
        super::super::image::dib_to_png(&dib).unwrap()
    }
    fn rich_fixture() -> super::super::rich::RichText {
        super::super::rich::RichText {
            plain: Some(Zeroizing::new("中文🍎".into())),
            html: Some(Zeroizing::new("<b>中文🍎</b>".into())),
            rtf: Some(Zeroizing::new(b"{\\rtf1 bold}".to_vec())),
        }
    }
    #[test]
    fn user_session_change_revokes_cached_file_capability_until_new_handshake() {
        let mut name = vec![0; 22];
        name[10] = 0x20;
        let mut transport = Transport::from_server_name(&name);
        assert_eq!(transport.file_capability_bits(), 3);
        transport.status(17);
        assert_eq!(transport.file_capability_bits(), 0);
    }
    #[test]
    fn file_url_items_preserve_each_root_and_stale_epochs_never_send_them() {
        let paths = vec![
            "/tmp/maccanvas-fixture/中文 a.txt".into(),
            "/tmp/maccanvas-fixture/folder".into(),
        ];
        let message = FileMessage::new(&paths).unwrap();
        let packet = decode(&message.0);
        assert_eq!(packet.files, paths);
        assert!(packet.text.unwrap().is_none());
        let mut transport = Transport::default();
        assert!(
            transport
                .command(Command::SendFiles {
                    epoch: 1,
                    message: FileMessage::new(&paths).unwrap()
                })
                .unwrap()
                .is_none()
        );
        transport
            .command(Command::Configure {
                epoch: 1,
                enabled: true,
                receive: true,
                send: true,
            })
            .unwrap();
        transport
            .command(Command::Fetch {
                epoch: 1,
                sequence: 23,
            })
            .unwrap();
        transport.packet(decode(&FileMessage::new(&paths).unwrap().0));
        assert!(transport.events.iter().any(|event|matches!(event,Event::FilesSnapshot{epoch:1,sequence:23,paths:received} if received==&paths)));
    }
    #[test]
    fn native_file_capability_uses_server_mask_without_sending_a_file_command() {
        let mut name = vec![0; 22];
        name[10] = 0x20;
        assert_eq!(file_capability(&name), Some((true, false)));
        name[5] = 1;
        assert_eq!(file_capability(&name), Some((true, true)));
        name[10] = 0x10;
        assert_eq!(file_capability(&name), Some((false, true)));
        assert_eq!(file_capability(b"ordinary Mac name"), None);
        let mut transport = Transport::from_server_name(&name);
        assert_eq!(
            &*transport
                .command(Command::Configure {
                    epoch: 1,
                    enabled: true,
                    receive: true,
                    send: true
                })
                .unwrap()
                .unwrap(),
            &monitor(true)
        );
        assert!(transport.events.iter().any(|event| matches!(
            event,
            Event::Trace {
                opcode: 0x22,
                detail: "file_copy_capability",
                value: 5,
                ..
            }
        )));
    }
    #[test]
    fn snapshot_queue_coalesces_images_rich_and_text_while_preserving_state() {
        let mut transport = Transport::default();
        transport.event(Event::SessionChanged { epoch: 1 });
        transport.event(Event::TiffSnapshot {
            epoch: 1,
            sequence: 1,
            tiff: Zeroizing::new(vec![0; 8]),
        });
        transport.event(Event::RichSnapshot {
            epoch: 1,
            sequence: 2,
            rich: rich_fixture(),
        });
        transport.event(Event::Snapshot {
            epoch: 1,
            sequence: 3,
            text: Some(Zeroizing::new("latest".into())),
        });
        assert_eq!(transport.events.len(), 2);
        assert!(matches!(
            transport.events.front(),
            Some(Event::SessionChanged { epoch: 1 })
        ));
        assert!(matches!(
            transport.events.back(),
            Some(Event::Snapshot { sequence: 3, .. })
        ));
    }
    #[test]
    fn rich_flavors_keep_plain_fallback_and_use_authorized_epoch() {
        let rich = rich_fixture();
        let wire = encode_rich(&rich).unwrap();
        let packet = decode(&wire);
        assert_eq!(&**packet.text.as_ref().unwrap().as_ref().unwrap(), "中文🍎");
        assert_eq!(
            &**packet.rich.as_ref().unwrap().html.as_ref().unwrap(),
            "<b>中文🍎</b>"
        );
        assert_eq!(
            &**packet.rich.as_ref().unwrap().rtf.as_ref().unwrap(),
            b"{\\rtf1 bold}"
        );
        let mut transport = Transport::default();
        assert!(
            transport
                .command(Command::SendRich {
                    epoch: 1,
                    message: RichMessage::new(&rich).unwrap()
                })
                .unwrap()
                .is_none()
        );
        transport
            .command(Command::Configure {
                epoch: 1,
                enabled: true,
                receive: true,
                send: true,
            })
            .unwrap();
        transport
            .command(Command::Fetch {
                epoch: 1,
                sequence: 7,
            })
            .unwrap();
        transport.packet(decode(&wire));
        assert!(transport.events.iter().any(|event|matches!(event,Event::RichSnapshot{epoch:1,sequence:7,rich} if rich.plain.as_deref().map(|s|s.as_str())==Some("中文🍎"))));
        assert!(
            transport
                .command(Command::SendRich {
                    epoch: 2,
                    message: RichMessage::new(&rich).unwrap()
                })
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn invalid_rich_does_not_destroy_valid_plain_fallback() {
        let wire = encode_flavors(&[
            (UTF8, b"plain"),
            (HTML, b"invalid\0html"),
            (RTF, b"invalid rtf"),
        ])
        .unwrap();
        let packet = decode(&wire);
        assert_eq!(&**packet.text.unwrap().as_ref().unwrap(), "plain");
        assert!(packet.rich.is_none());
    }
    #[test]
    fn tiff_flavor_is_framed_without_decoding_on_the_video_thread() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut cursor).unwrap();
            encoder
                .write_image::<tiff::encoder::colortype::RGB8>(1, 1, &[255, 0, 0])
                .unwrap();
        }
        let wire = encode_flavors(&[(TIFF, cursor.get_ref())]).unwrap();
        let packet = decode(&wire);
        assert_eq!(&**packet.tiff.as_ref().unwrap(), cursor.get_ref());
        assert!(packet.image.is_none());
        let mut transport = Transport::default();
        transport
            .command(Command::Configure {
                epoch: 1,
                enabled: true,
                receive: true,
                send: false,
            })
            .unwrap();
        transport
            .command(Command::Fetch {
                epoch: 1,
                sequence: 9,
            })
            .unwrap();
        transport.packet(decode(&wire));
        assert!(transport.events.iter().any(|e| matches!(
            e,
            Event::TiffSnapshot {
                epoch: 1,
                sequence: 9,
                ..
            }
        )));
    }
    #[test]
    fn png_flavor_uses_existing_archive_and_survives_encrypted_record_boundaries() {
        let png = png_fixture();
        let wire = encode_image(&png).unwrap();
        let mut framer = Framer::default();
        let mut complete = None;
        for chunk in wire.chunks(7) {
            let (used, packet) = framer.feed(chunk).unwrap();
            assert_eq!(used, chunk.len());
            if packet.is_some() {
                complete = packet;
            }
        }
        let packet = complete.unwrap();
        assert!(packet.text.unwrap().is_none());
        assert_eq!(&**packet.image.as_ref().unwrap(), &*png);
        let mut transport = Transport::default();
        transport
            .command(Command::Configure {
                epoch: 1,
                enabled: true,
                receive: true,
                send: true,
            })
            .unwrap();
        transport
            .command(Command::Fetch {
                epoch: 1,
                sequence: 77,
            })
            .unwrap();
        transport.packet(decode(&wire));
        assert!(transport.events.iter().any(|event|matches!(event,Event::ImageSnapshot{epoch:1,sequence:77,png:data} if **data==*png)));
        let mut transport = Transport::default();
        assert!(
            transport
                .command(Command::SendImage {
                    epoch: 1,
                    message: ImageMessage::new(&png).unwrap()
                })
                .unwrap()
                .is_none()
        );
        transport
            .command(Command::Configure {
                epoch: 2,
                enabled: true,
                receive: false,
                send: true,
            })
            .unwrap();
        assert!(
            transport
                .command(Command::SendImage {
                    epoch: 1,
                    message: ImageMessage::new(&png).unwrap()
                })
                .unwrap()
                .is_none()
        );
        assert!(
            transport
                .command(Command::SendImage {
                    epoch: 2,
                    message: ImageMessage::new(&png).unwrap()
                })
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn file_paths_and_preview_images_never_masquerade_as_file_transfer() {
        let png = png_fixture();
        let mut archive = Vec::new();
        put(&mut archive, 3);
        for (kind, data) in [
            (
                b"public.file-url".as_slice(),
                b"file:///fixture.txt".as_slice(),
            ),
            (UTF8, b"fixture.txt".as_slice()),
            (PNG, png.as_slice()),
        ] {
            counted(&mut archive, kind);
            put(&mut archive, 0);
            put(&mut archive, 0);
            counted(&mut archive, data);
        }
        let content = archive_content(&archive).unwrap();
        let (text, image) = (content.text, content.image);
        assert!(text.is_none());
        assert!(image.is_none());
    }
    #[test]
    fn png_and_text_flavors_are_parsed_without_treating_images_as_text() {
        let png = png_fixture();
        let mut archive = Vec::new();
        put(&mut archive, 2);
        for (kind, data) in [(UTF8, b"caption".as_slice()), (PNG, png.as_slice())] {
            counted(&mut archive, kind);
            put(&mut archive, 0);
            put(&mut archive, 0);
            counted(&mut archive, data);
        }
        let content = archive_content(&archive).unwrap();
        let (text, image) = (content.text, content.image);
        assert_eq!(&**text.as_ref().unwrap(), "caption");
        assert_eq!(&**image.as_ref().unwrap(), &*png);
        assert!(encode_image(b"invalid").is_err());
        let mut invalid = png.to_vec();
        invalid[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(encode_image(&invalid).is_err());
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
        assert_eq!(decode(&p).text.unwrap().unwrap().as_str(), "fixture");
        let mut promised_flavor = Vec::new();
        put(&mut promised_flavor, 1);
        counted(&mut promised_flavor, UTF8);
        put(&mut promised_flavor, 0);
        put(&mut promised_flavor, 0);
        put(&mut promised_flavor, 0);
        assert!(archive_text(&promised_flavor).unwrap().is_none());
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
    fn a_large_discard_in_progress_does_not_abort_the_video_control_session() {
        let mut header = vec![0x1f, 0, 0, 0];
        put(&mut header, 0);
        put(&mut header, (MAX_ARCHIVE + 1) as u32);
        put(&mut header, 100);
        let mut f = Framer::default();
        f.feed(&header).unwrap();
        let now = Instant::now();
        f.started = Some(now - Duration::from_secs(11));
        f.progress = Some(now);
        assert!(!f.expired(now));
        f.progress = Some(now - Duration::from_secs(11));
        assert!(f.expired(now));
        f.progress = Some(now);
        f.started = Some(now - Duration::from_secs(61));
        assert!(f.expired(now));
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
    fn an_expired_old_epoch_does_not_pause_the_new_session() {
        let mut t = configured();
        request(&mut t, 10);
        t.command(Command::Configure {
            epoch: 2,
            enabled: true,
            receive: true,
            send: false,
        })
        .unwrap();
        t.tick(Instant::now() + Duration::from_secs(6));
        assert!(
            t.events
                .iter()
                .any(|e| matches!(e, Event::Changed { epoch: 2 }))
        );
        assert!(!t.events.iter().any(|e| matches!(
            e,
            Event::Problem {
                reason: "fetch_timeout",
                ..
            }
        )));
        t.command(Command::Fetch {
            epoch: 2,
            sequence: 20,
        })
        .unwrap();
        let mut packet = decode(&encode_text("new session").unwrap());
        packet.echo = t.echo;
        t.packet(packet);
        assert!(t.events.iter().any(|e| matches!(
            e,
            Event::Snapshot {
                epoch: 2,
                sequence: 20,
                ..
            }
        )));
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
