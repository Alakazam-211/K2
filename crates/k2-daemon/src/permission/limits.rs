//! A1 — Per-workspace and per-server limits, the 24 h denial block and the
//! ask limit (§3.2, §6.6). All SQL over 0218, so every limit survives a
//! daemon restart (the per-hour cap included: it counts stored asks, not an
//! in-memory window).
//!
//! Also the render-time `check` cache (§4.2): `project()` (lane A3) asks
//! [`cached_check`] for a pending request, so a satisfied one is drawn
//! "Already on" without running the Settings read on every frame.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::permission::request::{code, limits as lim, PermissionRequest};
use serde_json::{json, Value};

use super::actions::Check;
use super::{inject, present, store};

pub(crate) use lim::CHECK_CACHE_SECS;

/// A refused ask: HTTP status, code, hint and extra top-level fields.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Limited {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
    pub extra: Value,
}

fn limited(status: u16, code: &'static str, hint: impl Into<String>) -> Limited {
    Limited { status, code, hint: hint.into(), extra: json!({}) }
}

/// The checks after dedupe (§3.2 table, in order): the 24 h block after a
/// denial, the ask limit, then the pending and hourly caps.
pub(crate) fn check_ask(workspace_id: &str, action: &str, params_hash: &str, now: i64) -> Result<(), Limited> {
    let db_err = |e: String| limited(500, "db", e);
    if let Some(d) = store::last_denial_since(workspace_id, action, params_hash, now - lim::DENIAL_BLOCK_SECS).map_err(db_err)? {
        let at = d.answered_at.unwrap_or(now);
        let retry = at + lim::DENIAL_BLOCK_SECS;
        let by = d.answered_by.clone().unwrap_or_default();
        let mut l = limited(
            409,
            code::RECENTLY_DENIED,
            format!(
                "{by} denied the same request at {}; asking again is blocked until {}. If something changed, run k2 permission update {} --why \"…\" once.",
                inject::when(at, now),
                inject::when(retry, now),
                d.display_id()
            ),
        );
        l.extra = json!({ "denied": { "by": by, "at": at, "reason": d.reason, "askAgainAt": retry, "id": d.display_id() } });
        return Err(l);
    }
    let same = store::count_same_asks_since(workspace_id, action, params_hash, now - lim::ASK_LIMIT_WINDOW_SECS).map_err(db_err)?;
    if same as i64 >= lim::ASKS_PER_WINDOW {
        return Err(limited(
            409,
            code::ASK_LIMIT,
            format!("this was asked {} times in the last 7 days; ask your human the usual way instead", lim::ASKS_PER_WINDOW),
        ));
    }
    if store::count_open_for_workspace(workspace_id).map_err(db_err)? >= lim::PENDING_PER_WORKSPACE {
        return Err(limited(
            429,
            code::TOO_MANY_REQUESTS,
            format!("{} requests are already waiting for this agent; wait for an answer or cancel one", lim::PENDING_PER_WORKSPACE),
        ));
    }
    if store::count_asks_since(workspace_id, now - 3600).map_err(db_err)? >= lim::ASKS_PER_WORKSPACE_PER_HOUR {
        return Err(limited(
            429,
            code::TOO_MANY_REQUESTS,
            format!("at most {} asks per hour per agent; try again later", lim::ASKS_PER_WORKSPACE_PER_HOUR),
        ));
    }
    if store::count_open_on_server().map_err(db_err)? >= lim::PENDING_PER_SERVER {
        return Err(limited(
            429,
            code::TOO_MANY_REQUESTS,
            format!("{} requests are already waiting on this server; try again later", lim::PENDING_PER_SERVER),
        ));
    }
    Ok(())
}

/// The ask number of a new request for the same values ("ask 2 of 2").
pub(crate) fn ask_number(workspace_id: &str, action: &str, params_hash: &str, now: i64) -> i64 {
    store::count_same_asks_since(workspace_id, action, params_hash, now - lim::ASK_LIMIT_WINDOW_SECS).unwrap_or(0) as i64 + 1
}

/// `check` for a request, cached [`CHECK_CACHE_SECS`] per request id
/// (render time, §4.2). The click never uses the cache (§6.2).
pub(crate) fn cached_check(req: &PermissionRequest) -> Check {
    static CACHE: Mutex<Option<HashMap<String, (Instant, Check)>>> = Mutex::new(None);
    let ttl = Duration::from_secs(CHECK_CACHE_SECS);
    if let Ok(g) = CACHE.lock() {
        if let Some((at, c)) = g.as_ref().and_then(|m| m.get(req.id.as_str())) {
            if at.elapsed() < ttl {
                return c.clone();
            }
        }
    }
    let c = match present::find_action(&req.action) {
        Some(r) => (r.check)(&present::ctx_of(req)),
        None => Check::Moved("K2 no longer knows this action".to_string()),
    };
    if let Ok(mut g) = CACHE.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        m.retain(|_, (at, _)| at.elapsed() < ttl);
        m.insert(req.id.as_str().to_string(), (Instant::now(), c.clone()));
    }
    c
}
