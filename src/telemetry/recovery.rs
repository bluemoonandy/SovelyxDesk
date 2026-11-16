//! Video recovery policy; independent of sockets, decoders, credentials and GUI timers.
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoAction {
    Wait,
    Refresh,
    Recover,
    Reconnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Healthy,
    Idle,
    ResumePending,
    Recovering,
    Reconnecting,
}
impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Idle => "idle",
            Self::ResumePending => "resume_pending",
            Self::Recovering => "recovering",
            Self::Reconnecting => "reconnecting",
        }
    }
}

struct Resume {
    since: Instant,
    refreshed: bool,
    reset: bool,
    reconnecting: bool,
}

#[derive(Default)]
pub struct VideoWatchdog {
    waiting_since: Option<Instant>,
    last_packet: Option<Instant>,
    last_complete: Option<Instant>,
    resume: Option<Resume>,
    reset: bool,
}
impl VideoWatchdog {
    pub fn packet(&mut self, now: Instant) {
        if !self.receiving(now) && self.waiting_since.is_some() {
            // Quiet desktops retain the old receive-driven grace period. An explicit
            // resume deadline is separate and cannot be extended by isolated packets.
            self.waiting_since = Some(now);
            self.reset = false;
        }
        self.last_packet = Some(now);
    }
    pub fn last_packet(&self) -> Option<Instant> {
        self.last_packet
    }
    pub fn last_complete(&self) -> Option<Instant> {
        self.last_complete
    }
    pub fn complete_frame(&mut self, now: Instant) {
        self.last_complete = Some(now);
        self.expect_picture(now);
    }
    fn receiving(&self, now: Instant) -> bool {
        self.last_packet
            .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(1))
    }
    pub fn expect_picture(&mut self, now: Instant) {
        self.waiting_since.get_or_insert(now);
    }
    pub fn decoded(&mut self) {
        self.waiting_since = None;
        self.resume = None;
        self.reset = false;
    }
    pub fn resume(&mut self, now: Instant) -> bool {
        if self.resume.is_some() {
            return false;
        }
        self.resume = Some(Resume {
            since: now,
            refreshed: false,
            reset: false,
            reconnecting: false,
        });
        true
    }
    pub fn pending(&self) -> bool {
        self.resume.is_some()
    }
    pub fn reset_pending(&self) -> bool {
        self.resume.as_ref().is_some_and(|r| r.reset)
    }
    pub fn requested_at(&self) -> Option<Instant> {
        self.resume.as_ref().map(|r| r.since)
    }
    pub fn refreshed(&mut self) {
        if let Some(r) = &mut self.resume {
            r.refreshed = true;
        }
    }
    pub fn recovery_sent(&mut self) {
        self.reset = true;
        if let Some(r) = &mut self.resume {
            r.reset = true;
            r.refreshed = true;
        }
    }
    pub fn reconnecting(&mut self) {
        if let Some(r) = &mut self.resume {
            r.reconnecting = true;
        }
    }
    pub fn phase(&self, now: Instant) -> Phase {
        if let Some(r) = &self.resume {
            if r.reconnecting {
                Phase::Reconnecting
            } else if r.refreshed || r.reset {
                Phase::Recovering
            } else {
                Phase::ResumePending
            }
        } else if !self.receiving(now) {
            Phase::Idle
        } else if self.reset {
            Phase::Recovering
        } else {
            Phase::Healthy
        }
    }
    pub fn action(&self, now: Instant) -> VideoAction {
        if let Some(r) = &self.resume {
            let wait = now.saturating_duration_since(r.since);
            if wait > Duration::from_secs(30) {
                return VideoAction::Reconnect;
            }
            if wait > Duration::from_secs(10) {
                // A silent server does not justify repeatedly resetting a decoder.
                if self.receiving(now)
                    && self.last_complete.is_some_and(|at| at >= r.since)
                    && !r.reset
                {
                    return VideoAction::Recover;
                }
                if !r.refreshed {
                    return VideoAction::Refresh;
                }
            }
            return VideoAction::Wait;
        }
        if !self.receiving(now) {
            return VideoAction::Wait;
        }
        match self
            .waiting_since
            .map(|since| now.saturating_duration_since(since))
        {
            Some(wait) if wait > Duration::from_secs(30) => VideoAction::Reconnect,
            Some(wait) if wait > Duration::from_secs(10) && !self.reset => VideoAction::Recover,
            _ => VideoAction::Wait,
        }
    }
}

/// Session-local presentation progress. Repainting an old image is not progress.
#[derive(Default)]
pub struct VideoProgress(Mutex<Option<(u64, Instant)>>);
impl VideoProgress {
    pub fn submitted(&self, number: u64, now: Instant) {
        if let Ok(mut last) = self.0.lock()
            && last.is_none_or(|(old, _)| number > old)
        {
            *last = Some((number, now));
        }
    }
    pub fn last_submitted(&self) -> Option<Instant> {
        self.0.lock().ok().and_then(|last| last.map(|(_, at)| at))
    }
}

