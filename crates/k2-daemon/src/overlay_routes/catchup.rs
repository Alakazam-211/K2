//! `POST /cli/thread/catchup` (inbox-catchup-v1, K2 0.46.3): Thread deltas
//! (or newest pages) for up to 200 chats, plus up to 50 project chat
//! deltas, in one round trip. Contract:
//! `.k2/prds/contract-phone-inbox-stream-0462.md` §3 (catch-up only) and
//! `/Users/Shared/k2-companion/thread-catchup-contract-0463.md`.
//!
//! - Each `threads[]` entry is built by [`super::thread_read_for`], the
//!   same reader `GET /cli/thread` uses, so for the same caller it is the
//!   plain read's body minus `ok` (`afterSeq` = `?after_seq=N&limit=L`,
//!   `newest:true` = `?limit=L`). App guests get their rooms only and
//!   hidden attachments exactly as on a plain read; never a
//!   `serde_json::to_value(items)` shortcut (the 0.46.1 delta leak).
//! - A failing address is an error on that entry, never the whole call.
//! - Byte budget ([`BUDGET_BYTES`], C1): entries fill item by item. When
//!   the budget runs out inside an entry, that entry is cut (fewer items,
//!   `has_more: true`) and every later entry is `deferred`. The first
//!   entry always serves at least one item, so every call makes progress.
//! - One catch-up in flight per token (C2): a second concurrent call is
//!   429 `{error:"rate_limited", retryAfterMs}`. Entries take and release
//!   the DB connection one at a time; nothing is held across the call.
//! - Top-level `rev` is read BEFORE the first store read; `rev` and
//!   `instanceId` are left off App guest answers (C4).
//! - catchup-unchanged-v1 (0.46.3): optional `sinceRev` + `instanceId`
//!   (both from the last answer, both or neither). When they match this
//!   process and nothing was written to a Thread since, the answer is
//!   `{ok, rev, instanceId, unchanged: true}` with NO store reads. Threads
//!   only: project chat posts don't move `rev`, so a caller that needs
//!   `groups` still makes a normal call for them. A request without
//!   `sinceRev` is answered exactly as before.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::sidecar_routes::Caller;

use super::{request_skin, thread_read_for, usage, ThreadReadMode};

pub const ROUTE: &str = "/cli/thread/catchup";
/// Most `threads[]` entries one call may name.
pub const MAX_THREADS: usize = 200;
/// Most `groups[]` entries one call may name.
pub const MAX_GROUPS: usize = 50;
/// Per-conversation item limit when `limit` is absent.
pub const LIMIT_DEFAULT: usize = 50;
/// Largest `limit` a call may ask for.
pub const LIMIT_MAX: usize = 500;
/// The response's byte budget (4 MiB).
pub const BUDGET_BYTES: usize = 4 * 1024 * 1024;
/// What a 429 tells the caller to wait.
pub const RETRY_AFTER_MS: u64 = 1000;
/// Room kept for the envelope (`ok`, `rev`, `instanceId`, `has_more`, the
/// two array brackets).
const ENVELOPE_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    After(i64),
    Newest,
}

#[derive(Debug)]
struct ThreadReq {
    addr: String,
    want: Want,
}

#[derive(Debug)]
struct GroupReq {
    group: String,
    after_seq: i64,
}

#[derive(Debug)]
struct Request {
    threads: Vec<ThreadReq>,
    groups: Vec<GroupReq>,
    limit: usize,
    /// catchup-unchanged-v1: the caller's last `(rev, instanceId)`. When it
    /// still matches, the answer is `unchanged` with no store reads.
    since: Option<(u64, String)>,
    /// Permission cards (prd-agent-settings-cards-v1 rev 3.1 §17.1): top
    /// level `"cards": 2` applies to every `threads[]` entry. Honoured for
    /// people only; anything that is not a whole number is ignored, as an
    /// unknown field was before.
    cards: u8,
}

/// A whole number `>= 0` (a JSON integer; `4.0`, `"4"` and `-1` are not).
fn whole(v: &Value) -> Option<i64> {
    v.as_i64().filter(|n| *n >= 0)
}

