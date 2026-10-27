//! One server-side line per push attempt (0.46.3, for the phone team's
//! end-to-end push tests): a phone can't show a push that never arrived,
//! so the daemon says what it sent, which way, and what the relay said.
//!
//! Each attempt is one device in one relay call (or one device the sealed
//! lane skipped). The line goes to the daemon log at the default level
//! ([`crate::log_debug!`], like every other `[push]` line), and the record
//! goes into an in-memory ring of the last [`RING_CAP`] (never the DB),
//! read back by the owner with `k2 push log`.
//!
//! PRIVACY: a record holds ONLY the fields below — a device id cut to 8
//! characters, the kind, the lane, a byte count, a short key fingerprint,
//! the relay's status/error code/path markers and a latency. Never message
//! text, previews, titles, names, conversation ids, tokens or vendor
//! tokens: the record is built from those fields alone, so none can leak.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::{GatewayResponse, PushError, PushEvent, ThreadMsgKind};

/// How many attempts the ring keeps.
pub const RING_CAP: usize = 200;

/// How many characters of a device id (and a key id) a record keeps.
pub const SHORT_LEN: usize = 8;

/// Which lane an attempt went on (prd-e2e-push-v1 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Sealed end to end to the device's key.
    Sealed,
    /// Today's path.
    Legacy,
    /// `sealed-only` generic ("K2 / New message") to an unkeyed phone.
    Generic,
}

impl Lane {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Sealed => "sealed",
            Self::Legacy => "legacy",
            Self::Generic => "generic",
        }
    }
}

/// One push attempt to one device.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushAttempt {
    /// Unix ms when the relay call finished (or the skip happened).
    pub at_ms: i64,
    /// The device id, first [`SHORT_LEN`] characters.
    pub device: String,
    /// What was pushed: `thread_message`, `thread_card`, `thread_secret`,
    /// `nudge`, `badge`, `ticket`, `ticket_comment`, `project`,
    /// `attention`, `heartbeat_failed`.
    pub kind: &'static str,
    /// `sealed` | `legacy` | `generic`.
    pub lane: &'static str,
    /// The bytes this device's push carried: the sealed blob, else the
    /// `payload` JSON. 0 when nothing was sent.
    pub bytes: usize,
    /// The device key's fingerprint (its key id, first [`SHORT_LEN`] hex
    /// characters). Sealed only.
    pub key: Option<String>,
    /// The relay's HTTP status; `None` when no response came back.
    pub status: Option<u16>,
    /// The relay's error code, or a class (`transport`, `not_sent:…`,
    /// `skipped:…`); `None` on success.
    pub error: Option<String>,
    /// The auth path the call took: `s0` (per-daemon), `s1` (sealed,
    /// per-daemon), `shared` (the shared token) or `none` (not sent).
    pub path: &'static str,
    /// The relay's response markers: `s0`, `s1`, `s0+s1` or `-`.
    pub relay: &'static str,
    /// The relay's per-device outcome from `results[]`: `sent`, `failed`
    /// or `dead`; `None` when the response didn't list the device.
    pub vendor: Option<&'static str>,
    /// Round trip to the relay, in ms (both calls when the shared token
    /// was the fallback).
    pub ms: u64,
}

impl PushAttempt {
    /// The log line (no timestamp; [`crate::log_debug!`] adds it).
    pub fn line(&self) -> String {
        let mut s = format!(
            "[push] attempt device={} kind={} lane={} bytes={}",
            self.device, self.kind, self.lane, self.bytes
        );
        if let Some(k) = &self.key {
            s.push_str(&format!(" key={k}"));
        }
        match self.status {
            Some(st) => s.push_str(&format!(" status={st}")),
            None => s.push_str(" status=-"),
        }
        if let Some(e) = &self.error {
            s.push_str(&format!(" error={e}"));
        }
        s.push_str(&format!(" path={} relay={}", self.path, self.relay));
        if let Some(v) = self.vendor {
            s.push_str(&format!(" vendor={v}"));
        }
        s.push_str(&format!(" ms={}", self.ms));
        s
    }
}

/// The first [`SHORT_LEN`] characters of an id.
pub fn short(id: &str) -> String {
    id.trim().chars().take(SHORT_LEN).collect()
}

