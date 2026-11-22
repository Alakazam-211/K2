//! Out of usage, daemon side (0.46.17): when an agent's LLM login
//! runs out (a subscription limit, an API key's credit), K2 posts one
//! system line on that agent's Thread and sends `llm_out_of_usage` on the
//! session bus, so people see it instead of an agent that went quiet.
//!
//! The signals and the words are `k2_core::llm_accounts::out_of_usage`.
//! Three small hooks feed this module, each from where the daemon already
//! reads the signal for a live session (pane):
//! - [`on_hook`]: Claude's `StopFailure` hook (`rate_limit`,
//!   `billing_error`), from `hook_ingest`;
//! - [`on_statusline`]: Claude's statusLine `rate_limits` at 100%, from
//!   `hook_ingest` (beside the usage feed);
//! - [`on_transcript_lines`]: Claude's API-error record and Codex's
//!   `token_count` / `error` events, from `activity_transcript`.
//!
//! Then, off the caller's thread: the pane's agent (its sidecar address,
//! else its workspace), the login it runs on (`llm_account_for`, the
//! spawn's own rules; the statusLine's slot when it names one), what
//! happens next (auto-rotate moved the default, another login has room,
//! or none), and one notice per agent and login per event
//! ([`ou::Episodes`]). An API key's `rate_limit` is a throttle, not out of
//! tokens: no notice.

use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{DateTime, FixedOffset, Local, Offset, TimeZone, Utc};
use parking_lot::Mutex;
use serde_json::Value;

use k2_core::agent_hooks::envelope::{HookEnvelope, HookSource};
use k2_core::llm_accounts::out_of_usage::{self as ou, Hit, Next, OutKind};
use k2_core::llm_accounts::{self as wallet_core, kind, Entry, Tool};
use k2_core::overlay::SystemNotice;

use crate::subscription_usage::{HarnessUsage, UsageWindow};

/// Who presses Next for auto-rotate (`llm_auto_next::KEY_PREFIX`): a
/// default switched by it carries `switchedBy: auto-next:<person>`.
pub const AUTO_NEXT_PREFIX: &str = "auto-next:";
/// A default auto-rotate switched this recently is the switch away from
/// the login the agent just ran out on.
pub const SWITCH_RECENT_SECS: i64 = 15 * 60;
/// A login at this fraction of a window or more has no room.
const NO_ROOM: f64 = 0.99;
/// One look per pane at most this often (the hook, the statusLine and the
/// transcript all fire for one event).
const PANE_GATE_SECS: i64 = 30;

/// The agent a pane belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Its Thread: `ws` or `ws/handle`.
    pub addr: String,
    pub workspace_id: String,
}

/// The login the agent ran out on, and what happens next.
#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub tool: Tool,
    pub id: Option<String>,
    pub label: String,
    pub api_key: bool,
    /// The reset of its reading's window at the limit, when known.
    pub reading_reset: Option<i64>,
    pub next: Next,
}

/// What the pipeline needs from the daemon (faked in tests).
pub trait World {
    fn place(&self, pane: &str, project_hint: Option<&str>) -> Option<Place>;
    /// `account`: the wallet login the signal itself named (a statusLine
    /// from a slot home).
    fn login(&self, place: &Place, tool: Option<Tool>, account: Option<&str>, now: i64) -> Option<Login>;
    /// Post K2's line on `addr`'s Thread; the doc id.
    fn post(&self, addr: &str, text: &str, notice: SystemNotice) -> Result<String, String>;
    /// The server's UTC offset at `at` (the reset's words).
    fn offset(&self, at: i64) -> FixedOffset;
}

/// One "out" signal from a live session.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub pane: String,
    pub project_hint: Option<String>,
    pub tool: Option<Tool>,
    pub account: Option<String>,
    pub hit: Hit,
}

/// A notice that went up.
#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub addr: String,
    pub workspace_id: String,
    pub doc_id: String,
    pub text: String,
    pub code: String,
    pub tool: Tool,
    pub login: String,
    pub resets_at: Option<i64>,
    pub switched_to: Option<String>,
}

