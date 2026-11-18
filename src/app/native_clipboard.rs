//! Explicitly enabled Apple HPS experiment. SSH is never started by this bridge.
use super::*;
use maccanvas::clipboard::{
    apple::{Command as NativeCommand, Event as NativeEvent},
    native,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[path = "native_clipboard/files.rs"]
mod files;
enum Pending {
    Files(Vec<std::path::PathBuf>),
    Rich {
        rich: maccanvas::clipboard::rich::RichText,
        message: maccanvas::clipboard::apple::RichMessage,
    },
    Text(Zeroizing<String>),
    Image {
        png: Zeroizing<Vec<u8>>,
        message: maccanvas::clipboard::apple::ImageMessage,
    },
}
impl Pending {
    fn hash(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        match self {
            Self::Files(paths) => {
                return maccanvas::clipboard::filecopy::urls::digest(
                    &paths
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect::<Vec<_>>(),
                );
            }
            Self::Rich { rich, .. } => return rich.digest(),
            Self::Text(text) => {
                hash.update(b"text\0");
                hash.update(text.as_bytes());
            }
            Self::Image { png, .. } => {
                hash.update(b"png\0");
                hash.update(&**png);
            }
        }
        hash.finalize().into()
    }
}
struct Captured {
    sequence: u32,
    content: Pending,
    hash: [u8; 32],
}
impl Captured {
    fn new(sequence: u32, content: Pending) -> Self {
        let hash = content.hash();
        Self {
            sequence,
            content,
            hash,
        }
    }
}
enum ImageResult {
    ApplyRich {
        rich: maccanvas::clipboard::rich::RichText,
        hash: [u8; 32],
    },
    Capture(Option<Captured>),
    Apply {
        png: Zeroizing<Vec<u8>>,
        dib: Zeroizing<Vec<u8>>,
        hash: [u8; 32],
    },
}
struct ImageTask {
    epoch: u64,
    sequence: u32,
    handle: std::thread::JoinHandle<Result<ImageResult>>,
}
// A reset can orphan a running conversion. Keep its global slot until the thread exits.
static IMAGE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct ImageSlot;
impl Drop for ImageSlot {
    fn drop(&mut self) {
        IMAGE_BUSY.store(false, std::sync::atomic::Ordering::Release);
    }
}
fn image_task(
    epoch: u64,
    sequence: u32,
    work: impl FnOnce() -> Result<ImageResult> + Send + 'static,
) -> Result<Option<ImageTask>> {
    if IMAGE_BUSY
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return Ok(None);
    }
    let handle = std::thread::Builder::new()
        .name("native-clipboard-image".into())
        .spawn(move || {
            let _slot = ImageSlot;
            work()
        });
    match handle {
        Ok(handle) => Ok(Some(ImageTask {
            epoch,
            sequence,
            handle,
        })),
        Err(error) => {
            IMAGE_BUSY.store(false, std::sync::atomic::Ordering::Release);
            Err(Error::Clipboard(format!(
                "Could not start clipboard worker: {error}"
            )))
        }
    }
}
fn capture_native(owner: isize, files: bool) -> Result<ImageResult> {
    if let Some((sequence, paths)) = native::read_files_snapshot(owner)? {
        return Ok(ImageResult::Capture(if files {
            Some(Captured::new(sequence, Pending::Files(paths)))
        } else {
            None
        }));
    }
    let mut rich = native::read_rich_snapshot(owner)?;
    if rich
        .as_ref()
        .is_some_and(|(_, rich)| rich.plain.as_ref().is_some_and(|plain| !plain.is_empty()))
    {
        let (seq, rich) = rich.take().unwrap();
        let message = maccanvas::clipboard::apple::RichMessage::new(&rich)?;
        return Ok(ImageResult::Capture(Some(Captured::new(
            seq,
            Pending::Rich { rich, message },
        ))));
    }
    if let Some((seq, image)) = native::read_image_snapshot(owner)? {
        let png = match image {
            native::ImageSource::Png(mut png) => {
                let length = maccanvas::clipboard::image::png_length(&png)?;
                png.truncate(length);
                // Full CRC/pixel validation, including limits, off the UI/video threads.
                maccanvas::clipboard::image::png_to_dib(&png)?;
                png
            }
            native::ImageSource::Tiff(tiff) => maccanvas::clipboard::image::tiff_to_png(&tiff)?,
            native::ImageSource::Dib(dib) => maccanvas::clipboard::image::dib_to_png(&dib)?,
        };
        Ok(ImageResult::Capture(Some(Captured::new(
            seq,
            Pending::Image {
                message: maccanvas::clipboard::apple::ImageMessage::new(&png)?,
                png,
            },
        ))))
    } else if let Some((seq, rich)) = rich {
        let message = maccanvas::clipboard::apple::RichMessage::new(&rich)?;
        Ok(ImageResult::Capture(Some(Captured::new(
            seq,
            Pending::Rich { rich, message },
        ))))
    } else {
        Ok(ImageResult::Capture(
            native::read_snapshot(owner)?
                .filter(|(_, text)| !text.is_empty())
                .map(|(seq, text)| Captured::new(seq, Pending::Text(text))),
        ))
    }
}
fn capture_native_with_retry(owner: isize, files: bool) -> Result<ImageResult> {
    for attempt in 0..3 {
        match capture_native(owner, files) {
            Err(Error::Clipboard(message))
                if attempt < 2
                    && message.starts_with("Windows clipboard is busy or unavailable") =>
            {
                std::thread::sleep(Duration::from_millis(20))
            }
            result => return result,
        }
    }
    unreachable!("the final capture attempt always returns")
}
pub(super) struct Bridge {
    profile: Option<Uuid>,
    configured: Option<(u64, bool, bool, bool, bool)>,
    epoch: u64,
    remote_dirty: bool,
    active: bool,
    baseline: bool,
    paused: bool,
    checked: Instant,
    interactive: bool,
    observed: Instant,
    sequence: u32,
    hash: Option<[u8; 32]>,
    pending: Option<Pending>,
    image_task: Option<ImageTask>,
    file_task: Option<files::Job>,
    staged: Vec<maccanvas::clipboard::filecopy::StagedFiles>,
    file_echo: Option<[u8; 32]>,
    changed: Instant,
}
impl Default for Bridge {
    fn default() -> Self {
        Self {
            profile: None,
            configured: None,
            epoch: 0,
            remote_dirty: false,
            active: false,
            baseline: true,
            paused: false,
            checked: Instant::now() - Duration::from_secs(1),
            interactive: false,
            observed: Instant::now(),
            sequence: 0,
            hash: None,
            pending: None,
            image_task: None,
            file_task: None,
            staged: Vec::new(),
            file_echo: None,
            changed: Instant::now(),
        }
    }
}
fn content_digest(kind: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(kind);
    hash.update(bytes);
    hash.finalize().into()
}
fn digest(text: &str) -> [u8; 32] {
    content_digest(b"text\0", text.as_bytes())
}
fn image_digest(png: &[u8]) -> [u8; 32] {
    content_digest(b"png\0", png)
}
fn image_result_current(
    epoch: u64,
    sequence: u32,
    current_epoch: u64,
    current_sequence: u32,
    active: bool,
) -> bool {
    active && epoch == current_epoch && sequence == current_sequence
}
fn post(state: &State, command: NativeCommand) -> Result<()> {
    state
        .session
        .as_ref()
        .ok_or_else(|| Error::Clipboard("No active Apple session".into()))?
        .native_clipboard(command)
}
pub(super) fn wire(app: &App, state: Rc<RefCell<State>>) {
    let weak = app.as_weak();
    app.on_native_clipboard_changed(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let mut state = state.borrow_mut();
        state.native_clipboard.configured = None;
        state.native_clipboard.pending = None;
        state.native_clipboard.paused = false;
        if app.get_native_clipboard_enabled() {
            state.files.pause_auto_clipboard();
            if app.get_clipboard_shared() {
                app.set_clipboard_shared(false);
                state.settings.clipboard_enabled = false;
                state.pending_clipboard = None;
            }
        }
        if let Err(error) = tick(&app, &mut state) {
            app.set_native_clipboard_status(format!("Native error: {error}").into());
        }
    });
}
pub(super) fn tick(app: &App, state: &mut State) -> Result<()> {
    let id = state.connection_profile.as_ref().map(|p| p.id);
    if let Some(id) = id
        && state.native_clipboard.profile != Some(id)
    {
        if let Some(job) = &state.native_clipboard.file_task {
            job.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        let staged = std::mem::take(&mut state.native_clipboard.staged);
        state.native_clipboard = Bridge {
            profile: Some(id),
            staged,
            ..Default::default()
        };
        app.set_native_clipboard_enabled(false);
        app.set_native_clipboard_files(false);
        app.set_native_clipboard_send(true);
        app.set_native_clipboard_receive(true);
    }
    if app.get_connected()
        && state
            .session
            .as_ref()
            .is_some_and(|session| !session.uses_hevc())
    {
        // A protocol switch must not leave the hidden native option blocking the existing SSH UI.
        app.set_native_clipboard_enabled(false);
    }
    let enabled = app.get_native_clipboard_enabled();
    let connected = app.get_connected() && state.session.as_ref().is_some_and(Session::uses_hevc);
    if !enabled
        && !state.native_clipboard.active
        && state.native_clipboard.file_task.is_none()
        && state.native_clipboard.image_task.is_none()
    {
        return Ok(());
    }
    if state.native_clipboard.checked.elapsed() >= Duration::from_millis(250) {
        state.native_clipboard.checked = Instant::now();
        state.native_clipboard.interactive = native::interactive_session();
    }
    let active = enabled && connected && state.native_clipboard.interactive;
    let receive = app.get_native_clipboard_receive();
    let send = app.get_native_clipboard_send();
    let file_enabled = app.get_native_clipboard_files();
    let config = (state.generation, active, receive, send, file_enabled);
    if state.native_clipboard.configured != Some(config) {
        if state
            .native_clipboard
            .configured
            .is_some_and(|previous| previous.0 != state.generation)
        {
            state.native_clipboard.paused = false;
        }
        state.native_clipboard.epoch = state.native_clipboard.epoch.wrapping_add(1);
        state.native_clipboard.active = active;
        state.native_clipboard.baseline = true;
        state.native_clipboard.sequence = native::sequence();
        state.native_clipboard.hash = None;
        state.native_clipboard.pending = None;
        state.native_clipboard.remote_dirty = false;
        state.native_clipboard.file_echo = None;
        state.native_clipboard.configured = Some(config);
        if connected {
            post(
                state,
                NativeCommand::Configure {
                    epoch: state.native_clipboard.epoch,
                    enabled: active,
                    receive,
                    send,
                },
            )?;
        }
        if let Some(session) = &state.session {
            session.authorize_native_files(state.native_clipboard.epoch, active && file_enabled)?;
        }
        if !state.native_clipboard.paused {
            app.set_native_clipboard_status(
                if active {
                    "Encrypted Apple clipboard: establishing baseline"
                } else {
                    "Paused — disabled, disconnected or Windows session locked"
                }
                .into(),
            );
        }
    }
    let events = state
        .session
        .as_ref()
        .map(Session::take_native_clipboard)
        .transpose()?
        .unwrap_or_default();
    if events.iter().any(|event| matches!(event,NativeEvent::SessionChanged{epoch} if *epoch==state.native_clipboard.epoch)) {
        if let Some(job)=&state.native_clipboard.file_task{job.cancelled.store(true,std::sync::atomic::Ordering::Release);}
        if let Some(session)=&state.session{session.authorize_native_files(state.native_clipboard.epoch,false)?;}
        state.native_clipboard.configured=None;
        state.native_clipboard.epoch=state.native_clipboard.epoch.wrapping_add(1);
        state.native_clipboard.pending=None;
        state.native_clipboard.baseline=true;
        state.native_clipboard.remote_dirty=false;
        app.set_native_clipboard_status("Mac session changed — rearming native clipboard".into());
        return Ok(());
    }
    if events.iter().any(|event| matches!(event, NativeEvent::Changed {epoch} if *epoch == state.native_clipboard.epoch)) {
        state.native_clipboard.remote_dirty = true;
    }
    files::poll(
        app,
        state,
        active,
        send,
        receive,
        state.native_clipboard.remote_dirty,
    )?;
    if state
        .native_clipboard
        .image_task
        .as_ref()
        .is_some_and(|task| task.handle.is_finished())
    {
        let task = state.native_clipboard.image_task.take().unwrap();
        let result = task
            .handle
            .join()
            .unwrap_or_else(|_| Err(Error::Clipboard("Clipboard image worker failed".into())));
        if image_result_current(
            task.epoch,
            task.sequence,
            state.native_clipboard.epoch,
            native::sequence(),
            active && !state.native_clipboard.paused && !events.iter().any(|event| matches!(event, NativeEvent::Problem {epoch, reason: "request_rejected" | "fetch_timeout" | "echo_mismatch"} if *epoch==state.native_clipboard.epoch)),
        ) && native::interactive_session()
        {
            match result {
                Ok(ImageResult::Capture(Some(capture)))
                    if send && capture.sequence == task.sequence =>
                {
                    state.native_clipboard.sequence = capture.sequence;
                    if (state.native_clipboard.hash==Some(capture.hash)||matches!(&capture.content,Pending::Files(_)))&&native::owns_clipboard(window_handle(app)?) {state.native_clipboard.pending=None;}
                    else {state.native_clipboard.hash = Some(capture.hash);state.native_clipboard.pending = Some(capture.content);}
                    state.native_clipboard.changed = Instant::now();
                }
                Ok(ImageResult::Capture(None)) if send => {
                    state.native_clipboard.sequence = task.sequence;
                    state.native_clipboard.hash = None;
                    state.native_clipboard.pending = None;
                }
                Ok(ImageResult::ApplyRich {rich,hash}) if receive && state.native_clipboard.pending.is_none() && state.native_clipboard.file_task.is_none() && !state.native_clipboard.remote_dirty && (state.native_clipboard.sequence!=task.sequence||state.native_clipboard.hash!=Some(hash)) => {
                    state.audit.event("clipboard.apple.apply_requested",Level::Info,json!({"clipboard.format":"rich_text","network.encrypted":true}))?;
                    if let Some(written)=native::write_rich_if_sequence(window_handle(app)?,&rich,task.sequence)? {
                        state.native_clipboard.sequence=written;state.native_clipboard.hash=Some(hash);state.clipboard.sent(written);
                        app.set_native_clipboard_status("Mac → Windows rich text received over encrypted Apple control".into());
                    }
                }
                Ok(ImageResult::Apply { png, dib, hash })
                    if receive
                        && state.native_clipboard.pending.is_none()
                        && state.native_clipboard.file_task.is_none()
                        && !state.native_clipboard.remote_dirty
                        && (state.native_clipboard.sequence != task.sequence || state.native_clipboard.hash != Some(hash)) =>
                {
                    state.audit.event("clipboard.apple.apply_requested", Level::Info, json!({"clipboard.format":"png","clipboard.image_bytes":png.len(),"network.encrypted":true}))?;
                    if let Some(written) = native::write_image_if_sequence(
                        window_handle(app)?,
                        &png,
                        &dib,
                        task.sequence,
                    )? {
                        state.native_clipboard.sequence = written;
                        state.native_clipboard.hash = Some(hash);
                        state.clipboard.sent(written);
                        app.set_native_clipboard_status(
                            "Mac → Windows image received over encrypted Apple control".into(),
                        );
                    }
                }
                Err(_) => {
                    // Suppress repeated work on the same unsupported/malformed copy.
                    state.native_clipboard.sequence = task.sequence;
                    state.native_clipboard.pending = None;
                    state.native_clipboard.hash = None;
                    app.set_native_clipboard_status("Native clipboard: unsupported, oversized or invalid image/text; contents unchanged".into());
                }
                _ => (),
            }
        }
    }
    for event in events {
        match event {
            NativeEvent::Changed { epoch }
                if active
                    && epoch == state.native_clipboard.epoch
                    && receive
                    && !state.native_clipboard.paused
                    && native::interactive_session() =>
            {
                state.native_clipboard.remote_dirty = true;
                if state.native_clipboard.image_task.is_none()
                    && state.native_clipboard.file_task.is_none()
                    && (!send
                        || (state.native_clipboard.pending.is_none()
                            && native::sequence() == state.native_clipboard.sequence))
                {
                    post(
                        state,
                        NativeCommand::Fetch {
                            epoch,
                            sequence: native::sequence(),
                        },
                    )?;
                    state.native_clipboard.remote_dirty = false;
                }
            }
            NativeEvent::ImageSnapshot {
                epoch,
                sequence,
                png: bytes,
            }
            | NativeEvent::TiffSnapshot {
                epoch,
                sequence,
                tiff: bytes,
            } if active && epoch == state.native_clipboard.epoch && receive => {
                state.native_clipboard.paused = false;
                if state.native_clipboard.baseline {
                    state.native_clipboard.baseline = false;
                    app.set_native_clipboard_status(
                        "Listening on encrypted Apple control — copy new text or a image".into(),
                    );
                    continue;
                }
                if sequence != native::sequence()
                    || state.native_clipboard.pending.is_some()
                    || state.native_clipboard.file_task.is_some()
                    || state.native_clipboard.image_task.is_some()
                    || !native::interactive_session()
                {
                    continue;
                }
                state.native_clipboard.image_task = image_task(epoch, sequence, move || {
                    let png = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                        bytes
                    } else {
                        maccanvas::clipboard::image::tiff_to_png(&bytes)?
                    };
                    let dib = maccanvas::clipboard::image::png_to_dib(&png)?;
                    let hash = image_digest(&png);
                    Ok(ImageResult::Apply { png, dib, hash })
                })?;
            }
            NativeEvent::FilesSnapshot {
                epoch,
                sequence,
                paths,
            } if active && epoch == state.native_clipboard.epoch && receive => {
                if state.native_clipboard.baseline {
                    state.native_clipboard.baseline = false;
                    continue;
                }
                if !file_enabled || paths.is_empty() {
                    continue;
                }
                let hash = maccanvas::clipboard::filecopy::urls::digest(&paths);
                if state.native_clipboard.file_echo == Some(hash) {
                    state.native_clipboard.file_echo = None;
                    continue;
                }
                if sequence != native::sequence()
                    || state.native_clipboard.file_task.is_some()
                    || state.native_clipboard.image_task.is_some()
                    || state.native_clipboard.pending.is_some()
                    || !native::interactive_session()
                {
                    continue;
                }
                files::start_download(state, paths, sequence)?;
            }
            NativeEvent::RichSnapshot {
                epoch,
                sequence,
                rich,
            } if active && epoch == state.native_clipboard.epoch && receive => {
                state.native_clipboard.paused = false;
                if state.native_clipboard.baseline {
                    state.native_clipboard.baseline = false;
                    app.set_native_clipboard_status(
                        "Listening on encrypted Apple control — copy new text or an image".into(),
                    );
                    continue;
                }
                if sequence != native::sequence()
                    || state.native_clipboard.pending.is_some()
                    || state.native_clipboard.file_task.is_some()
                    || state.native_clipboard.image_task.is_some()
                    || !native::interactive_session()
                {
                    continue;
                }
                state.native_clipboard.image_task = image_task(epoch, sequence, move || {
                    rich.validate()?;
                    let hash = rich.digest();
                    Ok(ImageResult::ApplyRich { rich, hash })
                })?;
            }
            NativeEvent::Snapshot {
                epoch,
                sequence,
                text,
            } if active && epoch == state.native_clipboard.epoch && receive => {
                // A current, validated reply resolves earlier transport ambiguity.
                state.native_clipboard.paused = false;
                if state.native_clipboard.baseline {
                    state.native_clipboard.baseline = false;
                    app.set_native_clipboard_status(
                        "Listening on encrypted Apple control — copy new text or a image".into(),
                    );
                    continue;
                }
                let Some(text) = text else {
                    continue;
                };
                if text.is_empty()
                    || sequence != native::sequence()
                    || state.native_clipboard.pending.is_some()
                    || state.native_clipboard.file_task.is_some()
                    || state.native_clipboard.image_task.is_some()
                    || !native::interactive_session()
                {
                    continue;
                }
                let hash = digest(&text);
                if state.native_clipboard.sequence == sequence
                    && state.native_clipboard.hash == Some(hash)
                {
                    continue;
                }
                state.audit.event(
                    "clipboard.apple.apply_requested",
                    Level::Info,
                    json!({"clipboard.text_bytes":text.len(),"network.encrypted":true}),
                )?;
                if let Some(written) =
                    native::write_if_sequence(window_handle(app)?, &text, sequence)?
                {
                    state.native_clipboard.sequence = written;
                    state.native_clipboard.hash = Some(hash);
                    state.clipboard.sent(written);
                    app.set_native_clipboard_status(
                        "Mac → Windows received over encrypted Apple control".into(),
                    );
                }
            }
            NativeEvent::SessionChanged { epoch } if epoch == state.native_clipboard.epoch => {
                state.native_clipboard.configured = None;
                state.native_clipboard.baseline = true;
                state.native_clipboard.pending = None;
                app.set_native_clipboard_status(
                    "Mac session changed — rearming native clipboard".into(),
                );
            }
            NativeEvent::Problem { epoch, reason } if epoch == state.native_clipboard.epoch => {
                if matches!(
                    reason,
                    "fetch_timeout" | "echo_mismatch" | "request_rejected"
                ) {
                    state.native_clipboard.paused = true;
                }
                if reason == "request_rejected" {
                    // No sender acceptance: invalidate older reads instead of letting an
                    // old Mac snapshot replace the Windows copy that could not be sent.
                    state.native_clipboard.epoch = state.native_clipboard.epoch.wrapping_add(1);
                    state.native_clipboard.configured = None;
                    state.native_clipboard.baseline = true;
                }
                app.set_native_clipboard_status(format!("Native experiment: {reason}; inspect clipboard.apple logs (no SSH fallback)").into());
            }
            NativeEvent::Sent { epoch, written } if epoch == state.native_clipboard.epoch => {
                app.set_native_clipboard_status(if written {
                    "Windows content written to the encrypted Mac connection. Paste on the Mac to verify acceptance."
                } else {
                    "Windows content queued; waiting for the encrypted connection to finish writing."
                }.into());
            }
            _ => (),
        }
    }
    if !active
        || state.native_clipboard.paused
        || state.native_clipboard.observed.elapsed() < Duration::from_millis(100)
    {
        return Ok(());
    }
    state.native_clipboard.observed = Instant::now();
    let sequence = native::sequence();
    if send
        && sequence != state.native_clipboard.sequence
        && state.native_clipboard.image_task.is_none()
        && state.native_clipboard.file_task.is_none()
    {
        state.native_clipboard.pending = None;
        let owner = window_handle(app)?;
        state.native_clipboard.image_task =
            image_task(state.native_clipboard.epoch, sequence, move || {
                capture_native_with_retry(owner, file_enabled)
            })?;
    }

    if send
        && state.native_clipboard.changed.elapsed() >= Duration::from_millis(150)
        && native::interactive_session()
        && state.native_clipboard.image_task.is_none()
        && state.native_clipboard.file_task.is_none()
        && native::sequence() == state.native_clipboard.sequence
        && let Some(content) = state.native_clipboard.pending.take()
    {
        if let Pending::Files(paths) = content {
            files::start_upload(state, paths)?;
        } else {
            post(
                state,
                match content {
                    Pending::Files(_) => unreachable!(),
                    Pending::Rich { message, .. } => NativeCommand::SendRich {
                        epoch: state.native_clipboard.epoch,
                        message,
                    },
                    Pending::Text(text) => NativeCommand::Send {
                        epoch: state.native_clipboard.epoch,
                        text,
                    },
                    Pending::Image { message, .. } => NativeCommand::SendImage {
                        epoch: state.native_clipboard.epoch,
                        message,
                    },
                },
            )?;
        }
    }
    if state.native_clipboard.remote_dirty
        && state.native_clipboard.image_task.is_none()
        && state.native_clipboard.file_task.is_none()
        && state.native_clipboard.pending.is_none()
        && (!send || native::sequence() == state.native_clipboard.sequence)
        && native::interactive_session()
    {
        post(
            state,
            NativeCommand::Fetch {
                epoch: state.native_clipboard.epoch,
                sequence: native::sequence(),
            },
        )?;
        state.native_clipboard.remote_dirty = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_results_require_current_epoch_sequence_and_authorization() {
        assert!(image_result_current(3, 8, 3, 8, true));
        assert!(!image_result_current(3, 8, 4, 8, true)); // reconnect/direction change
        assert!(!image_result_current(3, 8, 3, 9, true)); // another application's new copy
        assert!(!image_result_current(3, 8, 3, 8, false)); // disable/disconnect/lock
        assert_ne!(digest("same"), image_digest(b"same"));
    }
    #[test]
    fn image_worker_is_single_flight_and_releases_slot_on_completion_and_panic() {
        use std::sync::{Arc, Barrier};
        let start = Arc::new(Barrier::new(2));
        let end = Arc::new(Barrier::new(2));
        let (s, e) = (start.clone(), end.clone());
        let task = image_task(1, 2, move || {
            s.wait();
            e.wait();
            Ok(ImageResult::Capture(None))
        })
        .unwrap()
        .unwrap();
        start.wait();
        assert!(
            image_task(2, 3, || Ok(ImageResult::Capture(None)))
                .unwrap()
                .is_none()
        );
        end.wait();
        task.handle.join().unwrap().unwrap();
        let task = image_task(2, 3, || -> Result<ImageResult> {
            panic!("fixture conversion failure")
        })
        .unwrap()
        .unwrap();
        assert!(task.handle.join().is_err());
        let task = image_task(3, 4, || Ok(ImageResult::Capture(None)))
            .unwrap()
            .unwrap();
        task.handle.join().unwrap().unwrap();
    }
}