/// The kind label of an event on a lane (a badge on the sealed lane is a
/// nudge).
pub fn kind_label(event: &PushEvent, lane: Lane) -> &'static str {
    match event {
        PushEvent::ThreadPost(t) => match t.msg_kind {
            ThreadMsgKind::Text => "thread_message",
            ThreadMsgKind::Choice => "thread_card",
            ThreadMsgKind::Secret => "thread_secret",
        },
        PushEvent::Badge(_) if lane == Lane::Sealed => "nudge",
        PushEvent::Badge(_) => "badge",
        PushEvent::FeedbackCreated { .. } => "ticket",
        PushEvent::FeedbackCommented { .. } => "ticket_comment",
        PushEvent::ProjectMessageCreated { .. } | PushEvent::ProjectPost(_) => "project",
        PushEvent::AgentNeedsAttention { .. } => "attention",
        PushEvent::HeartbeatFailed { .. } => "heartbeat_failed",
        PushEvent::ChatApprovalRequested { .. } => "chat_approval",
    }
}

/// A relay error code made safe for a log line: the relay's codes are
/// short `snake_case` words; anything else (free text that could hold a
/// URL or a token) is dropped.
pub fn safe_code(code: &str) -> Option<String> {
    let c = code.trim();
    let ok = !c.is_empty()
        && c.len() <= 48
        && c.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    ok.then(|| c.to_string())
}

/// What one relay call (with its fallback) did, filled in by the post
/// helpers.
#[derive(Debug, Clone)]
pub(crate) struct RelayOutcome {
    pub path: &'static str,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub s0: bool,
    pub s1: bool,
}

impl Default for RelayOutcome {
    fn default() -> Self {
        Self { path: "none", status: None, error: None, s0: false, s1: false }
    }
}

impl RelayOutcome {
    /// Take a relay response (each call overwrites the last).
    pub fn observe(&mut self, path: &'static str, resp: &GatewayResponse) {
        self.path = path;
        self.status = Some(resp.status);
        self.s0 = resp.s0;
        self.s1 = resp.s1;
        self.error = if resp.is_success() { None } else { safe_code(&resp.reason()) };
    }

    /// Take a failed call (no response).
    pub fn observe_err(&mut self, path: &'static str, e: &PushError) {
        self.path = path;
        self.status = None;
        self.s0 = false;
        self.s1 = false;
        self.error = Some(match e {
            PushError::Transport(_) => "transport".to_string(),
            PushError::Remote { status, .. } => format!("http_{status}"),
            PushError::BadConfig(_) => "bad_config".to_string(),
        });
    }

    /// Nothing went out (`class` says why).
    pub fn not_sent(&mut self, class: &str) {
        *self = Self { error: Some(format!("not_sent:{class}")), ..Self::default() };
    }

    fn relay(&self) -> &'static str {
        match (self.s0, self.s1) {
            (true, true) => "s0+s1",
            (true, false) => "s0",
            (false, true) => "s1",
            (false, false) => "-",
        }
    }
}

/// The relay's per-device outcome for `token` in a dispatch response.
pub(crate) fn vendor_outcome(response: Option<&serde_json::Value>, token: &str) -> Option<&'static str> {
    let results = response?.get("results")?.as_array()?;
    let entry = results.iter().find(|e| e.get("token").and_then(|t| t.as_str()) == Some(token))?;
    if entry.get("dead").and_then(|d| d.as_bool()).unwrap_or(false) {
        Some("dead")
    } else if entry.get("sent").and_then(|d| d.as_bool()).unwrap_or(false) {
        Some("sent")
    } else {
        Some("failed")
    }
}

/// One device's part in one relay call.
pub(crate) struct DeviceAttempt<'a> {
    pub device_id: &'a str,
    pub token: &'a str,
    pub bytes: usize,
    pub key_id: Option<&'a str>,
}

