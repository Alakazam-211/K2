//! `POST /hook/event` ingest and the per-pane owner registry
//! (prd-daemon-activity-and-thread-working-v1 S1: DA5, DA12–DA15, DA17).
//!
//! Both transports land here: the TCP dispatcher (owner token, or a
//! scoped token pinned to `X-K2-Pane`) and the per-cell socket
//! (`cell_server`). Each post is parsed into a typed
//! [`HookEnvelope`] (the raw body is dropped right after) and put
//! through the owner check ([`k2_core::agent_hooks::owner`]). Only an
//! owner envelope goes on, as an [`IngestEvent`] on [`subscribe`]'s
//! channel, which the activity store ([`crate::activity_store`]) consumes.
//! Foreign and unknown-pane envelopes are counted and change nothing.
//!
//! S2 (DA13): no raw hook drives a lifecycle output any more. The store is
//! the only writer of `workspace_sessions.status` for hook events, and it
//! derives the legacy `agent:lifecycle` / `agent_status_changed` emits
//! from its own row.
//!
//! Errors never block the agent: everything but an auth failure is 204.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::broadcast;

use k2_core::agent_hooks::envelope::{self, HookEnvelope, HookHeaders};
use k2_core::agent_hooks::owner::{
    self, OwnerCheckMode, PaneFacts, PaneOwner, ProcessTable, Sender, Verdict,
};
use k2_core::log_debug;
use k2_core::proc_ancestry::LiveProcessTable;
use k2_core::session::SessionId;

/// Capacity of the ingest channel. A lagging consumer loses the oldest
/// envelopes, never blocks the hook route.
const BUS_CAP: usize = 4096;

/// One thing the activity store must learn from the hook plane.
#[derive(Debug, Clone)]
pub enum IngestEvent {
    /// An envelope from the pane's owner: the only kind that may change
    /// its row.
    Envelope {
        envelope: Arc<HookEnvelope>,
        received_at_ms: i64,
        /// This envelope claimed the pane.
        claimed_now: bool,
        /// The owner moved to (or first revealed) this conversation id.
        conversation_changed: Option<String>,
    },
    /// The pane's owner process exited or its pid was reused (DA14-6):
    /// the store sets the lead idle with reason `agent_exited`.
    OwnerReleased { pane: String, at_ms: i64 },
    /// A legacy `GET /hook/complete` from a live pane (DA13): evidence
    /// only, no payload and no pid.
    Legacy {
        pane: String,
        raw_event: String,
        received_at_ms: i64,
    },
}

fn bus() -> &'static broadcast::Sender<IngestEvent> {
    static BUS: OnceLock<broadcast::Sender<IngestEvent>> = OnceLock::new();
    BUS.get_or_init(|| broadcast::channel(BUS_CAP).0)
}

/// Subscribe to the hook plane (the activity store's input).
pub fn subscribe() -> broadcast::Receiver<IngestEvent> {
    bus().subscribe()
}

fn publish(ev: IngestEvent) {
    // No subscriber yet (S1 alone, or tests) is fine.
    let _ = bus().send(ev);
}

/// The outcome of one post, for the transport and for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestVerdict {
    Accepted,
    Foreign,
    UnknownPane,
    ParseError,
}

impl IngestVerdict {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Foreign => "foreign",
            Self::UnknownPane => "unknown_pane",
            Self::ParseError => "parse_error",
        }
    }
}

/// DA17 counters (per session, and daemon-wide).
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    pub accepted: u64,
    pub foreign: u64,
    pub unknown_pane: u64,
    pub parse_error: u64,
}

impl Counters {
    fn bump(&mut self, v: IngestVerdict) {
        match v {
            IngestVerdict::Accepted => self.accepted += 1,
            IngestVerdict::Foreign => self.foreign += 1,
            IngestVerdict::UnknownPane => self.unknown_pane += 1,
            IngestVerdict::ParseError => self.parse_error += 1,
        }
    }
}

#[derive(Debug, Default)]
struct PaneEntry {
    owner: PaneOwner,
    counters: Counters,
    last_envelope_ms: Option<i64>,
    last_event: Option<String>,
    cli_version: Option<String>,
    source: Option<&'static str>,
}

#[derive(Debug, Default)]
struct State {
    panes: HashMap<String, PaneEntry>,
    totals: Counters,
    /// Legacy `GET /hook/complete` posts seen (A10 reinstall signal).
    legacy: u64,
    /// Last accepted envelope per hook source (`claude`, `cursor`, …).
    last_by_source: HashMap<&'static str, i64>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The live-pane facts for `pane` (DA14-1): a registered v2 session, its
/// PTY child pid, and a premint/resume conversation id from its argv.
pub fn live_pane_facts(pane: &str) -> Option<PaneFacts> {
    let sid = SessionId::parse(pane)?;
    let session = crate::v2_session_map::lookup_by_session_id(&sid)?;
    Some(PaneFacts {
        child_pid: session.child_pid(),
        known_conversation_id: k2_core::workspace::provider_resume::session_id_from_spawn_argv(
            session.program.as_deref().unwrap_or(""),
            &session.args,
        )
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()),
    })
}

