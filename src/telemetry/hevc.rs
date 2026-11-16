//! Opt-in, bounded HEVC pipeline measurements. No payloads or per-frame logging.
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

pub const EVENTS: [&str; 15] = [
    "complete_au",
    "pacer_output",
    "session_delivery",
    "render_submit_unique",
    "after_render_unique",
    "render_callbacks",
    "waiting_keyframe",
    "decode_error",
    "pacer_overwrite",
    "pacer_recovery_clear",
    "session_queue_overwrite",
    "render_request_overwrite",
    "render_output_overwrite",
    "render_stale",
    "rtp_incomplete",
];
pub const TIMINGS: [&str; 10] = [
    "decode_call",
    "gpu_readback",
    "render_submit",
    "software_scale",
    "slint_render_cycle",
    "complete_au_gap",
    "rtp_timestamp_arrival_gap",
    "pixel_buffer_allocate",
    "slint_image_construct",
    "slint_image_set",
];
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Event {
    CompleteAu,
    PacerOutput,
    SessionDelivery,
    RenderSubmitUnique,
    AfterRenderUnique,
    RenderCallbacks,
    WaitingKeyframe,
    DecodeError,
    PacerOverwrite,
    PacerRecoveryClear,
    SessionQueueOverwrite,
    RenderRequestOverwrite,
    RenderOutputOverwrite,
    RenderStale,
    RtpIncomplete,
}
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Timing {
    DecodeCall,
    GpuReadback,
    RenderSubmit,
    SoftwareScale,
    SlintRenderCycle,
    CompleteAuGap,
    RtpTimestampArrivalGap,
    PixelBufferAllocate,
    SlintImageConstruct,
    SlintImageSet,
}

