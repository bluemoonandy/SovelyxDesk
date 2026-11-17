use std::{
    collections::VecDeque,
    net::{Shutdown, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    audit::{self, Level, Scope},
    credentials::{self, AuditedConsent, AuditedVault, CredentialSource, CredentialStore},
    error::{Error, Result},
    framebuffer::Framebuffer,
    input::Command,
    profiles::Profile,
    protocol::{self, Decoder, Message},
    streaming::StreamSettings,
    telemetry::{Counters, Metered},
};
use serde_json::json;
use zeroize::Zeroizing;

mod hevc;

fn reconnect_after_connection(hevc: bool, cancelled: bool, outcome: &Result<()>) -> bool {
    hevc && !cancelled && matches!(outcome, Err(Error::Io(_) | Error::ConnectionLost(_)))
}

pub type Frames = Arc<Mutex<Option<Arc<Framebuffer>>>>;

pub enum Event {
    Connected {
        profile: Profile,
        server_name: String,
        credentials_saved: bool,
    },
    Warning(String),
    Audio(crate::audio::Status),
    Cursor {
        update: crate::apple_hps::cursor::Update,
        point_scale: f32,
    },
    Error(String),
    Disconnected {
        reconnect: bool,
    },
}

pub struct Request {
    pub profile: Profile,
    pub credentials: CredentialSource,
    pub streaming: StreamSettings,
    pub audio: crate::audio::Settings,
    pub remember: bool,
    pub window: isize,
    pub generation: u64,
    pub audit: Scope,
    pub clipboard: bool,
}

struct Control {
    cancelled: AtomicBool,
    socket: Mutex<Option<TcpStream>>,
    gpu_frame: Mutex<Option<Arc<crate::render::gpu::nvdec::Frame>>>,
    frame_notifier: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    clipboard_enabled: AtomicBool,
    clipboard_utf8: AtomicBool,
    clipboard_pending: AtomicUsize,
    clipboard_text: Mutex<Option<Zeroizing<String>>>,
    native_clipboard: Mutex<VecDeque<crate::clipboard::apple::Event>>,
    native_epoch: AtomicU64,
    native_enabled: AtomicBool,
}

impl Control {
    fn configure_native(&self, epoch: u64, enabled: bool) -> Result<()> {
        self.native_enabled.store(false, Ordering::Release);
        self.native_epoch.store(epoch, Ordering::Release);
        self.native_clipboard
            .lock()
            .map_err(|_| Error::Clipboard("Native clipboard queue unavailable".into()))?
            .clear();
        self.native_enabled.store(enabled, Ordering::Release);
        Ok(())
    }
    fn accepts_native(&self, epoch: u64) -> bool {
        self.native_enabled.load(Ordering::Acquire)
            && self.native_epoch.load(Ordering::Acquire) == epoch
    }
    fn push_native(&self, event: crate::clipboard::apple::Event) -> Result<()> {
        let mut queue = self
            .native_clipboard
            .lock()
            .map_err(|_| Error::Clipboard("Native clipboard queue unavailable".into()))?;
        // Check after acquiring the lock: revocation can happen while the worker is decoding.
        if event
            .epoch()
            .is_some_and(|epoch| self.accepts_native(epoch))
        {
            if queue.len() == 16 {
                queue.pop_front();
            }
            queue.push_back(event);
        }
        Ok(())
    }
    fn new(clipboard: bool) -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            socket: Mutex::new(None),
            gpu_frame: Mutex::new(None),
            frame_notifier: Mutex::new(None),
            clipboard_enabled: AtomicBool::new(clipboard),
            clipboard_utf8: AtomicBool::new(false),
            clipboard_pending: AtomicUsize::new(0),
            clipboard_text: Mutex::new(None),
            native_clipboard: Mutex::new(VecDeque::new()),
            native_epoch: AtomicU64::new(0),
            native_enabled: AtomicBool::new(false),
        }
    }

    fn close(&self) -> Result<()> {
        self.cancelled.store(true, Ordering::Release);
        let socket = self
            .socket
            .lock()
            .map_err(|_| Error::Protocol("Connection control is unavailable".into()))?;
        if let Some(socket) = socket.as_ref() {
            match socket.shutdown(Shutdown::Both) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotConnected => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

pub struct Session {
    control: Arc<Control>,
    sender: SyncSender<WriterCommand>,
    pub counters: Arc<Counters>,
    audit: Scope,
    worker: Option<thread::JoinHandle<()>>,
    streaming: StreamSettings,
}

enum WriterCommand {
    Input(Command),
    ClipboardServer(crate::clipboard::Packet),
    ClipboardLocal(Zeroizing<String>),
    ClipboardEnabled(bool),
    NativeClipboard(crate::clipboard::apple::Command),
    Complete {
        size: crate::geometry::Extent,
        bytes: u64,
    },
}

impl Session {
    pub fn set_frame_notifier(&self, notifier: Arc<dyn Fn() + Send + Sync>) -> Result<()> {
        *self
            .control
            .frame_notifier
            .lock()
            .map_err(|_| Error::Display("Frame notification is unavailable".into()))? =
            Some(notifier);
        Ok(())
    }

    pub fn take_gpu_frame(&self) -> Result<Option<Arc<crate::render::gpu::nvdec::Frame>>> {
        Ok(self
            .control
            .gpu_frame
            .lock()
            .map_err(|_| Error::Display("GPU frame queue is unavailable".into()))?
            .take())
    }

    pub fn start(request: Request, events: Sender<(u64, Event)>, frames: Frames) -> Result<Self> {
        request
            .streaming
            .validate_authentication(request.profile.authentication)?;
        request.audio.validate()?;
        let streaming = request.streaming;
        let encoding = streaming.encoding;
        request.audit.critical("session.connection.requested", Level::Info, json!({
            "server.address": request.profile.host, "server.port": request.profile.port,
            "network.transport": "tcp", "network.protocol.name": "rfb",
            "rfb.authentication": request.profile.authentication,
            "credential.source": if matches!(request.credentials, CredentialSource::Saved) { "saved" } else { "manual" },
            "rfb.streaming": request.streaming,
        }))?;
        let audit = request.audit.clone();
        let control = Arc::new(Control::new(request.clipboard && !encoding.is_hevc()));
        let (sender, receiver) = mpsc::sync_channel(256);
        sender
            .try_send(WriterCommand::ClipboardEnabled(
                request.clipboard && !encoding.is_hevc(),
            ))
            .map_err(|_| Error::Protocol("Cannot initialize the clipboard writer".into()))?;
        let counters = Counters::for_session(encoding.is_hevc());
        let traffic = counters.clone();
        let worker_control = control.clone();
        let refresh = sender.clone();
        let worker_audit = audit.clone();
        let summary = counters.clone();
        let worker = thread::Builder::new()
            .name("maccanvas-rfb".into())
            .spawn(move || {
                let started = Instant::now();
                let generation = request.generation;
                let outcome = run(
                    request,
                    receiver,
                    refresh,
                    &events,
                    &frames,
                    &worker_control,
                    traffic,
                );
                let reconnect = reconnect_after_connection(
                    encoding.is_hevc(),
                    worker_control.cancelled.load(Ordering::Acquire),
                    &outcome,
                );
                if let Err(error) = &outcome
                    && events
                        .send((generation, Event::Error(error.to_string())))
                        .is_err()
                {
                    eprintln!("MacCanvas connection failed: {error}");
                }
                if let Err(error) = worker_audit.critical(
                    "session.connection.closed",
                    if outcome.is_ok() {
                        Level::Info
                    } else {
                        Level::Error
                    },
                    json!({
                        "event.outcome": if outcome.is_ok() { "closed" } else { "failure" },
                        "error.type": outcome.as_ref().err().map(audit::error_type),
                        "network.io.received": summary.received.load(Ordering::Relaxed),
                        "network.io.sent": summary.sent.load(Ordering::Relaxed),
                        "session.duration_ms": started.elapsed().as_millis() as u64,
                    }),
                ) {
                    eprintln!("MacCanvas connection summary could not be persisted: {error}");
                }
                if let Err(error) = worker_control.close() {
                    eprintln!("MacCanvas shutdown failed: {error}");
                }
                if events
                    .send((generation, Event::Disconnected { reconnect }))
                    .is_err()
                {
                    eprintln!("MacCanvas connection window has closed");
                }
            })?;
        Ok(Self {
            control,
            sender,
            counters,
            audit,
            worker: Some(worker),
            streaming,
        })
    }

    pub fn send(&self, command: Command) -> Result<()> {
        if matches!(command, Command::Audio(_)) && !self.uses_hevc() {
            return Err(Error::Audio {
                code: None,
                message: "Audio requires an HEVC session".into(),
            });
        }
        if let Command::Configure(settings) = command
            && self.requires_reconnect(settings)
        {
            return Err(Error::Configuration(
                "Changing transports, HEVC presets or the HEVC bitrate ceiling requires restarting the stream".into(),
            ));
        }
        self.post(WriterCommand::Input(command))
    }

    pub fn uses_hevc(&self) -> bool {
        self.streaming.encoding.is_hevc()
    }

    pub fn requires_reconnect(&self, settings: StreamSettings) -> bool {
        self.streaming.requires_reconnect(settings)
    }

    fn post(&self, command: WriterCommand) -> Result<()> {
        match self.sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.control.close()?;
                self.audit.critical(
                    "session.input_queue.exhausted",
                    Level::Error,
                    json!({"queue.capacity":256, "event.outcome":"disconnected"}),
                )?;
                Err(Error::Protocol(
                    "Input queue filled; the connection was closed to avoid stuck keys".into(),
                ))
            }
            Err(TrySendError::Disconnected(_)) => {
                Err(Error::Protocol("The connection has closed".into()))
            }
        }
    }

    pub fn close(&self) -> Result<()> {
        self.control.close()
    }

    pub fn is_closed(&self) -> bool {
        self.control.cancelled.load(Ordering::Acquire)
    }

    pub fn native_clipboard(&self, command: crate::clipboard::apple::Command) -> Result<()> {
        if !self.uses_hevc() {
            return Err(Error::Clipboard(
                "Native Apple clipboard requires the encrypted HEVC connection".into(),
            ));
        }
        match &command {
            crate::clipboard::apple::Command::Configure { epoch, enabled, .. } => {
                self.control.configure_native(*epoch, *enabled)?
            }
            crate::clipboard::apple::Command::Fetch { epoch, .. }
            | crate::clipboard::apple::Command::Send { epoch, .. }
                if !self.control.accepts_native(*epoch) =>
            {
                return Ok(());
            }
            _ => (),
        }
        let operation = match &command {
            crate::clipboard::apple::Command::Configure { enabled, .. } => {
                if *enabled {
                    "enable"
                } else {
                    "disable"
                }
            }
            crate::clipboard::apple::Command::Fetch { .. } => "fetch",
            crate::clipboard::apple::Command::Send { text, .. } => {
                crate::clipboard::validate(text, true)?;
                "send"
            }
        };
        self.audit.critical(
            "clipboard.apple.requested",
            Level::Info,
            json!({"clipboard.operation":operation,"network.encrypted":true}),
        )?;
        self.post(WriterCommand::NativeClipboard(command))
    }
    pub fn take_native_clipboard(&self) -> Result<Vec<crate::clipboard::apple::Event>> {
        Ok(self
            .control
            .native_clipboard
            .lock()
            .map_err(|_| Error::Clipboard("Native clipboard queue is unavailable".into()))?
            .drain(..)
            .collect())
    }

    pub fn share_clipboard(&self, enabled: bool) -> Result<()> {
        if enabled && self.uses_hevc() {
            return Err(Error::Clipboard(
                "Use the encrypted SSH controls in Clipboard while using HEVC".into(),
            ));
        }
        self.audit.critical(
            "clipboard.sharing.changed",
            Level::Info,
            json!({"clipboard.enabled":enabled}),
        )?;
        self.control
            .clipboard_enabled
            .store(enabled, Ordering::Release);
        if !enabled {
            self.control
                .clipboard_text
                .lock()
                .map_err(|_| Error::Clipboard("Clipboard queue is unavailable".into()))?
                .take();
        }
        self.post(WriterCommand::ClipboardEnabled(enabled))
    }

    pub fn clipboard_utf8(&self) -> bool {
        self.control.clipboard_utf8.load(Ordering::Acquire)
    }

    pub fn send_clipboard(&self, text: Zeroizing<String>) -> Result<()> {
        if !self.control.clipboard_enabled.load(Ordering::Acquire) {
            return Err(Error::Clipboard(
                "Enable plain-text clipboard sharing first".into(),
            ));
        }
        crate::clipboard::validate_vnc(&text, self.clipboard_utf8())?;
        self.post(WriterCommand::ClipboardLocal(text))
    }

    pub fn take_clipboard(&self) -> Result<Option<Zeroizing<String>>> {
        Ok(self
            .control
            .clipboard_text
            .lock()
            .map_err(|_| Error::Clipboard("Clipboard queue is unavailable".into()))?
            .take())
    }

    pub fn scope(&self) -> &Scope {
        &self.audit
    }

    pub fn is_finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }

    pub fn finish(mut self) -> Result<()> {
        self.control.close()?;
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| Error::Protocol("Connection worker stopped unexpectedly".into()))?;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = self.control.close() {
            eprintln!("MacCanvas connection cleanup failed: {error}");
        }
    }
}