/// Ingest one `POST /hook/event` (both transports). Never fails the agent.
pub fn ingest(headers: &HookHeaders, body: &[u8]) -> IngestVerdict {
    ingest_with(
        headers,
        body,
        &LiveProcessTable,
        &live_pane_facts,
        OwnerCheckMode::for_this_platform(),
        now_ms(),
    )
    .0
}

/// [`ingest`] with the process table, pane lookup, mode and clock
/// injected. Returns the verdict and, for an owner envelope, the envelope.
pub fn ingest_with(
    headers: &HookHeaders,
    body: &[u8],
    procs: &dyn ProcessTable,
    facts: &dyn Fn(&str) -> Option<PaneFacts>,
    mode: OwnerCheckMode,
    now: i64,
) -> (IngestVerdict, Option<Arc<HookEnvelope>>) {
    capture(headers, body, now);
    let env = match envelope::parse(headers, body) {
        Ok(env) => env,
        Err(e) => {
            log_debug!("[hook-event] parse_error {} pane={}", e.as_str(), headers.pane);
            let mut st = state().lock();
            st.totals.bump(IngestVerdict::ParseError);
            if let Some(entry) = st.panes.get_mut(&headers.pane) {
                entry.counters.bump(IngestVerdict::ParseError);
            }
            drop(st);
            record(headers.event_hint.as_deref().unwrap_or(""), &headers.pane, None, IngestVerdict::ParseError);
            return (IngestVerdict::ParseError, None);
        }
    };
    let pane_facts = facts(&env.pane);
    let Some(pane_facts) = pane_facts else {
        state().lock().totals.bump(IngestVerdict::UnknownPane);
        record(&env.event, &env.pane, env.tool_name.as_deref(), IngestVerdict::UnknownPane);
        return (IngestVerdict::UnknownPane, None);
    };

    let mut st = state().lock();
    let entry = st.panes.entry(env.pane.clone()).or_default();
    let out = owner::check(&mut entry.owner, Some(&pane_facts), &Sender::of(&env), procs, mode, now);
    let verdict = match out.verdict {
        Verdict::Owner => IngestVerdict::Accepted,
        Verdict::Foreign(reason) => {
            log_debug!(
                "[hook-event] foreign {:?} pane={} event={} pid={:?}",
                reason,
                env.pane,
                env.event,
                env.agent_pid
            );
            IngestVerdict::Foreign
        }
        Verdict::UnknownPane => IngestVerdict::UnknownPane,
    };
    if out.session_id_mismatch {
        log_debug!(
            "[hook-event] session_id {:?} differs from the pane's known conversation (pane={})",
            env.session_id,
            env.pane
        );
    }
    entry.counters.bump(verdict);
    entry.last_envelope_ms = Some(now);
    if verdict == IngestVerdict::Accepted {
        // DA22: the owner's `SessionEnd` releases its claim; a `/clear`
        // re-claims at once with its `SessionStart` (same process).
        if env.event == "SessionEnd" && env.agent_id.is_none() {
            entry.owner.claim = None;
        }
        entry.last_event = Some(env.event.clone());
        entry.cli_version = env.cli_version.clone().or(entry.cli_version.take());
        entry.source = Some(env.source.as_str());
    }
    st.totals.bump(verdict);
    if verdict == IngestVerdict::Accepted {
        st.last_by_source.insert(env.source.as_str(), now);
    }
    drop(st);

    record(&env.event, &env.pane, env.tool_name.as_deref(), verdict);
    if out.released_previous {
        publish(IngestEvent::OwnerReleased { pane: env.pane.clone(), at_ms: now });
    }
    if verdict != IngestVerdict::Accepted {
        return (verdict, None);
    }
    let env = Arc::new(env);
    publish(IngestEvent::Envelope {
        envelope: Arc::clone(&env),
        received_at_ms: now,
        claimed_now: out.claimed_now,
        conversation_changed: out.conversation_changed,
    });
    (verdict, Some(env))
}

fn record(event: &str, pane: &str, tool: Option<&str>, verdict: IngestVerdict) {
    k2_core::agent_hooks::record_recent_hook_event(
        event,
        k2_core::agent_hooks::map_event_type(event),
        pane,
        pane,
        tool,
        Some(verdict.as_str()),
    );
}

