pub mod hevc;

use std::{
    collections::VecDeque,
    io::{Read, Write},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicI32, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Counters {
    pub received: AtomicU64,
    pub sent: AtomicU64,
    pub encoding: AtomicI32,
    pub decoder: AtomicI32,
    pub hevc: Option<Arc<hevc::Diagnostics>>,
}

impl Counters {
    pub fn new() -> Arc<Self> {
        Self::for_session(false)
    }
    pub fn for_session(is_hevc: bool) -> Arc<Self> {
        let enabled = is_hevc
            && hevc::Diagnostics::enabled(
                std::env::var("MACCANVAS_HEVC_DIAGNOSTICS").ok().as_deref(),
            );
        Arc::new(Self {
            hevc: enabled.then(|| Arc::new(hevc::Diagnostics::new(Instant::now()))),
            encoding: AtomicI32::new(i32::MIN),
            ..Self::default()
        })
    }
}

pub struct Metered<T> {
    pub inner: T,
    pub counters: Arc<Counters>,
}

impl<T: Read> Read for Metered<T> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.counters
            .received
            .fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}

impl<T: Write> Write for Metered<T> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(buffer)?;
        self.counters
            .sent
            .fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<T: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Metered<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(context, buffer);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.counters
                .received
                .fetch_add((buffer.filled().len() - before) as u64, Ordering::Relaxed);
        }
        result
    }
}

impl<T: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Metered<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(context, bytes);
        if let Poll::Ready(Ok(size)) = result {
            self.counters.sent.fetch_add(size as u64, Ordering::Relaxed);
        }
        result
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub download_bps: f64,
    pub upload_bps: f64,
    pub fps: f64,
}

pub const HISTORY: usize = 40;
pub const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

pub struct Monitor {
    previous: (u64, u64),
    last: Instant,
    displayed: u64,
    pub history: VecDeque<Sample>,
}

impl Monitor {
    pub fn new(now: Instant) -> Self {
        Self {
            previous: (0, 0),
            last: now,
            displayed: 0,
            history: VecDeque::from(vec![Sample::default(); HISTORY]),
        }
    }
    pub fn displayed(&mut self) {
        self.displayed += 1;
    }
    pub fn sample(&mut self, counters: &Counters, now: Instant) -> Option<Sample> {
        let elapsed = now.saturating_duration_since(self.last);
        if elapsed < SAMPLE_INTERVAL {
            return None;
        }
        let current = (
            counters.received.load(Ordering::Relaxed),
            counters.sent.load(Ordering::Relaxed),
        );
        let seconds = elapsed.as_secs_f64();
        let sample = Sample {
            download_bps: current.0.saturating_sub(self.previous.0) as f64 * 8.0 / seconds,
            upload_bps: current.1.saturating_sub(self.previous.1) as f64 * 8.0 / seconds,
            fps: self.displayed as f64 / seconds,
        };
        self.previous = current;
        self.last = now;
        self.displayed = 0;
        self.history.pop_front();
        self.history.push_back(sample);
        Some(sample)
    }
    pub fn bars(&self) -> Vec<(f32, f32)> {
        let maximum = self
            .history
            .iter()
            .map(|sample| sample.download_bps.max(sample.upload_bps))
            .fold(1000.0, f64::max);
        self.history
            .iter()
            .map(|sample| {
                (
                    (sample.download_bps / maximum) as f32,
                    (sample.upload_bps / maximum) as f32,
                )
            })
            .collect()
    }
}