/// Cap automatic transport retries without changing the credential/Hello gate.
pub fn automatic_retry_allowed(attempt: u32) -> bool {
    (1..=3).contains(&attempt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_resume_must_expire_even_when_no_video_packet_arrives() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.packet(now);
        w.decoded();
        let resume = now + Duration::from_secs(600);
        assert!(w.resume(resume));
        assert_eq!(
            w.action(resume + Duration::from_secs(31)),
            VideoAction::Reconnect
        );
    }
    #[test]
    fn static_zero_fps_desktop_never_recovers_or_reconnects_by_itself() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.packet(now);
        w.decoded();
        for seconds in [10, 600, 1200, 1800, 3600] {
            let at = now + Duration::from_secs(seconds);
            assert_eq!(w.action(at), VideoAction::Wait);
            assert_eq!(w.phase(at), Phase::Idle);
        }
    }
    #[test]
    fn silent_resume_retries_refresh_once_without_resetting_the_decoder() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(now);
        assert_eq!(w.action(now + Duration::from_secs(10)), VideoAction::Wait);
        let retry = now + Duration::from_secs(11);
        assert_eq!(w.action(retry), VideoAction::Refresh);
        w.refreshed();
        assert_eq!(w.phase(retry), Phase::Recovering);
        for second in 12..=30 {
            assert_eq!(
                w.action(now + Duration::from_secs(second)),
                VideoAction::Wait
            );
        }
        assert_eq!(
            w.action(now + Duration::from_secs(31)),
            VideoAction::Reconnect
        );
    }
    #[test]
    fn repeated_input_and_sparse_metadata_cannot_extend_resume_forever() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(now);
        for second in 1..=31 {
            let at = now + Duration::from_secs(second);
            assert!(!w.resume(at));
            w.packet(at);
        }
        assert_eq!(w.requested_at(), Some(now));
        assert_eq!(
            w.action(now + Duration::from_secs(31)),
            VideoAction::Reconnect
        );
    }
    #[test]
    fn arriving_video_without_decode_resets_once_and_then_reconnects() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(now);
        let at = now + Duration::from_secs(11);
        w.packet(at);
        w.complete_frame(at);
        assert_eq!(w.action(at), VideoAction::Recover);
        w.recovery_sent();
        for second in 12..=30 {
            let at = now + Duration::from_secs(second);
            w.packet(at);
            assert_eq!(w.action(at), VideoAction::Wait);
        }
        assert_eq!(
            w.action(now + Duration::from_secs(31)),
            VideoAction::Reconnect
        );
    }
    #[test]
    fn incomplete_packets_do_not_trigger_a_decoder_reset() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(now);
        let at = now + Duration::from_secs(11);
        w.packet(at);
        assert_eq!(w.action(at), VideoAction::Refresh);
        w.refreshed();
        assert_eq!(w.action(at), VideoAction::Wait);
        w.complete_frame(at);
        assert_eq!(w.action(at), VideoAction::Recover);
    }
    #[test]
    fn successful_decode_cancels_all_resume_deadlines() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(now);
        w.refreshed();
        w.decoded();
        assert!(!w.pending());
        assert_eq!(w.requested_at(), None);
        assert_eq!(w.action(now + Duration::from_secs(1800)), VideoAction::Wait);
        assert!(w.resume(now + Duration::from_secs(1801)));
    }
    #[test]
    fn decoder_stall_also_has_only_one_automatic_reset_per_window() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.packet(now);
        w.expect_picture(now);
        let at = now + Duration::from_secs(11);
        w.packet(now + Duration::from_millis(10500));
        w.packet(at);
        // The silent interval renews the legacy receive-driven timer.
        w.expect_picture(at);
        assert_eq!(w.action(at), VideoAction::Wait);
        for second in 12..=22 {
            w.packet(now + Duration::from_secs(second) - Duration::from_millis(500));
            w.packet(now + Duration::from_secs(second));
        }
        assert_eq!(
            w.action(now + Duration::from_secs(22)),
            VideoAction::Recover
        );
        w.recovery_sent();
        assert_eq!(w.action(now + Duration::from_secs(22)), VideoAction::Wait);
    }
    #[test]
    fn duplicate_repaints_do_not_hide_a_stalled_presentation() {
        let now = Instant::now();
        let p = VideoProgress::default();
        p.submitted(7, now);
        p.submitted(6, now + Duration::from_secs(59));
        p.submitted(7, now + Duration::from_secs(60));
        assert_eq!(p.last_submitted(), Some(now));
        p.submitted(8, now + Duration::from_secs(61));
        assert_eq!(p.last_submitted(), Some(now + Duration::from_secs(61)));
    }
    #[test]
    fn reconnect_attempts_are_finite() {
        for attempt in 1..=3 {
            assert!(automatic_retry_allowed(attempt));
        }
        for attempt in [0, 4, 100, u32::MAX] {
            assert!(!automatic_retry_allowed(attempt));
        }
    }
}
