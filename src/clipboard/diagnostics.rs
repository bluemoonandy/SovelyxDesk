//! Metadata-only clipboard diagnostics; no contents, content digests, paths or window titles.
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub generation: u64,
    pub epoch: u64,
    pub connected: bool,
    pub hevc: bool,
    pub enabled: bool,
    pub interactive: bool,
    pub send: bool,
    pub receive: bool,
    pub paused: bool,
    pub baseline: bool,
    pub pending: bool,
    pub capture_busy: bool,
    pub worker_slot_busy: bool,
    pub file_busy: bool,
    pub remote_dirty: bool,
}
impl State {
    pub fn blocked_reason(self) -> &'static str {
        if !self.connected {
            "disconnected"
        } else if !self.hevc {
            "standard_vnc"
        } else if !self.enabled {
            "disabled"
        } else if !self.interactive {
            "windows_session_not_interactive"
        } else if self.paused {
            "bridge_paused"
        } else if !self.send {
            "send_disabled"
        } else if self.capture_busy {
            "capture_worker_busy"
        } else if self.worker_slot_busy {
            "capture_slot_busy"
        } else if self.file_busy {
            "file_worker_busy"
        } else {
            "none"
        }
    }
    fn work_pending(self) -> bool {
        self.enabled
            && self.connected
            && self.hevc
            && self.interactive
            && (self.paused
                || self.pending
                || self.capture_busy
                || self.worker_slot_busy
                || self.remote_dirty
                || (self.baseline && self.receive))
    }
}
#[derive(Default)]
pub struct Tracker {
    last_state: Option<State>,
    local_sequence: Option<u32>,
    sampled_at: Option<u64>,
    emitted_at: Option<u64>,
    stalled_at: Option<u64>,
    progress_at: u64,
    phase: &'static str,
    problem: Option<&'static str>,
    awaiting_write: bool,
    work_was_pending: bool,
    last_stage: Option<(&'static str, u32, u64)>,
}
pub struct Record {
    pub name: &'static str,
    pub warning: bool,
    pub attributes: Value,
}
impl Tracker {
    pub fn due(&self, now: u64) -> bool {
        self.sampled_at
            .is_none_or(|last| now.saturating_sub(last) >= 250)
    }
    pub fn problem(&mut self, reason: &'static str) {
        self.problem = Some(reason);
    }
    pub fn stage(
        &mut self,
        now: u64,
        stage: &'static str,
        sequence: u32,
        progress: bool,
    ) -> Option<Record> {
        if self.last_stage.is_some_and(|(old, seq, at)| {
            old == stage && seq == sequence && now.saturating_sub(at) < 1000
        }) {
            return None;
        }
        self.last_stage = Some((stage, sequence, now));
        self.phase = stage;
        match stage {
            "send_submitted" => {
                self.awaiting_write = true;
                self.work_was_pending = true;
            }
            "configured" | "encrypted_socket_written" => self.awaiting_write = false,
            _ => (),
        }
        if progress {
            self.progress_at = now;
            self.stalled_at = None;
        }
        Some(Record {
            name: "clipboard.apple.bridge_stage",
            warning: matches!(
                stage,
                "capture_failed" | "bridge_error" | "paused" | "capture_slot_busy"
            ),
            attributes: json!({"clipboard.stage":stage,"clipboard.windows_sequence":sequence}),
        })
    }
    pub fn observe(&mut self, now: u64, state: State, sequence: u32) -> Vec<Record> {
        if !self.due(now) {
            return Vec::new();
        }
        self.sampled_at = Some(now);
        let changed = self.last_state != Some(state);
        if self.last_state.is_none() {
            self.progress_at = now;
        }
        let local_changed = self.local_sequence.is_some_and(|old| old != sequence);
        self.local_sequence = Some(sequence);
        self.last_state = Some(state);
        let mut records = Vec::new();
        if local_changed && state.connected && state.hevc {
            // This is an OS sequence change, not proof of a user copy (our own writes also change it).
            records.push(Record { name:"clipboard.apple.local_change",warning:state.enabled && state.paused,
                attributes:json!({"clipboard.windows_sequence":sequence,"clipboard.blocked_reason":state.blocked_reason(),"clipboard.paused":state.paused}) });
        }
        if changed
            || (state.enabled
                && self
                    .emitted_at
                    .is_none_or(|last| now.saturating_sub(last) >= 15_000))
        {
            self.emitted_at = Some(now);
            records.push(Record {name:"clipboard.apple.bridge_state",warning:state.paused,attributes:json!({
                "session.generation":state.generation,"clipboard.epoch":state.epoch,
                "clipboard.connected":state.connected,"clipboard.hevc":state.hevc,"clipboard.enabled":state.enabled,
                "clipboard.interactive":state.interactive,"clipboard.send_enabled":state.send,"clipboard.receive_enabled":state.receive,
                "clipboard.paused":state.paused,"clipboard.baseline":state.baseline,"clipboard.pending":state.pending,
                "clipboard.capture_busy":state.capture_busy,"clipboard.capture_slot_busy":state.worker_slot_busy,
                "clipboard.file_busy":state.file_busy,"clipboard.remote_dirty":state.remote_dirty,
                "clipboard.blocked_reason":state.blocked_reason(),"clipboard.last_stage":self.phase,"clipboard.last_problem":self.problem,
                "clipboard.awaiting_socket_write":self.awaiting_write,
                "clipboard.no_progress_ms":now.saturating_sub(self.progress_at),"clipboard.windows_sequence":sequence,
            })});
        }
        let work_pending = state.work_pending()
            || (state.enabled
                && state.connected
                && state.hevc
                && state.interactive
                && self.awaiting_write);
        if work_pending && !self.work_was_pending {
            self.progress_at = now;
        }
        self.work_was_pending = work_pending;
        if work_pending
            && now.saturating_sub(self.progress_at) >= 5_000
            && self
                .stalled_at
                .is_none_or(|last| now.saturating_sub(last) >= 15_000)
        {
            self.stalled_at = Some(now);
            records.push(Record {name:"clipboard.apple.stalled",warning:true,attributes:json!({
                "session.generation":state.generation,"clipboard.epoch":state.epoch,
                "clipboard.blocked_reason":state.blocked_reason(),"clipboard.last_stage":self.phase,"clipboard.last_problem":self.problem,
                "clipboard.no_progress_ms":now.saturating_sub(self.progress_at),"clipboard.paused":state.paused,
            })});
        }
        records
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn active() -> State {
        State {
            connected: true,
            hevc: true,
            enabled: true,
            interactive: true,
            send: true,
            receive: true,
            ..State::default()
        }
    }
    #[test]
    fn copies_while_paused_remain_visible_and_warnings_are_bounded() {
        let mut t = Tracker::default();
        let s = State {
            paused: true,
            ..active()
        };
        t.problem("fetch_timeout");
        assert_eq!(t.observe(0, s, 1).len(), 1);
        assert!(t.observe(10, s, 2).is_empty());
        let copy = t.observe(250, s, 2);
        assert_eq!(copy[0].name, "clipboard.apple.local_change");
        assert_eq!(
            copy[0].attributes["clipboard.blocked_reason"],
            "bridge_paused"
        );
        assert!(
            t.observe(5000, s, 2)
                .iter()
                .any(|r| r.name == "clipboard.apple.stalled")
        );
        assert!(t.observe(5250, s, 2).is_empty());
        assert!(
            t.observe(20000, s, 2)
                .iter()
                .any(|r| r.name == "clipboard.apple.stalled")
        );
    }
    #[test]
    fn idle_is_not_a_failure_and_socket_progress_rearms_stall_detection() {
        let mut t = Tracker::default();
        t.observe(0, active(), 4);
        assert!(
            !t.observe(30000, active(), 4)
                .iter()
                .any(|r| r.name == "clipboard.apple.stalled")
        );
        t.stage(31000, "send_submitted", 5, true);
        assert!(
            t.observe(36000, active(), 5)
                .iter()
                .any(|r| r.name == "clipboard.apple.stalled")
        );
        t.stage(36500, "encrypted_socket_written", 5, true);
        assert!(
            !t.observe(37000, active(), 5)
                .iter()
                .any(|r| r.name == "clipboard.apple.stalled")
        );
    }
    #[test]
    fn paused_reason_survives_reconfiguration() {
        let mut t = Tracker::default();
        t.problem("request_rejected");
        t.stage(0, "configured", 1, true);
        let records = t.observe(
            0,
            State {
                paused: true,
                ..active()
            },
            1,
        );
        assert_eq!(
            records[0].attributes["clipboard.last_problem"],
            "request_rejected"
        );
    }
    #[test]
    fn disabled_locked_and_file_only_states_do_not_report_stalls() {
        for s in [
            State {
                enabled: false,
                paused: true,
                ..active()
            },
            State {
                interactive: false,
                pending: true,
                ..active()
            },
            State {
                file_busy: true,
                ..active()
            },
            State {
                connected: false,
                paused: true,
                ..active()
            },
        ] {
            let mut t = Tracker::default();
            t.observe(0, s, 1);
            assert!(
                !t.observe(30000, s, 1)
                    .iter()
                    .any(|r| r.name == "clipboard.apple.stalled")
            );
        }
    }
    #[test]
    fn work_after_long_idle_gets_its_own_grace_period() {
        let mut t = Tracker::default();
        t.observe(0, active(), 1);
        let blocked = State {
            worker_slot_busy: true,
            ..active()
        };
        assert!(
            !t.observe(60000, blocked, 2)
                .iter()
                .any(|r| r.name == "clipboard.apple.stalled")
        );
        assert!(
            t.observe(65000, blocked, 2)
                .iter()
                .any(|r| r.name == "clipboard.apple.stalled")
        );
    }
    #[test]
    fn repeated_slot_contention_is_rate_limited_and_metadata_has_no_payload_fields() {
        let mut t = Tracker::default();
        assert!(t.stage(0, "capture_slot_busy", 9, false).is_some());
        assert!(t.stage(10, "capture_slot_busy", 9, false).is_none());
        for record in t.observe(0, active(), 9) {
            for key in record.attributes.as_object().unwrap().keys() {
                assert!(
                    ![
                        "clipboard.text",
                        "clipboard.content",
                        "clipboard.hash",
                        "clipboard.path",
                        "window.title"
                    ]
                    .contains(&key.as_str())
                );
            }
        }
    }
}