pub fn rate(bits: f64) -> String {
    if bits >= 1_000_000_000.0 {
        format!("{:.2} Gbps", bits / 1_000_000_000.0)
    } else if bits >= 1_000_000.0 {
        format!("{:.2} Mbps", bits / 1_000_000.0)
    } else {
        format!("{:.1} kbps", bits.max(0.0) / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn meters_actual_bytes_not_buffer_capacity() {
        let counters = Counters::new();
        let mut input = Metered {
            inner: std::io::Cursor::new(vec![1, 2, 3]),
            counters: counters.clone(),
        };
        let mut buffer = [0; 100];
        assert_eq!(input.read(&mut buffer).unwrap(), 3);
        let mut output = Metered {
            inner: Vec::new(),
            counters: counters.clone(),
        };
        output.write_all(&[0; 7]).unwrap();
        assert_eq!(counters.received.load(Ordering::Relaxed), 3);
        assert_eq!(counters.sent.load(Ordering::Relaxed), 7);
    }
    #[test]
    fn rates_and_fps_use_actual_elapsed_time_and_go_idle() {
        let start = Instant::now();
        let mut monitor = Monitor::new(start);
        let counters = Counters::new();
        counters.received.store(1_000_000, Ordering::Relaxed);
        counters.sent.store(125, Ordering::Relaxed);
        for _ in 0..15 {
            monitor.displayed();
        }
        assert!(
            monitor
                .sample(&counters, start + Duration::from_millis(100))
                .is_none()
        );
        let sample = monitor
            .sample(&counters, start + Duration::from_millis(500))
            .unwrap();
        assert_eq!(sample.download_bps, 16_000_000.0);
        assert_eq!(sample.upload_bps, 2000.0);
        assert_eq!(sample.fps, 30.0);
        let idle = monitor
            .sample(&counters, start + Duration::from_secs(1))
            .unwrap();
        assert_eq!(idle.download_bps, 0.0);
        assert_eq!(idle.fps, 0.0);
        assert_eq!(monitor.history.len(), HISTORY);
    }
    #[test]
    fn formats_decimal_bits_not_bytes() {
        assert_eq!(rate(999_999.0), "1000.0 kbps");
        assert_eq!(rate(1_000_000.0), "1.00 Mbps");
        assert_eq!(rate(0.0), "0.0 kbps");
        assert_eq!(rate(8_000_000_000.0), "8.00 Gbps");
    }
    #[test]
    fn history_is_bounded_and_normalized() {
        let start = Instant::now();
        let mut monitor = Monitor::new(start);
        let counters = Counters::new();
        for index in 1..=200 {
            counters.received.fetch_add(100, Ordering::Relaxed);
            monitor.sample(&counters, start + Duration::from_millis(index * 500));
        }
        assert_eq!(monitor.bars().len(), HISTORY);
        assert!(
            monitor
                .bars()
                .iter()
                .all(|&(rx, tx)| (0.0..=1.0).contains(&rx) && tx == 0.0)
        );
    }
}

/// Emit one aggregate through the existing audit writer; no per-frame messages.
pub fn report_hevc(counters: &Counters, scope: &crate::audit::Scope, now: Instant) {
    let Some(diagnostics) = &counters.hevc else {
        return;
    };
    let Some(s) = diagnostics.snapshot(now, counters.received.load(Ordering::Relaxed)) else {
        return;
    };
    let _ = scope.event(
        "media.hevc.diagnostics",
        crate::audit::Level::Info,
        hevc_summary(s),
    );
}

fn hevc_summary(s: hevc::Snapshot) -> serde_json::Value {
    let rates = hevc::EVENTS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            (
                (*name).to_owned(),
                serde_json::json!({"count":s.events[i], "per_second":s.events[i] as f64/s.seconds}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let timings = hevc::TIMINGS.iter().enumerate().map(|(i, name)| {
        let t=s.timings[i];
        ((*name).to_owned(), serde_json::json!({"samples":t.count,"average_ms":t.average_ms,"p95_upper_ms":t.p95_upper_ms}))
    }).collect::<serde_json::Map<_,_>>();
    let backends = ["software", "nvdec", "d3d11"].iter().enumerate().map(|(i,name)|
        ((*name).to_owned(),serde_json::json!({"decode_calls":s.decode_calls_by_backend[i],"decoded_frames":s.decoded_by_backend[i],"decoded_fps":s.decoded_by_backend[i] as f64/s.seconds})))
        .collect::<serde_json::Map<_,_>>();
    let c = s.configuration;
    serde_json::json!({
        "schema":2,"window_seconds":s.seconds,
        "requested_fps":c.requested_fps,"local_max_fps":c.local_max_fps,"pacer_fps":c.pacer_fps,
        "wire_flags":c.wire_flags,"proposal_bank":c.proposal_bank,
        "negotiated_fps":null,"negotiated_fps_status":"not_exposed_by_current_response_parser",
        "canvas_width":c.width,"canvas_height":c.height,
        "stages":rates,"timings":timings,"backends":backends,
        "last_decode_backend":s.last_backend.map(|i| ["software","nvdec","d3d11"][i]),
        "decode_zero_output_calls":s.decode_zero_output,
        "video_receive_mbps":s.video_bytes as f64*8.0/s.seconds/1e6,
        "session_receive_mbps":s.session_bytes as f64*8.0/s.seconds/1e6,
        "rtp_timestamps_per_second":s.rtp_frames as f64/s.seconds,"rtp_clock_fps":s.rtp_clock_fps,
        "p95_bucket_us":250,"after_render_is_physical_present":false
    })
}

#[cfg(test)]
mod hevc_schema_tests {
    use super::*;
    #[test]
    fn summaries_keep_unknown_negotiation_and_actual_backend_separate_from_requests() {
        let start = Instant::now();
        let d = hevc::Diagnostics::new(start);
        d.configure(hevc::Configuration {
            requested_fps: 60,
            local_max_fps: 30,
            pacer_fps: 30,
            wire_flags: 5,
            proposal_bank: 100,
            width: 1920,
            height: 1080,
        });
        d.event(hevc::Event::CompleteAu, 60);
        d.video_bytes(5_000_000);
        let json = hevc_summary(
            d.snapshot(start + Duration::from_secs(2), 5_000_000)
                .unwrap(),
        );
        assert_eq!(json["schema"], 2);
        for name in [
            "pixel_buffer_allocate",
            "slint_image_construct",
            "slint_image_set",
        ] {
            assert_eq!(json["timings"][name]["samples"], 0);
        }
        assert!(json["negotiated_fps"].is_null());
        assert!(json["last_decode_backend"].is_null());
        assert_eq!(json["requested_fps"], 60);
        assert_eq!(json["pacer_fps"], 30);
        assert_eq!(json["stages"]["complete_au"]["per_second"], 30.0);
        assert_eq!(json["video_receive_mbps"], 20.0);
        assert_eq!(json["timings"]["decode_call"]["samples"], 0);
        assert_eq!(json["after_render_is_physical_present"], false);
    }
}