fn parse(body: &[u8]) -> Result<Request, CliResponse> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| usage(format!("the body must be a JSON object: {e}")))?;
    let Some(obj) = v.as_object() else {
        return Err(usage("the body must be a JSON object"));
    };

    let limit = match obj.get("limit") {
        None | Some(Value::Null) => LIMIT_DEFAULT,
        Some(raw) => match whole(raw) {
            Some(n) if n >= 1 && (n as usize) <= LIMIT_MAX => n as usize,
            _ => return Err(usage(format!("invalid limit {raw}: expected a whole number from 1 to {LIMIT_MAX}"))),
        },
    };

    let mut threads: Vec<ThreadReq> = Vec::new();
    let raw_threads = match obj.get("threads") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err(usage("threads must be an array of {addr, afterSeq} or {addr, newest:true}")),
    };
    if raw_threads.len() > MAX_THREADS {
        return Err(usage(format!("at most {MAX_THREADS} threads per call, got {}", raw_threads.len())));
    }
    for (i, t) in raw_threads.iter().enumerate() {
        let Some(t) = t.as_object() else {
            return Err(usage(format!("threads[{i}] must be an object")));
        };
        let addr = match t.get("addr").and_then(Value::as_str) {
            Some(a) if !a.trim().is_empty() => a.to_string(),
            _ => return Err(usage(format!("threads[{i}].addr is missing"))),
        };
        let after = match t.get("afterSeq") {
            None | Some(Value::Null) => None,
            Some(raw) => match whole(raw) {
                Some(n) => Some(n),
                None => {
                    return Err(usage(format!(
                        "invalid threads[{i}].afterSeq {raw}: expected a whole number >= 0"
                    )))
                }
            },
        };
        let newest = match t.get("newest") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => false,
            Some(Value::Bool(true)) => true,
            Some(raw) => return Err(usage(format!("invalid threads[{i}].newest {raw}: expected true"))),
        };
        let want = match (after, newest) {
            (Some(n), false) => Want::After(n),
            (None, true) => Want::Newest,
            _ => {
                return Err(usage(format!(
                    "threads[{i}] ({addr}) needs exactly one of afterSeq and newest:true"
                )))
            }
        };
        // A duplicate keeps the first.
        if !threads.iter().any(|x| x.addr == addr) {
            threads.push(ThreadReq { addr, want });
        }
    }

    let mut groups: Vec<GroupReq> = Vec::new();
    let raw_groups = match obj.get("groups") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err(usage("groups must be an array of {group, afterSeq}")),
    };
    if raw_groups.len() > MAX_GROUPS {
        return Err(usage(format!("at most {MAX_GROUPS} groups per call, got {}", raw_groups.len())));
    }
    for (i, g) in raw_groups.iter().enumerate() {
        let Some(g) = g.as_object() else {
            return Err(usage(format!("groups[{i}] must be an object")));
        };
        let group = match g.get("group").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => return Err(usage(format!("groups[{i}].group is missing"))),
        };
        let after_seq = match g.get("afterSeq").map(whole) {
            Some(Some(n)) => n,
            _ => {
                return Err(usage(format!(
                    "groups[{i}] ({group}) needs afterSeq, a whole number >= 0"
                )))
            }
        };
        if !groups.iter().any(|x| x.group == group) {
            groups.push(GroupReq { group, after_seq });
        }
    }
    // catchup-unchanged-v1: both or neither.
    let since_rev = match obj.get("sinceRev") {
        None | Some(Value::Null) => None,
        Some(raw) => match raw.as_u64() {
            Some(n) => Some(n),
            None => return Err(usage(format!("invalid sinceRev {raw}: expected a whole number >= 0"))),
        },
    };
    let instance = match obj.get("instanceId") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(raw) => return Err(usage(format!("invalid instanceId {raw}: expected the instanceId a catch-up returned"))),
    };
    let since = match (since_rev, instance) {
        (Some(r), Some(i)) => Some((r, i)),
        (None, None) => None,
        _ => return Err(usage("sinceRev and instanceId go together (both from the last catch-up's answer)")),
    };
    let cards = crate::permission::view::cards_body(obj.get("cards"));
    Ok(Request { threads, groups, limit, since, cards })
}

