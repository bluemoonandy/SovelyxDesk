//! Single-flight text synchronization. No clipboard contents or hashes are logged.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub enabled: bool,
    pub to_mac: bool,
    pub to_windows: bool,
    pub max_bytes: usize,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            to_mac: true,
            to_windows: true,
            max_bytes: 1024 * 1024,
        }
    }
}
impl Preferences {
    pub fn valid(&self) -> bool {
        (1..=1024 * 1024).contains(&self.max_bytes)
    }
}

pub struct Request {
    pub id: u64,
    pub sequence: u32,
    pub since: Option<i64>,
    pub read_text: bool,
    pub text: Option<Zeroizing<String>>,
    pub max_bytes: usize,
}

pub struct Reply {
    pub revision: i64,
    pub applied: bool,
    pub text: Option<Zeroizing<String>>,
}

struct Pending {
    sequence: u32,
    text: Zeroizing<String>,
    digest: [u8; 32],
}
struct Flight {
    id: u64,
    sequence: u32,
    digest: Option<[u8; 32]>,
}

#[derive(Default)]
pub struct Sync {
    prefs: Preferences,
    sequence: u32,
    revision: Option<i64>,
    apply_revision: Option<i64>,
    digest: Option<[u8; 32]>,
    pending: Option<Pending>,
    flight: Option<Flight>,
    serial: u64,
    changed_at: u64,
    next_at: u64,
    failures: u32,
}
impl Sync {
    /// Re-baseline on authorization, connection, direction or lock changes. Old replies are invalid.
    pub fn reset(&mut self, prefs: Preferences, sequence: u32, now: u64) {
        self.serial = self.serial.wrapping_add(1);
        self.prefs = prefs;
        self.sequence = sequence;
        self.revision = None;
        self.apply_revision = None;
        self.digest = None;
        self.pending = None;
        self.flight = None;
        self.changed_at = now;
        self.next_at = now;
        self.failures = 0;
    }
    pub fn observe(&mut self, sequence: u32, text: Option<Zeroizing<String>>, now: u64) {
        if sequence == self.sequence {
            return;
        }
        self.sequence = sequence;
        self.changed_at = now;
        // A fresh external copy is an intent, even when its bytes equal an older copy.
        // Only applied() establishes the sequence of our own write for echo suppression.
        self.digest = None;
        self.pending = text
            .filter(|text| self.prefs.enabled && self.prefs.to_mac && self.accepts(text))
            .map(|text| Pending {
                sequence,
                digest: hash(&text),
                text,
            });
    }
    pub fn accepts(&self, text: &str) -> bool {
        !text.is_empty() && text.len() <= self.prefs.max_bytes && !text.contains('\0')
    }
    pub fn busy(&self) -> bool {
        self.flight.is_some()
    }
    pub fn expects(&self, id: u64) -> bool {
        self.flight.as_ref().is_some_and(|flight| flight.id == id)
    }