/// The pipeline: one signal → at most one notice.
pub fn handle(world: &dyn World, episodes: &Mutex<ou::Episodes>, sig: &Signal, now: i64) -> Option<Posted> {
    if !ou::is_news(&sig.hit, now) {
        return None;
    }
    let place = world.place(&sig.pane, sig.project_hint.as_deref())?;
    let login = world.login(&place, sig.tool, sig.account.as_deref(), now)?;
    if sig.hit.kind == OutKind::Usage && login.api_key {
        // An API key has no usage window: a `rate_limit` is a throttle.
        return None;
    }
    let resets_at = sig.hit.resets_at.or(login.reading_reset).filter(|r| *r > now);
    // 0.46.17: a heartbeat fire that fails now is the limit, not a failure.
    crate::heartbeat_turned_off::note_out(&place.workspace_id, sig.hit.kind, resets_at, now);
    let key = format!("{}|{}|{}", place.addr, login.tool.as_str(), login.id.as_deref().unwrap_or(&login.label));
    if !episodes.lock().admit(&key, resets_at, now) {
        return None;
    }
    let facts = ou::Facts {
        agent: place.addr.clone(),
        tool: login.tool.as_str().to_string(),
        login: login.label.clone(),
        kind: sig.hit.kind,
        resets_at,
        next: login.next.clone(),
    };
    let text = ou::notice_text(&facts, now, world.offset(resets_at.unwrap_or(now)));
    let notice = ou::notice(&facts);
    let code = notice.code.clone();
    match world.post(&place.addr, &text, notice) {
        Ok(doc_id) => {
            k2_core::log_debug!("[llm-out] {} {} ({}): {code}", place.addr, login.tool.as_str(), sig.hit.source);
            Some(Posted {
                addr: place.addr,
                workspace_id: place.workspace_id,
                doc_id,
                text,
                code,
                tool: login.tool,
                login: login.label,
                resets_at,
                switched_to: match login.next {
                    Next::Switched { label } => Some(label),
                    _ => None,
                },
            })
        }
        Err(e) => {
            episodes.lock().release(&key);
            k2_core::log_debug!("[llm-out] couldn't tell {}'s Thread: {e}", place.addr);
            None
        }
    }
}

// ── The hooks ───────────────────────────────────────────────────────

/// Claude's `StopFailure` (the pane's own agent, not a subagent or a
/// nested run).
pub fn on_hook(env: &HookEnvelope) {
    if env.event != "StopFailure" || env.agent_id.is_some() || env.from_descendant || env.source != HookSource::Claude {
        return;
    }
    let Some(hit) = ou::claude_stop_failure(env.stop_error.as_deref(), env.stop_resets_at) else { return };
    observe(Signal { pane: env.pane.clone(), project_hint: None, tool: Some(Tool::Claude), account: None, hit });
}

/// A `StatusLine` hook body (`{k2_config_dir, statusline}`).
pub fn on_statusline(pane: &str, body: &[u8]) {
    // Most posts are below the limit: skip the parse unless "100" is in it.
    if !body.windows(3).any(|w| w == b"100") {
        return;
    }
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return };
    let Some(hit) = v.pointer("/statusline/rate_limits").filter(|r| r.is_object()).and_then(ou::claude_statusline) else {
        return;
    };
    let account =
        v.get("k2_config_dir").and_then(Value::as_str).and_then(|d| crate::usage_live::slot_account(Tool::Claude, d));
    observe(Signal { pane: pane.to_string(), project_hint: None, tool: Some(Tool::Claude), account, hit });
}

/// New transcript lines of a followed session (`harness`: `claude`,
/// `codex`). The newest line that says "out" counts.
pub fn on_transcript_lines(sid: &str, harness: &str, project_id: Option<&str>, lines: &[String]) {
    if lines.is_empty() || !matches!(harness, "claude" | "codex") {
        return;
    }
    let Some(hit) = lines.iter().rev().find_map(|l| ou::transcript_line(harness, l)) else { return };
    observe(Signal {
        pane: sid.to_string(),
        project_hint: project_id.map(str::to_string),
        tool: Tool::parse(harness),
        account: None,
        hit,
    });
}

fn episodes() -> &'static Mutex<ou::Episodes> {
    static E: OnceLock<Mutex<ou::Episodes>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(ou::Episodes::default()))
}

/// Signals the hooks handed over (tests read these instead of a thread).
#[cfg(test)]
pub(crate) static OBSERVED: Mutex<Vec<Signal>> = Mutex::new(Vec::new());

/// A signal from a hook: past the news check and the per-pane gate, the
/// look runs on its own short thread (the hook, the statusLine and the
/// transcript reader never wait on the database or a Thread write).
fn observe(sig: Signal) {
    let now = Utc::now().timestamp();
    if !ou::is_news(&sig.hit, now) {
        return;
    }
    {
        static GATE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
        let mut g = GATE.get_or_init(|| Mutex::new(HashMap::new())).lock();
        g.retain(|_, at| now - *at < PANE_GATE_SECS);
        if g.contains_key(&sig.pane) {
            return;
        }
        g.insert(sig.pane.clone(), now);
    }
    #[cfg(test)]
    OBSERVED.lock().push(sig);
    #[cfg(not(test))]
    {
        let _ = std::thread::Builder::new().name("llm-out-notice".into()).spawn(move || {
            if let Some(p) = handle(&LiveWorld, episodes(), &sig, Utc::now().timestamp()) {
                crate::session_events::emit_llm_out_of_usage(&p);
            }
        });
    }
    #[cfg(test)]
    let _ = episodes;
}

// ── The live world ──────────────────────────────────────────────────

/// The daemon's own lookups.
pub struct LiveWorld;