// ── one in flight per token (C2) ─────────────────────────────────────

fn in_flight() -> &'static Mutex<HashSet<u64>> {
    static SET: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

fn flight_hash(key: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

/// Held for the length of one catch-up; dropping it frees the token.
pub(crate) struct FlightGuard(u64);

impl Drop for FlightGuard {
    fn drop(&mut self) {
        in_flight().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.0);
    }
}

/// `None` when a catch-up for `key` (the caller's token) is running.
pub(crate) fn try_enter(key: &str) -> Option<FlightGuard> {
    let h = flight_hash(key);
    let entered = in_flight().lock().unwrap_or_else(|p| p.into_inner()).insert(h);
    // Lazily: a guard built for a refused call would free the running
    // call's slot when it drops.
    entered.then(|| FlightGuard(h))
}

pub(crate) fn rate_limited() -> CliResponse {
    CliResponse {
        status: "429 Too Many Requests",
        content_type: "application/json",
        body: json!({ "error": "rate_limited", "retryAfterMs": RETRY_AFTER_MS }).to_string(),
    }
}

// ── entries ──────────────────────────────────────────────────────────

/// `{<key>: name, status, error:{code, hint}}` from a plain read's error
/// response (its `error` object as-is; the older string shapes become
/// `{code, hint}` the way `thread/latest` maps them).
fn error_entry(key: &str, name: &str, r: &CliResponse) -> Value {
    let status: u16 = r.status.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(500);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let error = match &v["error"] {
        Value::Object(_) => v["error"].clone(),
        Value::String(s) => {
            let code = if v["message"].is_string() {
                s.clone()
            } else if s.starts_with("missing capability") {
                "cap_not_granted".to_string()
            } else {
                "error".to_string()
            };
            json!({ "code": code, "hint": v["message"].as_str().unwrap_or(s) })
        }
        _ => json!({ "code": "error", "hint": r.body }),
    };
    let mut out = serde_json::Map::new();
    out.insert(key.to_string(), json!(name));
    out.insert("status".to_string(), json!(status));
    out.insert("error".to_string(), error);
    Value::Object(out)
}

fn without_ok(mut v: Value) -> Value {
    if let Value::Object(m) = &mut v {
        m.remove("ok");
    }
    v
}

fn thread_entry(
    t: &ThreadReq,
    limit: usize,
    principal: Option<&crate::session_token::HookPrincipal>,
    gb_who: Option<&Who>,
    seen: &crate::permission::view::Seen,
) -> Value {
    // `gb:<name>`: a person's read goes through the Grok Bots gate, exactly
    // as the TCP `GET /cli/thread` does. Everyone else (an agent's cell, an
    // App guest) falls through to the overlay reader, which refuses it.
    if let (Some(who), true) = (gb_who, crate::grokbots_routes::is_gb_addr(&t.addr)) {
        let mut params = HashMap::new();
        params.insert("addr".to_string(), t.addr.clone());
        params.insert("limit".to_string(), limit.to_string());
        if let Want::After(n) = t.want {
            params.insert("after_seq".to_string(), n.to_string());
        }
        let r = crate::grokbots_routes::thread_by_addr(who.clone(), &params);
        if r.status.starts_with("200") {
            return match serde_json::from_str::<Value>(&r.body) {
                Ok(v) => without_ok(v),
                Err(e) => error_entry("addr", &t.addr, &CliResponse::internal_error(format!("gb read: {e}"))),
            };
        }
        return error_entry("addr", &t.addr, &r);
    }
    let mode = match t.want {
        Want::After(n) => ThreadReadMode::After(n),
        Want::Newest => ThreadReadMode::Newest,
    };
    match thread_read_for(&t.addr, mode, limit, principal, seen) {
        Ok(v) => without_ok(v),
        Err(r) => error_entry("addr", &t.addr, &r),
    }
}