const BUCKET_US: u128 = 250;
const BUCKETS: usize = 1024;
#[derive(Clone)]
struct Histogram {
    count: u64,
    total_ns: u128,
    maximum_ns: u128,
    buckets: [u64; BUCKETS],
}
impl Default for Histogram {
    fn default() -> Self {
        Self {
            count: 0,
            total_ns: 0,
            maximum_ns: 0,
            buckets: [0; BUCKETS],
        }
    }
}
#[derive(Debug, Default, Clone, Copy)]
pub struct Latency {
    pub count: u64,
    pub average_ms: f64,
    pub p95_upper_ms: f64,
}
impl Histogram {
    fn record(&mut self, elapsed: Duration) {
        let ns = elapsed.as_nanos();
        self.count += 1;
        self.total_ns += ns;
        self.maximum_ns = self.maximum_ns.max(ns);
        let index = (elapsed.as_micros() / BUCKET_US).min((BUCKETS - 1) as u128) as usize;
        self.buckets[index] += 1;
    }
    fn summary(&self) -> Latency {
        if self.count == 0 {
            return Latency::default();
        }
        let rank = (self.count * 95).div_ceil(100);
        let mut count = 0;
        let index = self
            .buckets
            .iter()
            .position(|n| {
                count += n;
                count >= rank
            })
            .unwrap();
        let upper_ns = if index == BUCKETS - 1 {
            self.maximum_ns
        } else {
            ((index + 1) as u128 * BUCKET_US * 1000).min(self.maximum_ns)
        };
        Latency {
            count: self.count,
            average_ms: self.total_ns as f64 / self.count as f64 / 1e6,
            p95_upper_ms: upper_ns as f64 / 1e6,
        }
    }
}
#[derive(Debug, Default, Clone, Copy)]
pub struct Configuration {
    pub requested_fps: u16,
    pub local_max_fps: u16,
    pub pacer_fps: u16,
    pub wire_flags: u32,
    pub proposal_bank: u16,
    pub width: u32,
    pub height: u32,
}
pub struct Snapshot {
    pub seconds: f64,
    pub configuration: Configuration,
    pub events: [u64; EVENTS.len()],
    pub timings: [Latency; TIMINGS.len()],
    pub decoded_by_backend: [u64; 3],
    pub decode_calls_by_backend: [u64; 3],
    pub decode_zero_output: u64,
    pub last_backend: Option<usize>,
    pub video_bytes: u64,
    pub session_bytes: u64,
    pub rtp_frames: u64,
    pub rtp_clock_fps: Option<f64>,
}
struct Window {
    since: Instant,
    configuration: Configuration,
    events: [u64; EVENTS.len()],
    timings: [Histogram; TIMINGS.len()],
    decoded: [u64; 3],
    decode_calls: [u64; 3],
    zero_output: u64,
    backend: Option<usize>,
    video_bytes: u64,
    previous_session_bytes: u64,
    rtp_frames: u64,
    rtp_steps: u64,
    rtp_ticks: u64,
    last_rtp: Option<(u32, Instant)>,
    recent_rtp: [Option<u32>; 256],
    rtp_cursor: usize,
    last_au: Option<Instant>,
    last_submit: Option<u64>,
    last_after: Option<u64>,
}
pub struct Diagnostics {
    window: Mutex<Window>,
}
impl Diagnostics {
    pub fn enabled(value: Option<&str>) -> bool {
        value == Some("1")
    }
    pub fn new(now: Instant) -> Self {
        Self {
            window: Mutex::new(Window {
                since: now,
                configuration: Configuration::default(),
                events: [0; EVENTS.len()],
                timings: std::array::from_fn(|_| Histogram::default()),
                decoded: [0; 3],
                decode_calls: [0; 3],
                zero_output: 0,
                backend: None,
                video_bytes: 0,
                previous_session_bytes: 0,
                rtp_frames: 0,
                rtp_steps: 0,
                rtp_ticks: 0,
                last_rtp: None,
                recent_rtp: [None; 256],
                rtp_cursor: 0,
                last_au: None,
                last_submit: None,
                last_after: None,
            }),
        }
    }
    pub fn configure(&self, configuration: Configuration) {
        if let Ok(mut w) = self.window.lock() {
            w.configuration = configuration;
        }
    }
    pub fn event(&self, event: Event, count: u64) {
        if let Ok(mut w) = self.window.lock() {
            w.events[event as usize] += count;
        }
    }
    pub fn timing(&self, timing: Timing, elapsed: Duration) {
        if let Ok(mut w) = self.window.lock() {
            w.timings[timing as usize].record(elapsed);
        }
    }
    pub fn video_bytes(&self, bytes: usize) {
        if let Ok(mut w) = self.window.lock() {
            w.video_bytes += bytes as u64;
        }
    }
    pub fn rtp_packet(&self, timestamp: u32, now: Instant) {
        if let Ok(mut w) = self.window.lock() {
            if w.last_rtp.is_some_and(|(last, _)| last == timestamp)
                || w.recent_rtp.contains(&Some(timestamp))
            {
                return;
            }
            let cursor = w.rtp_cursor;
            w.recent_rtp[cursor] = Some(timestamp);
            w.rtp_cursor = (cursor + 1) % w.recent_rtp.len();
            w.rtp_frames += 1;
            if let Some((last, arrived)) = w.last_rtp {
                let delta = timestamp.wrapping_sub(last);
                // Reordered pictures still count above, but must not corrupt the clock estimate.
                if delta >= 1 << 31 {
                    return;
                }
                w.timings[Timing::RtpTimestampArrivalGap as usize]
                    .record(now.saturating_duration_since(arrived));
                w.rtp_steps += 1;
                w.rtp_ticks += u64::from(delta);
            }
            w.last_rtp = Some((timestamp, now));
        }
    }
    pub fn reset_stream(&self) {
        if let Ok(mut w) = self.window.lock() {
            w.last_rtp = None;
            w.recent_rtp = [None; 256];
            w.rtp_cursor = 0;
            w.last_au = None;
        }
    }
    pub fn complete_au(&self, now: Instant) {
        if let Ok(mut w) = self.window.lock() {
            w.events[Event::CompleteAu as usize] += 1;
            if let Some(previous) = w.last_au {
                w.timings[Timing::CompleteAuGap as usize]
                    .record(now.saturating_duration_since(previous));
            }
            w.last_au = Some(now);
        }
    }
    pub fn decode(&self, backend: usize, elapsed: Duration, frames: Option<usize>) {
        if let Ok(mut w) = self.window.lock() {
            let backend = backend.min(2);
            w.backend = Some(backend);
            w.decode_calls[backend] += 1;
            w.timings[Timing::DecodeCall as usize].record(elapsed);
            if let Some(frames) = frames {
                w.decoded[backend] += frames as u64;
                if frames == 0 {
                    w.zero_output += 1;
                }
            } else {
                w.events[Event::DecodeError as usize] += 1;
            }
        }
    }
    pub fn submitted(&self, frame: u64, elapsed: Duration) {
        if let Ok(mut w) = self.window.lock()
            && w.last_submit != Some(frame)
        {
            w.events[Event::RenderSubmitUnique as usize] += 1;
            w.timings[Timing::RenderSubmit as usize].record(elapsed);
            w.last_submit = Some(frame);
        }
    }
    pub fn after_render(&self) {
        if let Ok(mut w) = self.window.lock()
            && w.last_submit.is_some()
            && w.last_after != w.last_submit
        {
            w.events[Event::AfterRenderUnique as usize] += 1;
            w.last_after = w.last_submit;
        }
    }
    pub fn snapshot(&self, now: Instant, session_bytes: u64) -> Option<Snapshot> {
        let mut w = self.window.lock().ok()?;
        let seconds = now.saturating_duration_since(w.since).as_secs_f64();
        if seconds < 1.0 {
            return None;
        }
        let snapshot = Snapshot {
            seconds,
            configuration: w.configuration,
            events: w.events,
            timings: std::array::from_fn(|i| w.timings[i].summary()),
            decoded_by_backend: w.decoded,
            decode_calls_by_backend: w.decode_calls,
            decode_zero_output: w.zero_output,
            last_backend: w.backend,
            video_bytes: w.video_bytes,
            session_bytes: session_bytes.saturating_sub(w.previous_session_bytes),
            rtp_frames: w.rtp_frames,
            rtp_clock_fps: (w.rtp_ticks != 0)
                .then(|| w.rtp_steps as f64 * 90_000.0 / w.rtp_ticks as f64),
        };
        w.since = now;
        w.events = [0; EVENTS.len()];
        w.timings = std::array::from_fn(|_| Histogram::default());
        w.decoded = [0; 3];
        w.decode_calls = [0; 3];
        w.zero_output = 0;
        w.video_bytes = 0;
        w.previous_session_bytes = session_bytes;
        w.rtp_frames = 0;
        w.rtp_steps = 0;
        w.rtp_ticks = 0;
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_enable_is_explicit() {
        for value in [None, Some("0"), Some("true"), Some("")] {
            assert!(!Diagnostics::enabled(value));
        }
        assert!(Diagnostics::enabled(Some("1")));
    }
    #[test]
    fn normalizes_delayed_windows_and_resets_without_recounting_frames() {
        let start = Instant::now();
        let d = Diagnostics::new(start);
        for n in 0..60 {
            d.complete_au(start + Duration::from_millis(n * 33));
            d.submitted(n, Duration::from_millis(2));
            d.after_render();
        }
        assert!(d.snapshot(start + Duration::from_millis(999), 0).is_none());
        let s = d
            .snapshot(start + Duration::from_secs(2), 5_000_000)
            .unwrap();
        assert_eq!(
            s.events[Event::CompleteAu as usize] as f64 / s.seconds,
            30.0
        );
        assert_eq!(s.session_bytes as f64 * 8.0 / s.seconds / 1e6, 20.0);
        d.submitted(59, Duration::from_millis(2));
        d.after_render();
        let idle = d
            .snapshot(start + Duration::from_secs(3), 5_000_000)
            .unwrap();
        assert!(idle.events.iter().all(|n| *n == 0));
        assert_eq!(idle.session_bytes, 0);
        assert_eq!(idle.timings[Timing::RenderSubmit as usize].count, 0);
    }
    #[test]
    fn quantiles_are_bounded_and_zero_samples_are_explicit() {
        let mut h = Histogram::default();
        assert_eq!(h.summary().count, 0);
        for _ in 0..95 {
            h.record(Duration::from_micros(1100));
        }
        for _ in 0..5 {
            h.record(Duration::from_secs(1));
        }
        let s = h.summary();
        assert_eq!(s.p95_upper_ms, 1.25);
        assert!((s.average_ms - 51.045).abs() < 1e-6);
        h.record(Duration::from_secs(1));
        assert_eq!(h.summary().p95_upper_ms, 1000.0);
    }
    #[test]
    fn decoder_outputs_are_not_assumed_one_per_call() {
        let start = Instant::now();
        let d = Diagnostics::new(start);
        d.decode(1, Duration::from_millis(2), Some(0));
        d.decode(1, Duration::from_millis(3), Some(2));
        d.decode(1, Duration::from_millis(4), None);
        d.decode(0, Duration::from_millis(5), Some(1));
        d.event(Event::PacerOverwrite, 1);
        d.event(Event::SessionQueueOverwrite, 2);
        let s = d.snapshot(start + Duration::from_secs(1), 0).unwrap();
        assert_eq!(s.decoded_by_backend, [1, 2, 0]);
        assert_eq!(s.decode_calls_by_backend, [1, 3, 0]);
        assert_eq!(s.decode_zero_output, 1);
        assert_eq!(s.events[Event::DecodeError as usize], 1);
        assert_eq!(s.timings[Timing::DecodeCall as usize].average_ms, 3.5);
        assert_eq!(s.events[Event::PacerOverwrite as usize], 1);
        assert_eq!(s.events[Event::SessionQueueOverwrite as usize], 2);
    }
    #[test]
    fn render_callbacks_only_count_successfully_submitted_distinct_images() {
        let start = Instant::now();
        let d = Diagnostics::new(start);
        d.after_render();
        d.submitted(1, Duration::from_millis(2));
        d.submitted(1, Duration::from_millis(99));
        d.after_render();
        d.after_render();
        d.submitted(2, Duration::from_millis(3));
        let s = d.snapshot(start + Duration::from_secs(1), 0).unwrap();
        assert_eq!(s.events[Event::RenderSubmitUnique as usize], 2);
        assert_eq!(s.events[Event::AfterRenderUnique as usize], 1);
        assert_eq!(s.timings[Timing::RenderSubmit as usize].average_ms, 2.5);
        assert_eq!(s.last_backend, None);
        d.after_render();
        let next = d.snapshot(start + Duration::from_secs(2), 0).unwrap();
        assert_eq!(next.events[Event::AfterRenderUnique as usize], 1);
    }

    #[test]
    fn worker_and_ui_updates_share_one_consistent_window() {
        let start = Instant::now();
        let d = std::sync::Arc::new(Diagnostics::new(start));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let d = d.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        d.event(Event::SessionDelivery, 1);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            d.snapshot(start + Duration::from_secs(1), 0)
                .unwrap()
                .events[Event::SessionDelivery as usize],
            4000
        );
    }

    #[test]
    fn rtp_cadence_handles_duplicates_reordering_and_clock_wrap() {
        let start = Instant::now();
        let d = Diagnostics::new(start);
        let first = u32::MAX - 1000;
        d.rtp_packet(first, start);
        d.rtp_packet(first, start);
        d.rtp_packet(first - 1, start);
        d.rtp_packet(
            first.wrapping_add(1500),
            start + Duration::from_micros(16667),
        );
        let s = d.snapshot(start + Duration::from_secs(1), 0).unwrap();
        assert_eq!(s.rtp_frames, 3);
        assert_eq!(s.rtp_clock_fps, Some(60.0));
        d.reset_stream();
        d.rtp_packet(1, start + Duration::from_secs(1));
        let s = d.snapshot(start + Duration::from_secs(2), 0).unwrap();
        assert_eq!(s.rtp_clock_fps, None);
    }
}