/// The legacy `GET /hook/complete` (DA13): only a live pane gets through
/// (it carries no pid or payload, so "pane is live" is the whole owner
/// check), and only as evidence for the activity store. Any legacy post
/// from a live pane means an older app rewrote `notify.sh`, so the
/// installer re-runs now (A10, debounced).
pub fn legacy_complete(params: &HashMap<String, String>) -> &'static str {
    let pane = params.get("paneId").cloned().unwrap_or_default();
    let raw = params.get("eventType").cloned().unwrap_or_default();
    if live_pane_facts(&pane).is_none() {
        state().lock().totals.bump(IngestVerdict::UnknownPane);
        record(&raw, &pane, None, IngestVerdict::UnknownPane);
        return r#"{"success":true}"#;
    }
    state().lock().legacy += 1;
    record(&raw, &pane, None, IngestVerdict::Accepted);
    publish(IngestEvent::Legacy { pane, raw_event: raw, received_at_ms: now_ms() });
    crate::hook_install::note_legacy_hook();
    r#"{"success":true}"#
}

/// Release every claim whose owner process is gone (DA14-6) and drop
/// entries for panes that are no longer live. The activity store's 10 s
/// liveness sweep calls this; the installer tick calls it too.
pub fn sweep(procs: &dyn ProcessTable, is_live: &dyn Fn(&str) -> bool) -> Vec<String> {
    let now = now_ms();
    let mut released = Vec::new();
    let mut st = state().lock();
    st.panes.retain(|pane, _| is_live(pane));
    for (pane, entry) in st.panes.iter_mut() {
        if entry.owner.release_if_dead(procs) {
            released.push(pane.clone());
        }
    }
    drop(st);
    for pane in &released {
        publish(IngestEvent::OwnerReleased { pane: pane.clone(), at_ms: now });
    }
    released
}

/// [`sweep`] against the live process table and v2 session map.
pub fn sweep_live() -> Vec<String> {
    sweep(&LiveProcessTable, &|pane| live_pane_facts(pane).is_some())
}

/// The DA17 per-session block for `/cli/hooks/status`.
pub fn status_json() -> serde_json::Value {
    let st = state().lock();
    let sessions: serde_json::Map<String, serde_json::Value> = st
        .panes
        .iter()
        .map(|(pane, e)| {
            (
                pane.clone(),
                serde_json::json!({
                    "owner": e.owner.claim,
                    "conversationId": e.owner.conversation_id,
                    "counters": e.counters,
                    "lastEnvelopeMs": e.last_envelope_ms,
                    "lastEvent": e.last_event,
                    "cliVersion": e.cli_version,
                    "source": e.source,
                    // The activity store's last evidence source for this
                    // session (`hook` | `title` | `process`; S3 adds
                    // `transcript`).
                    "evidence": crate::activity_store::row_json(pane)
                        .and_then(|r| r["evidenceSource"].as_str().map(str::to_string))
                        .or_else(|| (e.counters.accepted > 0).then(|| "hook".to_string())),
                }),
            )
        })
        .collect();
    serde_json::json!({
        "ownerCheck": OwnerCheckMode::for_this_platform().as_str(),
        "totals": st.totals,
        "legacyPosts": st.legacy,
        "lastEventBySource": st.last_by_source,
        "sessions": sessions,
    })
}

/// Last accepted envelope time for one hook source (`claude`, …).
pub fn last_event_ms(source: &str) -> Option<i64> {
    state().lock().last_by_source.get(source).copied()
}

// ── Fixture capture (§11, A41) ───────────────────────────────────────────

/// `K2_HOOK_CAPTURE_DIR` is honoured only in a debug build or under a
/// `K2_TEST_*` env (A41): the single named exception to "hook content is
/// never written to disk" (DA5). A release daemon ignores it.
pub fn capture_allowed(is_debug_build: bool, has_test_env: bool) -> bool {
    is_debug_build || has_test_env
}

fn capture_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("K2_HOOK_CAPTURE_DIR").filter(|d| !d.is_empty())?;
    let test_env = std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("K2_TEST_"));
    capture_allowed(cfg!(debug_assertions), test_env).then(|| std::path::PathBuf::from(dir))
}

