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
    Stabilizing,
    Recovering,
    Reconnecting,
}
impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Idle => "idle",
            Self::ResumePending => "resume_pending",
            Self::Stabilizing => "stabilizing",
            Self::Recovering => "recovering",
            Self::Reconnecting => "reconnecting",
        }
    }
}

const OBSERVATION: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
pub struct RecoveryEpisode {
    pub id: u64,
    pub started: Instant,
    pub attempts: u32,
    pub first_decoded: Option<Instant>,
    pub last_decoded: Option<Instant>,
    pub first_submitted: Option<Instant>,
    pub observing_until: Option<Instant>,
    observation_decoded: Option<Instant>,
    observation_submitted: Option<Instant>,
}

pub struct ObservationUpdate {
    pub reason: &'static str,
    pub episode: RecoveryEpisode,
}

struct Resume {
    episode: RecoveryEpisode,
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
    next_episode: u64,
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
    pub fn decoded(&mut self, now: Instant) {
        self.waiting_since = None;
        self.reset = false;
        if let Some(r) = &mut self.resume {
            r.episode.first_decoded.get_or_insert(now);
            r.episode.last_decoded = Some(now);
            // A fixed observation window, not a sliding timer extended by every frame.
            r.episode.observing_until.get_or_insert(now + OBSERVATION);
            r.episode.observation_decoded.get_or_insert(now);
        }
    }

    /// Update presentation progress and finish observation even on a static desktop.
    /// Missing UI submission is reported separately, never treated as a network fault.
    pub fn advance(
        &mut self,
        now: Instant,
        submitted: Option<Instant>,
    ) -> Option<ObservationUpdate> {
        let r = self.resume.as_mut()?;
        let until = r.episode.observing_until?;
        let fresh_submission = r.episode.observation_submitted.is_none()
            && submitted.is_some_and(|at| {
                r.episode
                    .observation_decoded
                    .is_some_and(|decoded| at >= decoded)
            });
        if fresh_submission {
            r.episode.observation_submitted = submitted;
            r.episode.first_submitted = r.episode.first_submitted.or(submitted);
        }
        let episode = r.episode;
        if now >= until {
            self.resume = None;
            return Some(ObservationUpdate {
                reason: if episode.observation_submitted.is_some() {
                    "observation_completed"
                } else {
                    "observation_expired_without_submission"
                },
                episode,
            });
        }
        fresh_submission.then_some(ObservationUpdate {
            reason: "presentation_progress",
            episode,
        })
    }

    pub fn episode(&self) -> Option<RecoveryEpisode> {
        self.resume.as_ref().map(|r| r.episode)
    }