fn run(
    request: Request,
    incoming: Receiver<WriterCommand>,
    refresh: SyncSender<WriterCommand>,
    events: &Sender<(u64, Event)>,
    frames: &Frames,
    control: &Arc<Control>,
    counters: Arc<Counters>,
) -> Result<()> {
    if request.streaming.encoding.is_hevc() {
        return hevc::run(request, incoming, events, frames, control, counters);
    }
    let generation = request.generation;
    let credentials = credentials::resolve(
        request.profile.id,
        request.credentials,
        &AuditedConsent {
            scope: &request.audit,
            window: request.window,
        },
        &AuditedVault {
            scope: &request.audit,
        },
    )?;
    if control.cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let stream = protocol::connect(&request.profile)?;
    request.audit.critical("network.connection.established", Level::Info, json!({
        "server.socket.address": stream.peer_addr()?.ip().to_string(), "server.socket.port": stream.peer_addr()?.port(),
        "client.socket.address": stream.local_addr()?.ip().to_string(), "client.socket.port": stream.local_addr()?.port(),
    }))?;
    *control
        .socket
        .lock()
        .map_err(|_| Error::Protocol("Connection control is unavailable".into()))? =
        Some(stream.try_clone()?);
    if control.cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut stream = Metered {
        inner: stream,
        counters: counters.clone(),
    };
    request.audit.critical(
        "security.authentication.started",
        Level::Info,
        json!({"network.protocol.name":"rfb"}),
    )?;
    let info = protocol::handshake_with_settings(
        &mut stream,
        request.profile.authentication,
        &credentials,
        request.streaming,
    )?;
    request.audit.critical("security.authentication.completed", Level::Info, json!({
        "event.outcome":"success", "rfb.security.type": if request.profile.authentication == crate::profiles::AuthMode::MacLogin { 30 } else { 2 },
        "rfb.framebuffer.width": info.size.width, "rfb.framebuffer.height": info.size.height,
        "rfb.desktop.name": info.name, "network.encrypted": false,
    }))?;
    stream.inner.set_read_timeout(None)?;
    let mut profile = request.profile;
    let mut credentials_saved = false;
    if request.remember {
        match (AuditedVault {
            scope: &request.audit,
        })
        .save(profile.id, &credentials)
        {
            Ok(()) => {
                profile.has_saved_credentials = true;
                credentials_saved = true;
            }
            Err(error) => {
                if events
                    .send((
                        generation,
                        Event::Warning(format!(
                            "Connected, but the password was not saved: {error}"
                        )),
                    ))
                    .is_err()
                {
                    return Err(Error::Protocol("Connection window closed".into()));
                }
            }
        }
    }
    drop(credentials);
    let writer = stream.inner.try_clone()?;
    writer.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut writer = Metered {
        inner: writer,
        counters: counters.clone(),
    };
    let writer_control = control.clone();
    let writer_events = events.clone();
    let writer_audit = request.audit.clone();
    let writer_thread = thread::spawn(move || {
        let mut noticed = |notice| -> Result<()> {
            use crate::clipboard::Notice;
            match notice {
                Notice::Text(text) => {
                    if writer_control.clipboard_enabled.load(Ordering::Acquire) {
                        *writer_control.clipboard_text.lock().map_err(|_| Error::Clipboard("Clipboard queue is unavailable".into()))? = Some(text);
                    }
                }
                Notice::Mode(utf8) => { writer_control.clipboard_utf8.store(utf8, Ordering::Release); }
                Notice::Received { bytes, encoding, accepted } => writer_audit.event("clipboard.received", Level::Info, json!({
                    "clipboard.direction":"mac_to_windows", "clipboard.encoding":encoding, "clipboard.wire_bytes":bytes, "clipboard.accepted":accepted,
                }))?,
                Notice::Sent { bytes, encoding, announced } => writer_audit.event(if announced { "clipboard.announced" } else { "clipboard.sent" }, Level::Info, json!({
                    "clipboard.direction":"windows_to_mac", "clipboard.encoding":encoding, "clipboard.text_bytes":bytes,
                }))?,
                Notice::Ignored { bytes, reason } => writer_audit.event("clipboard.ignored", Level::Info, json!({
                    "clipboard.wire_bytes":bytes, "clipboard.reason":reason,
                }))?,
            }
            Ok(())
        };
        if let Err(error) = write_loop(
            &mut writer,
            incoming,
            request.streaming,
            &writer_control,
            &mut noticed,
            Some(&writer_audit),
        ) {
            if let Err(audit_error) = writer_audit.critical(
                "session.writer.failed",
                Level::Error,
                json!({"error.type":audit::error_type(&error)}),
            ) {
                eprintln!("MacCanvas could not persist the writer failure: {audit_error}");
            }
            if !writer_control.cancelled.load(Ordering::Acquire)
                && writer_events
                    .send((generation, Event::Error(error.to_string())))
                    .is_err()
            {
                eprintln!("MacCanvas input failed: {error}");
            }
            if let Err(error) = writer_control.close() {
                eprintln!("MacCanvas input shutdown failed: {error}");
            }
        }
    });
    events
        .send((
            generation,
            Event::Connected {
                profile,
                server_name: info.name,
                credentials_saved,
            },
        ))
        .map_err(|_| Error::Protocol("Connection window closed".into()))?;
    let mut decoder = Decoder::new(info.size);
    let result = loop {
        if control.cancelled.load(Ordering::Acquire) {
            break Ok(());
        }
        let bytes_before = counters.received.load(Ordering::Relaxed);
        match decoder.read_message_with_clipboard(&mut stream, || {
            control.clipboard_enabled.load(Ordering::Acquire)
        }) {
            Ok(Message::Update { changed }) => {
                if let Some(encoding) = decoder.last_encoding {
                    counters.encoding.store(encoding, Ordering::Relaxed);
                }
                if changed {
                    let mut latest = match frames.lock() {
                        Ok(latest) => latest,
                        Err(_) => break Err(Error::Display("Frame queue is unavailable".into())),
                    };
                    *latest = Some(Arc::new(decoder.frame.clone()));
                }
                if refresh
                    .send(WriterCommand::Complete {
                        size: decoder.frame.size,
                        bytes: counters
                            .received
                            .load(Ordering::Relaxed)
                            .saturating_sub(bytes_before),
                    })
                    .is_err()
                {
                    break if control.cancelled.load(Ordering::Acquire) {
                        Ok(())
                    } else {
                        Err(Error::Protocol("Input worker closed".into()))
                    };
                }
            }
            Ok(Message::Clipboard(packet)) => {
                if control.clipboard_pending.fetch_add(1, Ordering::AcqRel) >= 4 {
                    control.clipboard_pending.fetch_sub(1, Ordering::AcqRel);
                    break Err(Error::Protocol(
                        "Too many pending clipboard messages".into(),
                    ));
                }
                if refresh
                    .send(WriterCommand::ClipboardServer(packet))
                    .is_err()
                {
                    break Err(Error::Protocol("Clipboard writer closed".into()));
                }
            }
            Ok(Message::Other) => (),
            Err(_) if control.cancelled.load(Ordering::Acquire) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    control.close()?;
    writer_thread
        .join()
        .map_err(|_| Error::Protocol("Input worker stopped unexpectedly".into()))?;
    result
}

fn write_loop(
    writer: &mut impl std::io::Write,
    incoming: Receiver<WriterCommand>,
    mut settings: StreamSettings,
    control: &Control,
    mut noticed: impl FnMut(crate::clipboard::Notice) -> Result<()>,
    keyboard_audit: Option<&Scope>,
) -> Result<()> {
    let mut keyboard_trace = crate::input::diagnostics::Trace::default();
    let mut clipboard = crate::clipboard::Transport::new(false);
    let mut last_request = Instant::now();
    let mut pending = None;
    let mut ignore_in_flight_budget = false;
    while !control.cancelled.load(Ordering::Acquire) {
        let delay = pending.map_or(Duration::from_millis(100), |(_, bytes)| {
            (last_request + settings.request_interval(bytes))
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100))
        });
        if let Some((size, bytes)) = pending
            && Instant::now() >= last_request + settings.request_interval(bytes)
        {
            Command::Refresh {
                size,
                incremental: true,
            }
            .write(writer)?;
            last_request = Instant::now();
            pending = None;
            continue;
        }
        match incoming.recv_timeout(delay) {
            Ok(WriterCommand::NativeClipboard(_)) => {
                return Err(Error::Clipboard(
                    "Native clipboard cannot use an unencrypted VNC writer".into(),
                ));
            }
            Ok(WriterCommand::ClipboardEnabled(enabled)) => clipboard.enable(writer, enabled)?,
            Ok(WriterCommand::ClipboardLocal(text)) => {
                for notice in clipboard.local(writer, text)? {
                    noticed(notice)?;
                }
            }
            Ok(WriterCommand::ClipboardServer(packet)) => {
                control.clipboard_pending.fetch_sub(1, Ordering::AcqRel);
                for notice in clipboard.server(writer, packet)? {
                    noticed(notice)?;
                }
            }
            Ok(WriterCommand::Input(Command::Configure(updated))) => {
                updated.validate()?;
                settings = updated;
                Command::Configure(updated).write(writer)?;
                // A new target must not charge a frame requested under the old target.
                if let Some((size, _)) = pending {
                    pending = Some((size, 0));
                    ignore_in_flight_budget = false;
                } else {
                    ignore_in_flight_budget = true;
                }
                last_request = Instant::now();
            }
            Ok(WriterCommand::Input(command)) => {
                command.write(writer)?;
                if let (Some(scope), Command::Key { keysym, down }) = (keyboard_audit, command)
                    && let Some(record) = keyboard_trace
                        .sent(crate::input::keyboard::Key { keysym, down }, "rfb_written")
                {
                    let _ = scope.event("input.keyboard.transport", Level::Info, record);
                }
                if matches!(command, Command::Refresh { .. }) {
                    last_request = Instant::now();
                    pending = None;
                }
            }
            Ok(WriterCommand::Complete { size, bytes }) => {
                let bytes = if std::mem::take(&mut ignore_in_flight_budget) {
                    0
                } else {
                    bytes
                };
                pending = Some((size, bytes));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabling_native_sharing_rejects_a_worker_result_already_in_flight() {
        use crate::clipboard::apple::Event as NativeEvent;
        let control = Arc::new(Control::new(false));
        control.configure_native(1, true).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker_control = control.clone();
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            let event = NativeEvent::Snapshot {
                epoch: 1,
                sequence: 9,
                text: Some(Zeroizing::new("fixture".into())),
            };
            worker_barrier.wait();
            worker_barrier.wait();
            worker_control.push_native(event).unwrap();
        });
        barrier.wait();
        control.configure_native(2, false).unwrap();
        barrier.wait();
        worker.join().unwrap();
        assert!(control.native_clipboard.lock().unwrap().is_empty());
        control.configure_native(3, true).unwrap();
        control
            .push_native(NativeEvent::Changed { epoch: 1 })
            .unwrap();
        assert!(control.native_clipboard.lock().unwrap().is_empty());
        control
            .push_native(NativeEvent::Changed { epoch: 3 })
            .unwrap();
        assert_eq!(control.native_clipboard.lock().unwrap().len(), 1);
    }

    #[test]
    fn transport_loss_retries_but_authentication_refusal_and_manual_disconnect_do_not() {
        let lost = Err(Error::ConnectionLost("fixture transport loss".into()));
        assert!(reconnect_after_connection(true, false, &lost));
        assert!(!reconnect_after_connection(true, true, &lost));
        assert!(!reconnect_after_connection(false, false, &lost));
        let io = Err(Error::Io(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )));
        assert!(reconnect_after_connection(true, false, &io));
        for outcome in [
            Ok(()),
            Err(Error::Authentication("fixture refusal".into())),
            Err(Error::Consent("fixture refusal".into())),
            Err(Error::Credential("fixture refusal".into())),
            Err(Error::Audit("fixture audit failure".into())),
            Err(Error::Cancelled),
        ] {
            assert!(!reconnect_after_connection(true, false, &outcome));
        }
    }
    struct Sink {
        sent: Sender<Vec<u8>>,
    }
    impl std::io::Write for Sink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.sent.send(data.to_vec()).unwrap();
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn bandwidth_pacing_never_blocks_pointer_or_key_release() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let control = Arc::new(Control::new(false));
        let (sent, received) = mpsc::channel();
        let worker_control = control.clone();
        let worker = thread::spawn(move || {
            write_loop(
                &mut Sink { sent },
                receiver,
                StreamSettings {
                    encoding: crate::streaming::Encoding::Auto,
                    target_kbps: 100,
                    ..Default::default()
                },
                &worker_control,
                |_| Ok(()),
                None,
            )
            .unwrap()
        });
        sender
            .send(WriterCommand::Complete {
                size: crate::geometry::Extent::new(4, 4).unwrap(),
                bytes: 1_000_000,
            })
            .unwrap();
        sender
            .send(WriterCommand::Input(Command::Key {
                keysym: 0xffe3,
                down: false,
            }))
            .unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_millis(250)).unwrap(),
            [4, 0, 0, 0]
        );
        control.cancelled.store(true, Ordering::Release);
        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn lowering_a_target_does_not_freeze_updates_for_the_previous_frame() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let control = Arc::new(Control::new(false));
        let (sent, received) = mpsc::channel();
        let worker_control = control.clone();
        let worker = thread::spawn(move || {
            write_loop(
                &mut Sink { sent },
                receiver,
                StreamSettings {
                    encoding: crate::streaming::Encoding::Auto,
                    ..Default::default()
                },
                &worker_control,
                |_| Ok(()),
                None,
            )
            .unwrap()
        });
        sender
            .send(WriterCommand::Complete {
                size: crate::geometry::Extent::new(4, 4).unwrap(),
                bytes: 33_000_000,
            })
            .unwrap();
        sender
            .send(WriterCommand::Input(Command::Configure(StreamSettings {
                encoding: crate::streaming::Encoding::Auto,
                target_kbps: 100,
                ..Default::default()
            })))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let fragment = received
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if fragment.first() == Some(&3) {
                break;
            }
        }
        control.cancelled.store(true, Ordering::Release);
        drop(sender);
        worker.join().unwrap();
    }
}