/// The latest reset among a reading's windows with no room.
fn reset_at_limit(windows: &[UsageWindow]) -> Option<i64> {
    windows
        .iter()
        .filter(|w| w.used >= NO_ROOM)
        .filter_map(|w| DateTime::parse_from_rfc3339(&w.resets_at).ok().map(|d| d.timestamp()))
        .max()
}

/// A wallet entry's last reading, when it has one.
fn entry_windows(e: &Entry) -> Vec<UsageWindow> {
    e.usage_json
        .as_deref()
        .and_then(|j| serde_json::from_str::<HarnessUsage>(j).ok())
        .map(|h| h.windows)
        .unwrap_or_default()
}

fn has_room(e: &Entry) -> bool {
    entry_windows(e).iter().all(|w| w.used < NO_ROOM)
}

impl World for LiveWorld {
    fn place(&self, pane: &str, project_hint: Option<&str>) -> Option<Place> {
        let project = crate::session_token::workspace_for_session(pane)
            .or_else(|| project_hint.map(str::to_string))
            .filter(|p| !p.trim().is_empty())?;
        let addr = crate::cell_identity::sidecar_thread_addr(&project, pane)
            .unwrap_or_else(|| crate::compute_routes::ws_handle(&project));
        Some(Place { addr, workspace_id: project })
    }

    fn login(&self, place: &Place, tool: Option<Tool>, account: Option<&str>, now: i64) -> Option<Login> {
        let params: HashMap<String, String> = [("addr".to_string(), place.addr.clone())].into_iter().collect();
        let target = crate::llm_account_for::resolve(&params).ok()?;
        let tool = target.tool().or(tool)?;
        let (resolved, out, next) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let r = crate::llm_account_for::resolve_account(&conn, tool, &target).ok()?;
            // The statusLine named its slot: that login, whatever the pin says.
            let named = account.and_then(|a| wallet_core::get(&conn, a).ok().flatten()).filter(|e| e.removed_at.is_none());
            let mut out = match &named {
                Some(e) => (Some(e.id.clone()), e.label.clone(), e.is_api_key()),
                None => (r.account_id.clone(), r.label.clone(), r.kind.as_deref() == Some(kind::API_KEY)),
            };
            let pooled = named.is_none() && r.source == "default";
            let mut next = Next::Nothing;
            if pooled {
                let active = wallet_core::active_info(&conn, tool).ok().flatten();
                let entries = wallet_core::list_for(&conn, tool).unwrap_or_default();
                let auto = active.as_ref().filter(|a| {
                    a.switched_by.as_deref().is_some_and(|b| b.starts_with(AUTO_NEXT_PREFIX))
                        && now - a.switched_at <= SWITCH_RECENT_SECS
                });
                // Auto-rotate moved the default off a login now at its
                // limit: the agent ran out there, and the default moved on.
                let previous = auto.and_then(|a| {
                    entries
                        .iter()
                        .filter(|e| e.id != a.account_id && e.removed_at.is_none() && !e.is_api_key() && !has_room(e))
                        .max_by_key(|e| e.last_used_at.unwrap_or(0))
                        .map(|e| (e.clone(), a.account_id.clone()))
                });
                if let Some((prev, to)) = previous {
                    let to_label = entries.iter().find(|e| e.id == to).map(|e| e.label.clone()).unwrap_or_else(|| to.clone());
                    out = (Some(prev.id.clone()), prev.label.clone(), false);
                    next = Next::Switched { label: to_label };
                } else if let Some(c) = wallet_core::next_signed_in(&conn, tool)
                    .ok()
                    .flatten()
                    .filter(|c| Some(c.id.as_str()) != out.0.as_deref() && has_room(c))
                {
                    next = Next::Available { id: c.id, label: c.label };
                }
            }
            (r, out, next)
        };
        let (id, label, api_key) = out;
        // Outside the database lock: the reading the chip reads.
        let reading_reset = if id == resolved.account_id {
            crate::llm_account_for::reading_for(&resolved, DateTime::<Utc>::from_timestamp(now, 0).unwrap_or_else(Utc::now))
                .and_then(|h| reset_at_limit(&h.windows))
        } else {
            let db = k2_core::db::shared();
            let conn = db.lock();
            id.as_deref()
                .and_then(|i| wallet_core::get(&conn, i).ok().flatten())
                .and_then(|e| reset_at_limit(&entry_windows(&e)))
        };
        Some(Login { tool, id, label, api_key, reading_reset, next })
    }

    fn post(&self, addr: &str, text: &str, notice: SystemNotice) -> Result<String, String> {
        crate::overlay_routes::post_workspace_notice(addr, text, notice)
    }

    fn offset(&self, at: i64) -> FixedOffset {
        Local.timestamp_opt(at, 0).single().map(|t| t.offset().fix()).unwrap_or_else(|| Utc.fix())
    }
}

#[cfg(test)]
#[path = "llm_out_notice_tests.rs"]
mod tests;