    pub fn resume(&mut self, now: Instant) -> bool {
        if let Some(r) = &mut self.resume {
            if r.episode.observing_until.take().is_some() {
                r.episode.observation_decoded = None;
                r.episode.observation_submitted = None;
                r.episode.attempts = r.episode.attempts.saturating_add(1);
                return true;
            }
            return false;
        }
        self.next_episode = self.next_episode.saturating_add(1);
        self.resume = Some(Resume {
            episode: RecoveryEpisode {
                id: self.next_episode,
                started: now,
                attempts: 1,
                first_decoded: None,
                last_decoded: None,
                first_submitted: None,
                observing_until: None,
                observation_decoded: None,
                observation_submitted: None,
            },
            refreshed: false,
            reset: false,
            reconnecting: false,
        });
        true
    }
    pub fn pending(&self) -> bool {
        self.resume
            .as_ref()
            .is_some_and(|r| r.episode.observing_until.is_none())
    }
    pub fn reset_pending(&self) -> bool {
        self.pending() && self.resume.as_ref().is_some_and(|r| r.reset)
    }
    pub fn requested_at(&self) -> Option<Instant> {
        self.resume.as_ref().map(|r| r.episode.started)
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
            } else if r.episode.observing_until.is_some() {
                Phase::Stabilizing
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
            if r.episode.observing_until.is_some() {
                return VideoAction::Wait;
            }
            let wait = now.saturating_duration_since(r.episode.started);
            if wait > Duration::from_secs(30) {
                return VideoAction::Reconnect;
            }
            if wait > Duration::from_secs(10) {
                // A silent server does not justify repeatedly resetting a decoder.
                if self.receiving(now)
                    && self.last_complete.is_some_and(|at| at >= r.episode.started)
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

pub fn presentation_stalled(
    started: Instant,
    decoded: Option<Instant>,
    submitted: Option<Instant>,
    now: Instant,
) -> bool {
    decoded.is_some_and(|decoded| {
        submitted.is_none_or(|at| at < decoded)
            && now.saturating_duration_since(submitted.unwrap_or(started)) > Duration::from_secs(2)
    })
}

/// Cap automatic transport retries without changing the credential/Hello gate.
pub fn automatic_retry_allowed(attempt: u32) -> bool {
    (1..=3).contains(&attempt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_decode_must_not_restart_the_original_resume_deadline() {
        let start = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(start);
        w.packet(start + Duration::from_secs(1));
        w.decoded(start + Duration::from_secs(1));
        assert!(w.resume(start + Duration::from_secs(3)));
        assert_eq!(w.requested_at(), Some(start));
        assert_eq!(
            w.action(start + Duration::from_secs(31)),
            VideoAction::Reconnect
        );
    }
    #[test]
    fn a_single_submitted_static_frame_finishes_without_more_video() {
        let start = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(start);
        let decoded = start + Duration::from_secs(1);
        w.packet(decoded);
        w.decoded(decoded);
        assert_eq!(w.phase(decoded), Phase::Stabilizing);
        let submitted = decoded + Duration::from_millis(20);
        assert_eq!(
            w.advance(submitted, Some(submitted)).unwrap().reason,
            "presentation_progress"
        );
        let done = w.advance(decoded + OBSERVATION, Some(submitted)).unwrap();
        assert_eq!(done.reason, "observation_completed");
        assert_eq!(done.episode.attempts, 1);
        assert_eq!(done.episode.first_decoded, Some(decoded));
        assert_eq!(done.episode.first_submitted, Some(submitted));
        assert!(w.episode().is_none());
        assert_eq!(
            w.action(start + Duration::from_secs(1800)),
            VideoAction::Wait
        );
    }
    #[test]
    fn decoded_but_unsubmitted_is_bounded_and_never_reauthenticates() {
        let start = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(start);
        w.decoded(start + Duration::from_secs(1));
        assert!(
            w.advance(start + Duration::from_secs(2), Some(start))
                .is_none()
        );
        assert_eq!(w.action(start + Duration::from_secs(31)), VideoAction::Wait);
        let done = w.advance(start + Duration::from_secs(31), None).unwrap();
        assert_eq!(done.reason, "observation_expired_without_submission");
        assert!(done.episode.first_submitted.is_none());
        assert!(w.episode().is_none());
        assert_eq!(w.action(start + Duration::from_secs(32)), VideoAction::Wait);
    }
    #[test]
    fn continuous_frames_do_not_slide_the_observation_deadline() {
        let start = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(start);
        for second in 1..=5 {
            let at = start + Duration::from_secs(second);
            w.packet(at);
            w.decoded(at);
            w.advance(at, Some(at));
        }
        assert_eq!(
            w.episode().unwrap().observing_until,
            Some(start + Duration::from_secs(6))
        );
        assert_eq!(
            w.advance(
                start + Duration::from_secs(6),
                Some(start + Duration::from_secs(5))
            )
            .unwrap()
            .reason,
            "observation_completed"
        );
        assert!(w.episode().is_none());
    }
    #[test]
    fn repeated_short_recoveries_and_focus_demands_keep_one_deadline() {
        let start = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(start);
        for second in [1, 5, 9, 13, 17, 21, 25] {
            let decoded = start + Duration::from_secs(second);
            w.packet(decoded);
            w.decoded(decoded);
            w.advance(decoded, Some(decoded));
            let demand = decoded + Duration::from_secs(2);
            assert!(w.resume(demand));
            assert!(!w.resume(demand));
            assert_eq!(w.requested_at(), Some(start));
            assert_eq!(w.episode().unwrap().id, 1);
        }
        assert_eq!(w.episode().unwrap().attempts, 8);
        assert_eq!(
            w.action(start + Duration::from_secs(31)),
            VideoAction::Reconnect
        );
        w.reconnecting();
        assert_eq!(
            w.phase(start + Duration::from_secs(31)),
            Phase::Reconnecting
        );
        assert!(!w.resume(start + Duration::from_secs(32)));
    }
    #[test]
    fn focus_return_after_completed_observation_starts_a_new_episode() {
        let start = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(start);
        w.decoded(start + Duration::from_secs(1));
        w.advance(
            start + Duration::from_secs(6),
            Some(start + Duration::from_secs(2)),
        );
        assert!(w.resume(start + Duration::from_secs(60)));
        assert_eq!(w.episode().unwrap().id, 2);
        assert_eq!(w.requested_at(), Some(start + Duration::from_secs(60)));
    }
    #[test]
    fn explicit_resume_must_expire_even_when_no_video_packet_arrives() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.packet(now);
        w.decoded(now);
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
        w.decoded(now);
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
    fn successful_decode_stops_retries_and_finishes_finite_observation() {
        let now = Instant::now();
        let mut w = VideoWatchdog::default();
        w.resume(now);
        w.refreshed();
        w.decoded(now);
        assert!(!w.pending());
        assert_eq!(w.requested_at(), Some(now));
        let done = w.advance(now + Duration::from_secs(5), Some(now)).unwrap();
        assert_eq!(done.reason, "observation_completed");
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
    fn first_and_static_decoded_frames_without_submission_are_visible_in_diagnostics() {
        let start = Instant::now();
        let decoded = start + Duration::from_millis(50);
        assert!(!presentation_stalled(
            start,
            Some(decoded),
            None,
            start + Duration::from_secs(1)
        ));
        assert!(presentation_stalled(
            start,
            Some(decoded),
            None,
            start + Duration::from_secs(3)
        ));
        let submitted = start + Duration::from_secs(1);
        let next = submitted + Duration::from_millis(50);
        assert!(presentation_stalled(
            start,
            Some(next),
            Some(submitted),
            start + Duration::from_secs(4)
        ));
        assert!(!presentation_stalled(
            start,
            Some(next),
            Some(next + Duration::from_millis(1)),
            start + Duration::from_secs(4)
        ));
        assert!(!presentation_stalled(
            start,
            None,
            None,
            start + Duration::from_secs(4)
        ));
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
