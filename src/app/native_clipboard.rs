//! Explicitly enabled Apple HPS experiment. SSH is never started by this bridge.
use super::*;
use maccanvas::clipboard::{
    apple::{Command as NativeCommand, Event as NativeEvent},
    native,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub(super) struct Bridge {
    profile: Option<Uuid>,
    configured: Option<(u64, bool, bool, bool)>,
    epoch: u64,
    remote_dirty: bool,
    active: bool,
    verified: bool,
    baseline: bool,
    paused: bool,
    checked: Instant,
    interactive: bool,
    observed: Instant,
    sequence: u32,
    hash: Option<[u8; 32]>,
    pending: Option<Zeroizing<String>>,
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
            verified: false,
            baseline: true,
            paused: false,
            checked: Instant::now() - Duration::from_secs(1),
            interactive: false,
            observed: Instant::now(),
            sequence: 0,
            hash: None,
            pending: None,
            changed: Instant::now(),
        }
    }
}
fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
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
        state.native_clipboard = Bridge {
            profile: Some(id),
            ..Default::default()
        };
        app.set_native_clipboard_enabled(false);
        app.set_native_clipboard_send(false);
        app.set_native_clipboard_receive(true);
        app.set_native_clipboard_verified(false);
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
    if !enabled && !state.native_clipboard.active {
        return Ok(());
    }
    if state.native_clipboard.checked.elapsed() >= Duration::from_millis(250) {
        state.native_clipboard.checked = Instant::now();
        state.native_clipboard.interactive = native::interactive_session();
    }
    let active = enabled && connected && state.native_clipboard.interactive;
    let receive = app.get_native_clipboard_receive();
    let send = app.get_native_clipboard_send() && state.native_clipboard.verified;
    let config = (state.generation, active, receive, send);
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
        app.set_native_clipboard_status(
            if active {
                "Encrypted Apple clipboard: establishing baseline"
            } else {
                "Paused — disabled, disconnected or Windows session locked"
            }
            .into(),
        );
    }
    let events = state
        .session
        .as_ref()
        .map(Session::take_native_clipboard)
        .transpose()?
        .unwrap_or_default();
    if events.iter().any(|event| matches!(event,NativeEvent::SessionChanged{epoch} if *epoch==state.native_clipboard.epoch)) {
        state.native_clipboard.configured=None;
        state.native_clipboard.epoch=state.native_clipboard.epoch.wrapping_add(1);
        state.native_clipboard.pending=None;
        state.native_clipboard.baseline=true;
        state.native_clipboard.remote_dirty=false;
        app.set_native_clipboard_status("Mac session changed — rearming native clipboard".into());
        return Ok(());
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
                if !send
                    || (state.native_clipboard.pending.is_none()
                        && native::sequence() == state.native_clipboard.sequence)
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
                        "Listening on encrypted Apple control — copy new text on the Mac".into(),
                    );
                    continue;
                }
                let Some(text) = text else {
                    continue;
                };
                if text.is_empty()
                    || sequence != native::sequence()
                    || state.native_clipboard.pending.is_some()
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
                    state.native_clipboard.verified = true;
                    state.clipboard.sent(written);
                    app.set_native_clipboard_verified(true);
                    app.set_native_clipboard_status("Mac → Windows received over encrypted Apple control; sending can now be enabled".into());
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
                if reason == "fetch_timeout" || reason == "echo_mismatch" {
                    state.native_clipboard.paused = true;
                }
                app.set_native_clipboard_status(format!("Native experiment: {reason}; inspect clipboard.apple logs (no SSH fallback)").into());
            }
            NativeEvent::Sent { epoch } if epoch == state.native_clipboard.epoch => {
                app.set_native_clipboard_status(
                    "Windows text queued on encrypted Apple control — verify by pasting on the Mac"
                        .into(),
                );
            }
            _ => (),
        }
    }
    if !active
        || state.native_clipboard.paused
        || !send
        || state.native_clipboard.observed.elapsed() < Duration::from_millis(100)
    {
        return Ok(());
    }
    state.native_clipboard.observed = Instant::now();
    let sequence = native::sequence();
    if sequence != state.native_clipboard.sequence {
        let snapshot = native::read_snapshot(window_handle(app)?);
        match snapshot {
            Ok(Some((seq, text))) => {
                state.native_clipboard.sequence = seq;
                state.native_clipboard.hash = Some(digest(&text));
                state.native_clipboard.pending = (!text.is_empty()).then_some(text);
                state.native_clipboard.changed = Instant::now();
            }
            Ok(None) => {
                state.native_clipboard.sequence = sequence;
                state.native_clipboard.hash = None;
                state.native_clipboard.pending = None;
            }
            Err(_) => {
                app.set_native_clipboard_status(
                    "Native clipboard: Windows text busy or unsupported".into(),
                );
                return Ok(());
            }
        }
    }
    if state.native_clipboard.changed.elapsed() >= Duration::from_millis(150)
        && native::interactive_session()
        && let Some(text) = state.native_clipboard.pending.take()
    {
        post(
            state,
            NativeCommand::Send {
                epoch: state.native_clipboard.epoch,
                text,
            },
        )?;
    }
    if state.native_clipboard.remote_dirty
        && state.native_clipboard.pending.is_none()
        && native::sequence() == state.native_clipboard.sequence
    {
        post(
            state,
            NativeCommand::Fetch {
                epoch: state.native_clipboard.epoch,
                sequence: state.native_clipboard.sequence,
            },
        )?;
        state.native_clipboard.remote_dirty = false;
    }
    Ok(())
}