/// One `groups[]` entry: the project chat read's own member rules
/// (`chat_read_refusal`: an agent reads only its own projects' chats), then
/// the same body builder as `GET /cli/project-group/messages?after_seq=`.
/// `caller` is the classified caller; `None` (an in-process call) is the
/// owner when no passport is stamped, and refused when one is (fail closed).
fn group_entry(g: &GroupReq, limit: usize, guest: bool, caller: Option<&Caller>, passport: bool) -> Value {
    if guest {
        return error_entry(
            "group",
            &g.group,
            &super::forbidden("project chats are for people signed in to this server and their agents, not App guests"),
        );
    }
    let mut params = HashMap::new();
    params.insert("group".to_string(), g.group.clone());
    let refusal = match caller {
        Some(c) => crate::project_group_routes::chat_read_refusal(
            crate::project_group_routes::MESSAGES_PATH,
            &params,
            c,
        ),
        None if passport => Some(super::forbidden("project chats need a classified caller here")),
        None => None,
    };
    if let Some(r) = refusal {
        return error_entry("group", &g.group, &r);
    }
    let body = crate::project_group_routes::resolve_group_selector(&g.group)
        .and_then(|id| crate::project_group_routes::messages_after_seq_body(&id, g.after_seq, Some(limit as i64)));
    match body {
        Ok(v) => {
            let mut v = without_ok(v);
            v["group"] = json!(g.group);
            v
        }
        Err(r) => error_entry("group", &g.group, &r),
    }
}

// ── the byte budget (C1) ─────────────────────────────────────────────

struct Budget {
    left: usize,
    /// Something was served already (the first entry is always served).
    served: bool,
    /// The budget ran out: every later entry is deferred.
    cut: bool,
}

/// Which list an entry's budget cut trims, and from which end.
#[derive(Clone, Copy)]
enum Cut {
    /// A delta (`items`, oldest first): keep the oldest, `has_more` =
    /// newer items remain.
    DeltaItems,
    /// A newest page (`items`, oldest first within the page): keep the
    /// newest, `has_more` = older items exist.
    PageItems,
    /// A project chat delta (`messages`, oldest first).
    GroupMessages,
}

impl Budget {
    fn new(total: usize) -> Self {
        Budget { left: total.saturating_sub(ENVELOPE_BYTES), served: false, cut: false }
    }

    /// Serve `entry` whole, cut it to what fits, or `None` (defer it).
    fn fit(&mut self, entry: Value, cut: Cut) -> Option<Value> {
        if self.cut {
            return None;
        }
        let whole = entry.to_string().len() + 1;
        if whole <= self.left {
            self.left -= whole;
            self.served = true;
            return Some(entry);
        }
        // Over the budget: keep as many items as fit (at least one for the
        // first entry), then stop.
        self.cut = true;
        let key = match cut {
            Cut::GroupMessages => "messages",
            Cut::DeltaItems | Cut::PageItems => "items",
        };
        let list = match entry.get(key).and_then(Value::as_array) {
            Some(l) if !l.is_empty() => l.clone(),
            // An error or an empty list that is still too big: only the
            // first entry is served regardless.
            _ => {
                if self.served {
                    return None;
                }
                self.served = true;
                self.left = 0;
                return Some(entry);
            }
        };
        let mut base = entry.clone();
        base[key] = json!([]);
        base["has_more"] = json!(true);
        if matches!(cut, Cut::GroupMessages) {
            base["truncated"] = json!(true);
        }
        let room = self.left.saturating_sub(base.to_string().len() + 1);
        let sizes: Vec<usize> = list.iter().map(|i| i.to_string().len() + 1).collect();
        let order: Vec<usize> = match cut {
            Cut::PageItems => (0..list.len()).rev().collect(),
            Cut::DeltaItems | Cut::GroupMessages => (0..list.len()).collect(),
        };
        let mut used = 0usize;
        let mut keep = 0usize;
        for &i in &order {
            if used + sizes[i] > room {
                break;
            }
            used += sizes[i];
            keep += 1;
        }
        if keep == 0 {
            if self.served {
                return None;
            }
            keep = 1;
        }
        let kept: Vec<Value> = match cut {
            Cut::PageItems => list[list.len() - keep..].to_vec(),
            Cut::DeltaItems | Cut::GroupMessages => list[..keep].to_vec(),
        };
        base[key] = Value::Array(kept);
        self.left = 0;
        self.served = true;
        Some(base)
    }
}

