use super::*;
use maccanvas::clipboard::sync::{Preferences, Reply, Sync};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(super) struct Auto {
    profile: Option<Uuid>,
    generation: u64,
    prefs: Preferences,
    sync: Sync,
    cancel: CancellationToken,
    clock: Instant,
    checked: Instant,
    interactive: bool,
    running: bool,
    sequence: u32,
    observed: Instant,
}
impl Default for Auto {
    fn default() -> Self {
        Self {
            profile: None,
            generation: 0,
            prefs: Preferences::default(),
            sync: Sync::default(),
            cancel: CancellationToken::new(),
            clock: Instant::now(),
            checked: Instant::now() - Duration::from_secs(1),
            interactive: false,
            running: false,
            sequence: 0,
            observed: Instant::now() - Duration::from_secs(1),
        }
    }
}
impl Auto {
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
    pub(super) fn clear(&mut self) {
        self.cancel.cancel();
        self.cancel = CancellationToken::new();
        self.sequence = native::sequence();
        self.sync
            .reset(Preferences::default(), self.sequence, self.now());
        self.running = false;
    }
}
fn load(app: &App, state: &mut State) {
    let profile = state.connection_profile.as_ref().map(|p| p.id);
    let prefs = state
        .settings
        .clipboard_auto
        .iter()
        .find(|p| Some(p.profile_id) == profile)
        .map(|p| p.preferences.clone())
        .unwrap_or_default();
    if profile != state.files.auto.profile || prefs != state.files.auto.prefs {
        state.files.auto.clear();
        state.files.auto.profile = profile;
        state.files.auto.prefs = prefs.clone();
        app.set_auto_clipboard_enabled(prefs.enabled);
        app.set_auto_clipboard_to_mac(prefs.to_mac);
        app.set_auto_clipboard_to_windows(prefs.to_windows);
        app.set_auto_clipboard_limit(prefs.max_bytes.to_string().into());
    }
}
pub(super) fn guard(app: &App, state: &mut State) {
    load(app, state);
    if state.files.auto.prefs.enabled && state.settings.clipboard_enabled {
        state.settings.clipboard_enabled = false;
        state.pending_clipboard = None;
        app.set_clipboard_shared(false);
        if let Some(session) = &state.session
            && !session.uses_hevc()
            && session.share_clipboard(false).is_err()
        {
            state.files.auto.clear();
            app.set_auto_clipboard_status(
                "Error — disable VNC sharing before starting automatic SSH sync".into(),
            );
            return;
        }
    }
    let auto = &mut state.files.auto;
    if auto.checked.elapsed() >= Duration::from_millis(250) {
        auto.checked = Instant::now();
        auto.interactive = native::interactive_session();
    }
    let matches = state
        .files
        .profile
        .as_ref()
        .is_some_and(|p| Some(p.id) == auto.profile);
    let run = auto.prefs.enabled
        && !app.get_native_clipboard_enabled()
        && (auto.prefs.to_mac || auto.prefs.to_windows)
        && auto.interactive
        && matches
        && app.get_connected()
        && app.get_files_connected();
    if auto.generation != state.generation || run != auto.running {
        auto.clear();
        auto.generation = state.generation;
        if run {
            auto.sequence = native::sequence();
            auto.sync
                .reset(auto.prefs.clone(), auto.sequence, auto.now());
            auto.running = true;
        }
    }
    if !run {
        app.set_auto_clipboard_status(
            if app.get_native_clipboard_enabled() {
                "Paused — native Apple clipboard selected"
            } else if !auto.prefs.enabled {
                "Paused — automatic sharing is off"
            } else if !auto.prefs.to_mac && !auto.prefs.to_windows {
                "Paused — choose a synchronization direction"
            } else if !auto.interactive {
                "Paused — Windows session is locked or inactive"
            } else {
                "Paused — connect SSH securely to resume"
            }
            .into(),
        );
    }
}
pub(super) fn tick(app: &App, state: &mut State) -> Result<()> {
    let auto = &mut state.files.auto;
    if !auto.running || auto.observed.elapsed() < Duration::from_millis(100) {
        return Ok(());
    }
    auto.observed = Instant::now();
    let now = auto.now();
    let seq = native::sequence();
    if seq != auto.sequence {
        // The open clipboard snapshot includes its own sequence; a competing application wins.
        let snapshot = if auto.prefs.to_mac {
            native::read_snapshot(window_handle(app)?)
        } else {
            Ok(None)
        };
        match snapshot {
            Ok(Some((sequence, text))) => {
                auto.sequence = sequence;
                auto.sync.observe(sequence, Some(text), now);
            }
            Ok(None) => {
                auto.sequence = seq;
                auto.sync.observe(seq, None, now);
            }
            Err(_) => {
                // A busy clipboard is retried. Invalidate remote results via the current OS sequence.
                app.set_auto_clipboard_status(
                    "Paused — Windows clipboard is busy or text is unsupported".into(),
                );
                return Ok(());
            }
        }
    }
    if app.get_files_busy() {
        return Ok(());
    }
    if let Some(request) = auto.sync.start(now) {
        let id = request.id;
        if let Some(client) = &state.files.client {
            if client
                .command(Command::AutoClipboard {
                    request,
                    cancel: auto.cancel.clone(),
                })
                .is_err()
            {
                auto.sync.failed(id, now);
                app.set_auto_clipboard_status(
                    "Error — SSH clipboard queue unavailable; retrying with backoff".into(),
                );
            } else {
                app.set_auto_clipboard_status("Syncing — encrypted plain text".into());
            }
        }
    }
    Ok(())
}
pub(super) fn complete(app: &App, state: &mut State, id: u64, result: Result<Reply>) -> Result<()> {
    // Recheck at application time; a lock or disconnect can occur while SSH is awaiting output.
    guard(app, state);
    if !state.files.auto.running
        || !state.files.auto.sync.expects(id)
        || !native::interactive_session()
    {
        return Ok(());
    }
    let now = state.files.auto.now();
    match result {
        Ok(reply) => {
            let text = state
                .files
                .auto
                .sync
                .complete(id, reply, native::sequence(), now);
            if let Some(text) = text
                && !app.get_files_busy()
            {
                scope(&state.files)?.event("clipboard.auto.apply_requested",Level::Info,json!({"clipboard.direction":"mac_to_windows","clipboard.text_bytes":text.len(),"clipboard.request_id":id}))?;
                match native::write_if_sequence(
                    window_handle(app)?,
                    &text,
                    state.files.auto.sequence,
                ) {
                    Ok(Some(sequence)) => {
                        state.files.auto.sequence = sequence;
                        state.files.auto.sync.applied(sequence, &text);
                        state.clipboard.sent(sequence);
                    }
                    Ok(None) => (), // Another application copied newer data while the result was in transit.
                    Err(_) => {
                        app.set_auto_clipboard_status(
                            "Error — Windows clipboard unavailable; retrying with backoff".into(),
                        );
                        return Ok(());
                    }
                }
            }
            app.set_auto_clipboard_status("Connected — automatic text sync active".into());
        }
        Err(error) => {
            state.files.auto.sync.failed(id, now);
            // Transport error messages from this path are fixed, never remote text or parser excerpts.
            app.set_auto_clipboard_status(format!("Error — {error}").into());
        }
    }
    Ok(())
}
pub(super) fn wire(app: &App, state: Rc<RefCell<State>>) {
    let weak = app.as_weak();
    app.on_auto_clipboard_changed(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let result = (|| {
            let mut state = state.borrow_mut();
            // Revoke the previous direction/permission before validation or any disk wait.
            state.files.auto.clear();
            let id = state
                .connection_profile
                .as_ref()
                .ok_or_else(|| Error::Clipboard("Connect to a Mac first".into()))?
                .id;
            let prefs = Preferences {
                enabled: app.get_auto_clipboard_enabled(),
                to_mac: app.get_auto_clipboard_to_mac(),
                to_windows: app.get_auto_clipboard_to_windows(),
                max_bytes: app.get_auto_clipboard_limit().parse().unwrap_or(0),
            };
            if !prefs.valid() {
                return Err(Error::Clipboard(
                    "Auto sync text limit must be 1–1048576 UTF-8 bytes".into(),
                ));
            }
            let old = state.settings.clipboard_auto.clone();
            state.settings.clipboard_auto.retain(|p| p.profile_id != id);
            state
                .settings
                .clipboard_auto
                .push(maccanvas::profiles::ClipboardPreference {
                    profile_id: id,
                    preferences: prefs.clone(),
                });
            let saved = state.audit.critical("clipboard.auto.preferences_requested",Level::Info,json!({"profile.id":id,"clipboard.auto.enabled":prefs.enabled,"clipboard.to_mac":prefs.to_mac,"clipboard.to_windows":prefs.to_windows,"clipboard.max_bytes":prefs.max_bytes}))
                .and_then(|_| state.settings.save_auto_clipboard(&state.path));
            if let Err(error) = saved {
                state.settings.clipboard_auto = old;
                return Err(error);
            }
            state.files.auto.clear();
            load(&app, &mut state);
            Ok(())
        })();
        if let Err(error) = result {
            let mut state = state.borrow_mut();
            let id = state.connection_profile.as_ref().map(|profile| profile.id);
            if let Some(entry) = state.settings.clipboard_auto.iter_mut().find(|entry| Some(entry.profile_id) == id) {
                entry.preferences.enabled = false;
            }
            state.files.auto.clear();
            state.files.auto.profile = None;
            load(&app, &mut state);
            app.set_auto_clipboard_status(format!("Error — sharing stopped; preferences not saved: {error}").into());
        }
        // There must be only one automatic clipboard writer. Preserve the existing VNC implementation.
        if app.get_auto_clipboard_enabled() && app.get_clipboard_shared() {
            app.set_clipboard_shared(false);
            app.invoke_clipboard_changed();
        }
    });
}
