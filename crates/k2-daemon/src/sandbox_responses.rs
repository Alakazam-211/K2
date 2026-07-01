//! F2 — per-session AGENT RESPONSE LOG + the principal→session OWNERSHIP map
//! backing `GET /v1/sandboxes/<id>/messages`.
//!
//! The in-cell agent (`claude`) drives the scoped `/cli/respond` verb over the
//! per-cell UDS (see [`crate::cell_server`]); each call [`append`]s a line here.
//! The external API caller then polls `GET /v1/sandboxes/<id>/messages?since=<seq>`
//! to drain the new lines. Two process-wide maps, both behind the default-OFF
//! `K2_SANDBOX_API` gate (nothing writes here unless a sandbox-API cell exists):
//!
//! - [`RESPONSES`] — `session_id → Vec<RespondEntry>`, the append-only (capped)
//!   response log.
//! - [`OWNERS`] — `session_id → principal_id`, recorded at create time
//!   (`v1_sandboxes::handle_v1_sandboxes`) so [`handle_messages`] can REFUSE a
//!   principal that does not own the session (default-deny; never leak another
//!   principal's transcript). See [`crate::v1_sandboxes::handle_messages`].
//!
//! In-memory only, by design: the log is an ephemeral drain buffer for a live
//! cell, not a durable record — it dies with the daemon exactly as the cell does.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-session retention cap. A session's log is bounded to this many entries;
/// once exceeded the OLDEST are dropped (the `seq` counter keeps climbing, so a
/// slow poller that misses a drop just sees a gap, never stale data). Bounds the
/// worst-case memory of a chatty/never-polled cell to ~1000 lines.
const MAX_ENTRIES_PER_SESSION: usize = 1000;

/// One appended agent response line. Serialized into the `messages` array of the
/// `GET .../messages` response.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RespondEntry {
    /// Monotonic per-session sequence (1-based). Strictly increasing even across
    /// cap-eviction, so `?since=<seq>` is a stable cursor.
    pub seq: u64,
    /// The agent-supplied message text. NOT the request prompt — never a secret
    /// staged by the host; this is the agent's own output.
    pub text: String,
    /// Unix seconds at append time (daemon wall-clock — fine here, not a workflow).
    pub ts: u64,
    /// Whether the agent flagged this as its FINAL response for the turn.
    #[serde(rename = "final")]
    pub final_: bool,
}