// ── the route ────────────────────────────────────────────────────────

/// `POST /cli/thread/catchup` for the caller bound on this thread (an App
/// guest's pass via `with_request_skin`; an agent passport stamped in
/// `params`). `gb_who`: the TCP caller, for `gb:` entries (the cell socket
/// passes `None`). `flight_key`: the caller's token, for the one-in-flight
/// rule (`None` skips it). `caller`: the classified caller, for the project
/// chat member rules on `groups[]`.
pub(crate) fn handle(
    params: &HashMap<String, String>,
    body: &[u8],
    gb_who: Option<&Who>,
    flight_key: Option<&str>,
    caller: Option<&Caller>,
) -> CliResponse {
    handle_with_budget(params, body, gb_who, flight_key, caller, BUDGET_BYTES)
}

pub(crate) fn handle_with_budget(
    params: &HashMap<String, String>,
    body: &[u8],
    gb_who: Option<&Who>,
    flight_key: Option<&str>,
    caller: Option<&Caller>,
    budget_bytes: usize,
) -> CliResponse {
    let req = match parse(body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let _flight = match flight_key {
        Some(k) => match try_enter(k) {
            Some(g) => Some(g),
            None => return rate_limited(),
        },
        None => None,
    };
    let guest = request_skin().is_some();
    let principal = crate::caller_workspace::principal_from_params(params);
    // Read BEFORE the first store read (§3): any frame at or below it is
    // already in the answer.
    let rev = crate::overlay_ws::current_rev();
    // catchup-unchanged-v1: the caller's last answer is still current (same
    // daemon process, no Thread write since). No store reads. THREADS ONLY:
    // project chat posts don't move `rev`, so `groups` must still be
    // checked by a normal call. App guests never get `rev` (C4), so this
    // never applies to them.
    if !guest {
        if let Some((since_rev, since_instance)) = &req.since {
            let instance = crate::boot_status::instance_id();
            if *since_rev == rev && since_instance.as_str() == instance {
                return CliResponse::ok_json(
                    json!({ "ok": true, "rev": rev, "instanceId": instance, "unchanged": true }).to_string(),
                );
            }
        }
    }
    let mut budget = Budget::new(budget_bytes);
    // Permission cards: every entry is projected for this caller (§4.2).
    let seen = super::request_seen(params, caller, req.cards);

    let mut threads = Vec::with_capacity(req.threads.len());
    for t in &req.threads {
        if budget.cut {
            threads.push(json!({ "addr": t.addr, "deferred": true }));
            continue;
        }
        let entry = thread_entry(t, req.limit, principal.as_ref(), gb_who, &seen);
        let cut = match t.want {
            Want::After(_) => Cut::DeltaItems,
            Want::Newest => Cut::PageItems,
        };
        threads.push(budget.fit(entry, cut).unwrap_or_else(|| json!({ "addr": t.addr, "deferred": true })));
    }
    let mut groups = Vec::with_capacity(req.groups.len());
    for g in &req.groups {
        if budget.cut {
            groups.push(json!({ "group": g.group, "deferred": true }));
            continue;
        }
        let entry = group_entry(g, req.limit, guest, caller, principal.is_some());
        groups.push(
            budget
                .fit(entry, Cut::GroupMessages)
                .unwrap_or_else(|| json!({ "group": g.group, "deferred": true })),
        );
    }

    let mut out = json!({
        "ok": true,
        "has_more": budget.cut && (threads.iter().chain(groups.iter()).any(|e| e["deferred"] == true)),
        "threads": threads,
        "groups": groups,
    });
    if !guest {
        out["rev"] = json!(rev);
        out["instanceId"] = json!(crate::boot_status::instance_id());
    }
    CliResponse::ok_json(out.to_string())
}

#[cfg(test)]
mod tests;