fn capture(headers: &HookHeaders, body: &[u8], now: i64) {
    let Some(dir) = capture_dir() else { return };
    let payload: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let line = serde_json::json!({
        "t_ms": now,
        "kind": "hook",
        "headers": {
            "pane": headers.pane,
            "agentPid": headers.agent_pid,
            "source": headers.source.as_str(),
            "cliVersion": headers.cli_version,
            "event": headers.event_hint,
            "truncated": headers.truncated,
        },
        "payload": payload,
    });
    use std::io::Write;
    let path = dir.join("hooks.jsonl");
    let res = std::fs::create_dir_all(&dir).and_then(|_| {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(f, "{line}")
    });
    if let Err(e) = res {
        log_debug!("[hook-event] capture to {} failed: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::agent_hooks::envelope::HookSource;
    use k2_core::agent_hooks::owner::ProcInfo;
    use std::cell::RefCell;

    /// The ingest registry is process-wide; serialize these tests.
    static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[derive(Default)]
    struct Procs(RefCell<HashMap<i32, ProcInfo>>);
    impl Procs {
        fn add(&self, pid: i32, ppid: i32, start: u64) -> &Self {
            self.0.borrow_mut().insert(pid, ProcInfo { pid, ppid, start_time: start });
            self
        }
    }
    impl ProcessTable for Procs {
        fn info(&self, pid: i32) -> Option<ProcInfo> {
            self.0.borrow().get(&pid).copied()
        }
    }

    fn headers(pane: &str, pid: i32) -> HookHeaders {
        HookHeaders {
            pane: pane.to_string(),
            agent_pid: Some(pid),
            source: HookSource::Claude,
            hook_version: Some(2),
            cli_version: Some("2.1.292".into()),
            truncated: false,
            event_hint: None,
        }
    }

    fn counters(pane: &str) -> Counters {
        state().lock().panes.get(pane).map(|e| e.counters).expect("pane entry")
    }

    #[test]
    fn owner_envelopes_reach_the_channel_and_foreign_ones_do_not() {
        let _g = LOCK.lock();
        let pane = "11111111-1111-4111-8111-111111111111";
        let procs = Procs::default();
        procs.add(10, 1, 1).add(100, 10, 2).add(200, 100, 3).add(300, 200, 4).add(500, 1, 5);
        let facts = |p: &str| (p == pane).then(|| PaneFacts { child_pid: Some(100), known_conversation_id: None });
        let mut rx = subscribe();
        let a = OwnerCheckMode::Ancestry;

        let body = br#"{"hook_event_name":"UserPromptSubmit","session_id":"c1","prompt":"[thread:ws] hi"}"#;
        let (v, env) = ingest_with(&headers(pane, 100), body, &procs, &facts, a, 1);
        assert_eq!(v, IngestVerdict::Accepted);
        assert_eq!(env.expect("owner envelope").prompt_thread_addr.as_deref(), Some("ws"));
        match rx.try_recv().expect("an ingest event") {
            IngestEvent::Envelope { envelope, claimed_now, conversation_changed, .. } => {
                assert_eq!(envelope.event, "UserPromptSubmit");
                assert!(claimed_now);
                assert_eq!(conversation_changed.as_deref(), Some("c1"));
            }
            other => panic!("unexpected {other:?}"),
        }

        // Nested `claude -p` (300): foreign, nothing published.
        let (v, env) = ingest_with(&headers(pane, 300), br#"{"hook_event_name":"Stop"}"#, &procs, &facts, a, 2);
        assert_eq!(v, IngestVerdict::Foreign);
        assert!(env.is_none());
        // T-S1d: stale env from a detached process (500, ppid 1).
        let (v, _) = ingest_with(&headers(pane, 500), br#"{"hook_event_name":"Stop"}"#, &procs, &facts, a, 3);
        assert_eq!(v, IngestVerdict::Foreign);
        assert!(rx.try_recv().is_err(), "foreign envelopes never reach the store");

        // Unknown pane and a bad body are counted.
        let (v, _) = ingest_with(&headers("22222222-2222-4222-8222-222222222222", 100), br#"{"hook_event_name":"Stop"}"#, &procs, &facts, a, 4);
        assert_eq!(v, IngestVerdict::UnknownPane);
        let (v, _) = ingest_with(&headers(pane, 100), b"nope", &procs, &facts, a, 5);
        assert_eq!(v, IngestVerdict::ParseError);

        assert_eq!(
            counters(pane),
            Counters { accepted: 1, foreign: 2, unknown_pane: 0, parse_error: 1 }
        );
        let status = status_json();
        assert_eq!(status["sessions"][pane]["counters"]["foreign"], 2);
        assert_eq!(status["sessions"][pane]["owner"]["via"], "pty_child");
        assert_eq!(status["sessions"][pane]["evidence"], "hook");
        assert!(status["totals"]["unknownPane"].as_u64().expect("count") >= 1);

        // The owner exits: the sweep releases it and tells the store.
        let dead = Procs::default();
        dead.add(10, 1, 1);
        let released = sweep(&dead, &|p| p == pane);
        assert_eq!(released, vec![pane.to_string()]);
        assert!(matches!(rx.try_recv(), Ok(IngestEvent::OwnerReleased { .. })));
        // A pane that is no longer live is dropped from the registry.
        sweep(&dead, &|_| false);
        assert!(state().lock().panes.get(pane).is_none());
    }

    /// A41: a release build without a `K2_TEST_*` env never captures.
    #[test]
    fn capture_is_debug_or_test_only() {
        assert!(!capture_allowed(false, false));
        assert!(capture_allowed(false, true));
        assert!(capture_allowed(true, false));
    }
}