/// `session_id → response log`. Process-wide; mirrors `sandbox_quota`'s
/// `LazyLock<Mutex<…>>` shell (the repo's process-singleton idiom).
static RESPONSES: LazyLock<Mutex<HashMap<String, Vec<RespondEntry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `session_id → owning principal id` (`V1Principal::display_id()` —
/// `"owner"` or an API-key UUID). The AUTHZ source of truth for
/// `GET .../messages`: a requester whose principal id != the recorded owner is
/// rejected, so one principal can never read another's transcript.
static OWNERS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Current Unix time in whole seconds (0 if the clock is before the epoch —
/// can't happen on a real box).
fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append an agent response line for `session_id`, returning its assigned `seq`.
/// `seq` is `last.seq + 1` (NOT `len + 1`), so it stays monotonic even after the
/// retention cap drops older entries.
pub fn append(session_id: &str, text: String, final_: bool) -> u64 {
    let mut map = RESPONSES.lock().expect("sandbox responses mutex poisoned");
    let v = map.entry(session_id.to_string()).or_default();
    let seq = v.last().map(|e| e.seq + 1).unwrap_or(1);
    v.push(RespondEntry { seq, text, ts: now_unix_secs(), final_ });
    // Bounded retention: drop the oldest beyond the cap (see MAX_ENTRIES_PER_SESSION).
    if v.len() > MAX_ENTRIES_PER_SESSION {
        let overflow = v.len() - MAX_ENTRIES_PER_SESSION;
        v.drain(0..overflow);
    }
    seq
}

/// Clone every entry with `seq > since_seq` (the new-since-last-poll slice).
/// Unknown session → empty vec (never an error — a just-spawned cell that hasn't
/// responded yet, or an already-torn-down one, both read as "nothing new").
pub fn since(session_id: &str, since_seq: u64) -> Vec<RespondEntry> {
    let map = RESPONSES.lock().expect("sandbox responses mutex poisoned");
    map.get(session_id)
        .map(|v| v.iter().filter(|e| e.seq > since_seq).cloned().collect())
        .unwrap_or_default()
}

/// Record `session_id`'s owning principal id at create time (idempotent —
/// overwrites, but ids are host-minted unique per spawn). Called on the SUCCESS
/// path of `handle_v1_sandboxes` so a later `GET .../messages` can authorize.
pub fn record_owner(session_id: &str, principal_id: &str) {
    let mut map = OWNERS.lock().expect("sandbox owners mutex poisoned");
    map.insert(session_id.to_string(), principal_id.to_string());
}

/// The principal id that owns `session_id`, or `None` if we never recorded it
/// (unknown / not-an-API session). The caller treats `None` as "deny" so an
/// unknown id can never be read.
pub fn owner_of(session_id: &str) -> Option<String> {
    let map = OWNERS.lock().expect("sandbox owners mutex poisoned");
    map.get(session_id).cloned()
}

/// Drop BOTH maps' entries for `session_id` — called from the authoritative
/// cell-teardown point (`v2_spawn` ChildExit, which fires on clean exit AND
/// crash/kill). Without this the `RESPONSES`/`OWNERS` key sets grow for the
/// life of the daemon (the per-session vec is capped, but the number of keys
/// is not). In-memory no-op for the vast majority of sessions that never
/// spawned a sandbox cell, so it's safe to call unconditionally on teardown.
/// A late `GET .../messages` after eviction reads as a uniform 404 (unknown
/// owner) — correct: the cell is gone, its transcript is not retained.
pub fn evict(session_id: &str) {
    RESPONSES
        .lock()
        .expect("sandbox responses mutex poisoned")
        .remove(session_id);
    OWNERS
        .lock()
        .expect("sandbox owners mutex poisoned")
        .remove(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_monotonic_seq_and_since_filters() {
        let sid = "test-sess-mono";
        assert_eq!(append(sid, "a".to_string(), false), 1);
        assert_eq!(append(sid, "b".to_string(), false), 2);
        assert_eq!(append(sid, "c".to_string(), true), 3);

        // since(0) returns all three in order.
        let all = since(sid, 0);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[2].seq, 3);
        assert!(all[2].final_, "third entry carries the final flag");

        // since(2) returns only the tail.
        let tail = since(sid, 2);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].seq, 3);

        // since(>=last) is empty.
        assert!(since(sid, 3).is_empty());
        // unknown session is empty, never an error.
        assert!(since("no-such-session", 0).is_empty());
    }

    #[test]
    fn final_serializes_as_the_json_key_final() {
        let e = RespondEntry { seq: 1, text: "hi".to_string(), ts: 7, final_: true };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j.get("final").and_then(|v| v.as_bool()), Some(true));
        assert!(j.get("final_").is_none(), "the rust field name must not leak");
    }

    #[test]
    fn retention_cap_drops_oldest_but_keeps_seq_monotonic() {
        let sid = "test-sess-cap";
        let mut last = 0;
        for i in 0..(MAX_ENTRIES_PER_SESSION + 5) {
            last = append(sid, format!("m{i}"), false);
        }
        // seq kept climbing past the cap.
        assert_eq!(last, (MAX_ENTRIES_PER_SESSION + 5) as u64);
        // the vec is bounded.
        let all = since(sid, 0);
        assert_eq!(all.len(), MAX_ENTRIES_PER_SESSION);
        // the oldest were evicted — first retained seq is the (overflow+1)th.
        assert_eq!(all[0].seq, 6, "first 5 entries evicted");
        assert_eq!(all.last().unwrap().seq, last);
    }

    #[test]
    fn owner_record_and_lookup() {
        let sid = "test-sess-owner";
        assert_eq!(owner_of(sid), None, "unknown session has no owner");
        record_owner(sid, "key-abc");
        assert_eq!(owner_of(sid).as_deref(), Some("key-abc"));
    }

    #[test]
    fn evict_drops_both_maps_and_a_later_read_is_denied() {
        let sid = "test-sess-evict";
        record_owner(sid, "key-e");
        append(sid, "line".to_string(), false);
        assert!(owner_of(sid).is_some() && !since(sid, 0).is_empty());
        evict(sid);
        // Both maps forget the session: no owner (→ 404 at the route) and no log.
        assert_eq!(owner_of(sid), None, "owner entry evicted");
        assert!(since(sid, 0).is_empty(), "response log evicted");
    }
}