/// Log and keep one record per device of a relay call.
pub(crate) fn record_call(
    event: &PushEvent,
    lane: Lane,
    devices: &[DeviceAttempt<'_>],
    out: &RelayOutcome,
    response: Option<&serde_json::Value>,
    elapsed: std::time::Duration,
) {
    let at_ms = super::unix_now_ms();
    for d in devices {
        record(PushAttempt {
            at_ms,
            device: short(d.device_id),
            kind: kind_label(event, lane),
            lane: lane.as_wire(),
            bytes: d.bytes,
            key: d.key_id.map(short),
            status: out.status,
            error: out.error.clone(),
            path: out.path,
            relay: out.relay(),
            vendor: vendor_outcome(response, d.token),
            ms: elapsed.as_millis() as u64,
        });
    }
}

/// A sealed device skipped before any call (`class` = why).
pub(crate) fn record_skip(event: &PushEvent, device_id: &str, key_id: &str, class: &str) {
    record(PushAttempt {
        at_ms: super::unix_now_ms(),
        device: short(device_id),
        kind: kind_label(event, Lane::Sealed),
        lane: Lane::Sealed.as_wire(),
        bytes: 0,
        key: Some(short(key_id)),
        status: None,
        error: Some(format!("skipped:{class}")),
        path: "none",
        relay: "-",
        vendor: None,
        ms: 0,
    });
}

/// A bounded list of attempts, oldest first.
#[derive(Debug, Default)]
pub struct Ring {
    items: VecDeque<PushAttempt>,
}

impl Ring {
    pub const fn new() -> Self {
        Self { items: VecDeque::new() }
    }

    /// Keep `attempt`, dropping the oldest past [`RING_CAP`].
    pub fn push(&mut self, attempt: PushAttempt) {
        while self.items.len() >= RING_CAP {
            self.items.pop_front();
        }
        self.items.push_back(attempt);
    }

    /// Oldest first; `device` keeps one device's (matched on its first
    /// [`SHORT_LEN`] characters).
    pub fn filtered(&self, device: Option<&str>) -> Vec<PushAttempt> {
        let want = device.map(short).filter(|d| !d.is_empty());
        self.items.iter().filter(|a| want.as_ref().is_none_or(|w| &a.device == w)).cloned().collect()
    }
}

fn ring() -> std::sync::MutexGuard<'static, Ring> {
    static RING: Mutex<Ring> = Mutex::new(Ring::new());
    RING.lock().unwrap_or_else(|p| p.into_inner())
}

/// Log one attempt and keep it in the process-wide ring.
pub fn record(attempt: PushAttempt) {
    crate::log_debug!("{}", attempt.line());
    ring().push(attempt);
}

/// The process-wide ring ([`Ring::filtered`]).
pub fn recent(device: Option<&str>) -> Vec<PushAttempt> {
    ring().filtered(device)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(device: &str) -> PushAttempt {
        PushAttempt {
            at_ms: 1,
            device: short(device),
            kind: "badge",
            lane: "legacy",
            bytes: 10,
            key: None,
            status: Some(200),
            error: None,
            path: "s0",
            relay: "s0",
            vendor: Some("sent"),
            ms: 3,
        }
    }

    #[test]
    fn short_keeps_eight_chars_and_safe_code_drops_free_text() {
        assert_eq!(short("0123456789abcdef"), "01234567");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short("ééééééééé"), "éééééééé");
        assert_eq!(safe_code("subdomain_not_owned").as_deref(), Some("subdomain_not_owned"));
        assert_eq!(safe_code("HTTP 403"), None);
        assert_eq!(safe_code("https://api.push.apple.com/3/device/abc"), None);
        assert_eq!(safe_code(&"x".repeat(49)), None);
    }

    #[test]
    fn ring_keeps_the_last_200_and_filters_by_device() {
        let mut ring = Ring::new();
        for i in 0..(RING_CAP + 5) {
            let mut a = attempt(if i % 2 == 0 { "aaaaaaaa-1" } else { "bbbbbbbb-2" });
            a.at_ms = i as i64;
            ring.push(a);
        }
        let all = ring.filtered(None);
        assert_eq!(all.len(), RING_CAP);
        assert_eq!(all[0].at_ms, 5, "the oldest five dropped");
        assert_eq!(all[RING_CAP - 1].at_ms, (RING_CAP + 4) as i64);
        let mine = ring.filtered(Some("aaaaaaaa-and-the-rest-of-the-id"));
        assert_eq!(mine.len(), 100, "filter matches on the first 8 characters");
        assert!(mine.iter().all(|a| a.device == "aaaaaaaa"));
        assert!(ring.filtered(Some("cccccccc")).is_empty());
        assert_eq!(ring.filtered(Some("")).len(), RING_CAP, "a blank filter is no filter");
    }
}
