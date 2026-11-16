//! Per-connection spans, buffered until the offer is sent to avoid per-step logging I/O.
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::{
    audit::{self, Level, Scope},
    error::Result,
};

pub(super) struct Preparation<'a> {
    scope: &'a Scope,
    origin: Instant,
    origin_utc: String,
    spans: Vec<Value>,
}

impl<'a> Preparation<'a> {
    pub fn new(scope: &'a Scope) -> Self {
        Self {
            scope,
            origin: Instant::now(),
            origin_utc: utc(),
            spans: Vec::new(),
        }
    }

    pub fn run<T>(&mut self, phase: &'static str, work: impl FnOnce() -> Result<T>) -> Result<T> {
        let start = Instant::now();
        let start_utc = utc();
        let result = work();
        let end = Instant::now();
        let end_utc = utc();
        self.spans.push(json!({
            "phase":phase,"start_utc":start_utc,"end_utc":end_utc,
            "start_offset_ns":start.saturating_duration_since(self.origin).as_nanos() as u64,
            "end_offset_ns":end.saturating_duration_since(self.origin).as_nanos() as u64,
            "duration_ns":end.saturating_duration_since(start).as_nanos() as u64,
            "outcome":if result.is_ok() {"success"} else {"failure"},
            "error_type":result.as_ref().err().map(audit::error_type),
        }));
        if result.is_err() {
            self.record("failed", None)?;
        }
        result
    }

    pub fn record(&self, outcome: &'static str, pending_bytes: Option<usize>) -> Result<()> {
        self.scope.event(
            "media.hevc.preparation.summary",
            Level::Info,
            json!({
                "media.preparation.origin_utc":self.origin_utc,
                "media.preparation.duration_ns":self.origin.elapsed().as_nanos() as u64,
                "media.preparation.phases":self.spans,"event.outcome":outcome,
                "control.output_queue_bytes":pending_bytes,
                "control.offer_socket_write_complete":pending_bytes.map(|n|n==0),
            }),
        )
    }

    pub fn flushed(&self) -> Result<()> {
        self.scope.event("media.hevc.preparation.offer_flushed",Level::Info,json!({
            "media.preparation.origin_utc":self.origin_utc,
            "media.preparation.flush_observed_offset_ns":self.origin.elapsed().as_nanos() as u64,
            "control.output_queue_bytes":0,"control.offer_socket_write_complete":true,
        }))
    }
}

fn utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audit::Audit, error::Error};

    #[test]
    fn spans_have_monotonic_bounds_and_never_log_results_or_error_messages() {
        let dir = tempfile::tempdir().unwrap();
        let mut guard = Audit::start(dir.path().join("audit")).unwrap();
        let scope = guard.audit().scope(json!({"test.fixture":"preparation"}));
        let mut p = Preparation::new(&scope);
        assert_eq!(
            p.run("state_prepare", || Ok("PrivateReturnFixture"))
                .unwrap(),
            "PrivateReturnFixture"
        );
        p.record("offer_queued", Some(17)).unwrap();
        p.flushed().unwrap();
        assert!(
            p.run::<()>("offer_build", || Err(Error::Protocol(
                "PrivateErrorFixture".into()
            )))
            .is_err()
        );
        let path = guard.audit().path();
        guard.shutdown().unwrap();
        let log = std::fs::read_to_string(path).unwrap();
        assert!(!log.contains("PrivateReturnFixture"));
        assert!(!log.contains("PrivateErrorFixture"));
        let rows: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let summaries: Vec<_> = rows
            .iter()
            .filter(|r| r["event_name"] == "media.hevc.preparation.summary")
            .collect();
        assert_eq!(summaries.len(), 2);
        assert_eq!(
            summaries[0]["attributes"]["control.offer_socket_write_complete"],
            false
        );
        assert_eq!(summaries[1]["attributes"]["event.outcome"], "failed");
        let spans = summaries[1]["attributes"]["media.preparation.phases"]
            .as_array()
            .unwrap();
        assert_eq!(spans.len(), 2);
        for span in spans {
            let a = span["start_offset_ns"].as_u64().unwrap();
            let b = span["end_offset_ns"].as_u64().unwrap();
            assert_eq!(span["duration_ns"].as_u64().unwrap(), b - a);
            assert!(span["start_utc"].as_str().unwrap().ends_with('Z'));
            assert!(span["end_utc"].as_str().unwrap().ends_with('Z'));
        }
        assert!(
            rows.iter()
                .any(|r| r["event_name"] == "media.hevc.preparation.offer_flushed")
        );
    }
}