    pub fn start(&mut self, now: u64) -> Option<Request> {
        if !self.prefs.enabled
            || !self.prefs.valid()
            || self.busy()
            || now < self.next_at
            || (!self.prefs.to_mac && !self.prefs.to_windows)
        {
            return None;
        }
        // A remote baseline precedes any write. Copying before enabling does not share old text.
        let push = self.revision.is_some() && self.pending.is_some();
        if push && now.saturating_sub(self.changed_at) < 150 {
            return None;
        }
        self.serial = self.serial.wrapping_add(1);
        let text = if push {
            self.pending.as_ref().map(|p| p.text.clone())
        } else {
            None
        };
        self.flight = Some(Flight {
            id: self.serial,
            sequence: self.sequence,
            digest: if push {
                self.pending.as_ref().map(|p| p.digest)
            } else {
                None
            },
        });
        Some(Request {
            id: self.serial,
            sequence: self.sequence,
            since: self.revision,
            read_text: self.prefs.to_windows,
            text,
            max_bytes: self.prefs.max_bytes,
        })
    }
    /// Windows changes observed during a remote read win that conflict. Remote writes use CAS.
    pub fn complete(
        &mut self,
        id: u64,
        reply: Reply,
        sequence: u32,
        now: u64,
    ) -> Option<Zeroizing<String>> {
        if self.flight.as_ref().is_none_or(|f| f.id != id) {
            return None;
        }
        let flight = self.flight.take().unwrap();
        let previous = self.revision.replace(reply.revision);
        self.failures = 0;
        self.next_at = now.saturating_add(750);
        if let Some(digest) = flight.digest {
            if reply.applied {
                self.digest = Some(digest);
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.sequence == flight.sequence)
                {
                    self.pending = None;
                }
            }
            return None;
        }
        if previous.is_none()
            || previous == Some(reply.revision)
            || !self.prefs.to_windows
            || sequence != flight.sequence
            || self.pending.is_some()
        {
            return None;
        }
        let text = reply
            .text
            .filter(|text| self.accepts(text) && Some(hash(text)) != self.digest);
        if text.is_some() {
            // A Windows clipboard lock can fail after the SSH read succeeded. Acknowledge
            // the remote revision only after a successful local commit, so polling retries it.
            self.revision = previous;
            self.apply_revision = Some(reply.revision);
        }
        text
    }
    pub fn applied(&mut self, sequence: u32, text: &str) {
        self.sequence = sequence;
        if let Some(revision) = self.apply_revision.take() {
            self.revision = Some(revision);
        }
        self.digest = Some(hash(text));
        self.pending = None;
    }
    pub fn failed(&mut self, id: u64, now: u64) {
        if self.flight.as_ref().is_some_and(|f| f.id == id) {
            self.flight = None;
            self.failures = self.failures.saturating_add(1);
            self.next_at = now.saturating_add((1000u64 << self.failures.min(5)).min(30_000));
        }
    }
}
fn hash(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prefs() -> Preferences {
        Preferences {
            enabled: true,
            ..Preferences::default()
        }
    }
    fn reply(revision: i64, text: Option<&str>) -> Reply {
        Reply {
            revision,
            applied: false,
            text: text.map(|t| Zeroizing::new(t.into())),
        }
    }
    fn ready() -> Sync {
        let mut s = Sync::default();
        s.reset(prefs(), 10, 0);
        let r = s.start(0).unwrap();
        assert!(
            s.complete(r.id, reply(1, Some("old private text")), 10, 0)
                .is_none()
        );
        s
    }
    #[test]
    fn defaults_do_not_share() {
        let mut s = Sync::default();
        assert!(s.start(100).is_none());
    }
    #[test]
    fn unicode_round_trip_and_echo_suppression() {
        let mut s = ready();
        let text = "中文🍎\n    code();";
        s.observe(11, Some(Zeroizing::new(text.into())), 100);
        assert!(s.start(200).is_none());
        let r = s.start(750).unwrap();
        assert_eq!(r.text.as_deref().map(|v| v.as_str()), Some(text));
        assert!(s.start(800).is_none());
        let mut done = reply(2, None);
        done.applied = true;
        assert!(s.complete(r.id, done, 11, 800).is_none());
        let r = s.start(1550).unwrap();
        assert!(s.complete(r.id, reply(3, Some(text)), 11, 1550).is_none());
        let r = s.start(2300).unwrap();
        let remote = s
            .complete(r.id, reply(4, Some("Mac 中文🍎\n    code")), 11, 2300)
            .unwrap();
        s.applied(12, &remote);
        s.observe(12, Some(remote), 2400);
        let r = s.start(3050).unwrap();
        assert!(r.text.is_none());
    }
    #[test]
    fn fast_copies_keep_latest_and_late_reads_cannot_overwrite() {
        let mut s = ready();
        let r = s.start(750).unwrap();
        s.observe(11, Some(Zeroizing::new("first".into())), 760);
        s.observe(12, Some(Zeroizing::new("latest".into())), 780);
        assert!(
            s.complete(r.id, reply(2, Some("remote")), 12, 900)
                .is_none()
        );
        let r = s.start(1650).unwrap();
        assert_eq!(r.since, Some(2));
        assert_eq!(r.sequence, 12);
        assert_eq!(r.text.unwrap().as_str(), "latest");
    }
    #[test]
    fn cas_conflict_retries_current_copy_not_stale_payload() {
        let mut s = ready();
        s.observe(11, Some(Zeroizing::new("A".into())), 100);
        let r = s.start(750).unwrap();
        s.observe(12, Some(Zeroizing::new("B".into())), 800);
        s.complete(r.id, reply(9, None), 12, 900);
        let r = s.start(1650).unwrap();
        assert_eq!(r.since, Some(9));
        assert_eq!(r.text.unwrap().as_str(), "B");
    }
    #[test]
    fn empty_binary_oversized_and_non_text_do_not_clear_remote() {
        for text in [None, Some(""), Some("bad\0text"), Some("123456")] {
            let mut s = ready();
            s.prefs.max_bytes = 5;
            s.observe(11, text.map(|t| Zeroizing::new(t.into())), 100);
            assert!(s.start(750).unwrap().text.is_none());
        }
    }
    #[test]
    fn disconnect_disable_lock_new_mac_and_reconnect_invalidate_old_results() {
        for settings in [Preferences::default(), prefs()] {
            let mut s = ready();
            let old = s.start(750).unwrap();
            s.reset(settings, 50, 800);
            assert!(
                s.complete(old.id, reply(99, Some("old Mac")), 50, 900)
                    .is_none()
            );
            if let Some(r) = s.start(900) {
                assert!(r.since.is_none());
            }
        }
    }
    #[test]
    fn direction_and_background_are_independent_of_focus() {
        let mut s = ready();
        s.prefs.to_mac = false;
        s.observe(11, Some(Zeroizing::new("local".into())), 100);
        let r = s.start(750).unwrap();
        assert!(r.text.is_none());
        assert_eq!(
            s.complete(r.id, reply(2, Some("remote")), 11, 800)
                .unwrap()
                .as_str(),
            "remote"
        );
        s.prefs.to_windows = false;
        assert!(s.start(1600).is_none());
        s.prefs.to_mac = true;
        assert!(!s.start(1600).unwrap().read_text);
    }
    #[test]
    fn explicitly_copying_identical_text_again_is_not_mistaken_for_an_echo() {
        let mut s = ready();
        s.observe(11, Some(Zeroizing::new("A".into())), 100);
        let r = s.start(750).unwrap();
        let mut done = reply(2, None);
        done.applied = true;
        s.complete(r.id, done, 11, 800);
        s.observe(12, Some(Zeroizing::new("A".into())), 1000);
        // Mac may have changed to B since the last snapshot: preserve the user's new A intent.
        let r = s.start(1550).unwrap();
        assert_eq!(r.text.unwrap().as_str(), "A");
    }
    #[test]
    fn receive_only_mode_can_restore_identical_remote_text_after_a_local_copy() {
        let mut s = ready();
        s.prefs.to_mac = false;
        let r = s.start(750).unwrap();
        let text = s.complete(r.id, reply(2, Some("A")), 10, 800).unwrap();
        s.applied(11, &text);
        s.observe(12, None, 900);
        let r = s.start(1550).unwrap();
        assert_eq!(
            s.complete(r.id, reply(3, Some("A")), 12, 1600)
                .unwrap()
                .as_str(),
            "A"
        );
    }
    #[test]
    fn a_failed_windows_apply_retries_the_remote_revision() {
        let mut s = ready();
        let r = s.start(750).unwrap();
        assert!(
            s.complete(r.id, reply(2, Some("remote")), 10, 800)
                .is_some()
        );
        // Simulate OpenClipboard failing: no applied() acknowledgement.
        let r = s.start(1550).unwrap();
        assert_eq!(r.since, Some(1));
        let text = s
            .complete(r.id, reply(2, Some("remote")), 10, 1600)
            .unwrap();
        s.applied(11, &text);
        let r = s.start(2350).unwrap();
        assert_eq!(r.since, Some(2));
    }
    #[test]
    fn bounded_retry_and_single_flight() {
        let mut s = ready();
        let r = s.start(750).unwrap();
        assert!(s.start(1000).is_none());
        s.failed(r.id, 1000);
        assert!(s.start(2999).is_none());
        assert!(s.start(3000).is_some());
        assert!(
            !Preferences {
                max_bytes: 0,
                ..prefs()
            }
            .valid()
        );
        assert!(
            !Preferences {
                max_bytes: 1024 * 1024 + 1,
                ..prefs()
            }
            .valid()
        );
    }
}
