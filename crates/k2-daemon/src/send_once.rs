//! Idempotent sends (0.46.15): `k2 msg`, `k2 msg --inbox-wake|--inbox-silent`
//! (local and `agent::host`), `k2 thread ask` and the federated live send
//! carry one random `idempotency_key` per CLI invocation. A send that timed
//! out on the caller's side may still have landed; the CLI then asks again
//! with the SAME key, and this ledger answers the first answer instead of
//! delivering twice. (`k2 thread "text"` uses the Thread's own durable
//! `clientMsgId`, post-idempotency-v1; it is not kept here.)
//!
//! - The key is (route, sender, key). The sender comes from the credential,
//!   never the body: an agent passport's workspace (`a:<uuid>`, the same on
//!   the cell socket and on TCP, so the CLI's socket-then-TCP fallback finds
//!   the first try), else the person or token kind the dispatcher stamped.
//!   One sender's key never answers another's.
//! - In memory, for [`TTL`] (10 minutes) after the first answer. The routes
//!   it guards keep no per-send record of their own to replay from, and a
//!   retry comes within seconds; a daemon restart forgets the keys (a send
//!   that crossed the restart may then run again, as before this ledger).
//! - A 2xx answer is kept, "not delivered" bodies included (`k2 msg`'s
//!   `success: false`): a repeat answers what the first try did (with
//!   `"replayed": true`); a fresh send is a fresh invocation with a new key.
//!   Any other status (a 400, a refusal, a 500) ran nothing to repeat, so it
//!   frees the key: the same key may try again (the CLI's tray send retries
//!   a path the daemon couldn't read with a staged copy, same key).
//! - A repeat while the first try still runs waits for it up to
//!   [`IN_FLIGHT_WAIT`] (below the CLI's 30 s request limit), then answers
//!   409 `in_progress`; the CLI asks again with the same key.
//! - A key is 8-64 characters of `[A-Za-z0-9_-]` (a UUID fits), else 400.

use std::collections::HashMap;
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::cli_response::CliResponse;

/// The form / query / JSON body field.
pub const PARAM: &str = "idempotency_key";

/// How long a finished send's answer is kept.
pub const TTL: Duration = Duration::from_secs(600);

/// How long a repeat waits for a first try that is still running.
pub const IN_FLIGHT_WAIT: Duration = Duration::from_secs(20);

/// The most answers kept; past it the oldest finished ones go first.
const MAX_KEPT: usize = 4096;

const MIN_LEN: usize = 8;
const MAX_LEN: usize = 64;

/// True for 8-64 characters of `[A-Za-z0-9_-]`.
pub fn is_valid(key: &str) -> bool {
    (MIN_LEN..=MAX_LEN).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn invalid() -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "success": false,
            "error": {
                "code": "usage",
                "hint": "idempotency_key must be 8-64 characters of A-Z, a-z, 0-9, '-' or '_' (a UUID works)",
            }
        })
        .to_string(),
    }
}

fn in_progress(key: &str) -> CliResponse {
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "success": false,
            "error": {
                "code": "in_progress",
                "hint": "the first try with this idempotency key is still running; ask again with the same key",
            },
            "idempotencyKey": key,
        })
        .to_string(),
    }
}

/// The sender of a `/cli/*` request whose params the dispatcher stamped:
/// the passport's workspace, else the TCP caller kind (`owner`,
/// `login:<name>`), else `fallback` (the route's own notion of who called).
pub fn sender_of(params: &HashMap<String, String>, fallback: &str) -> String {
    if let Some(p) = crate::caller_workspace::principal_from_params(params) {
        return crate::client_msg_id::agent_ns(&p.workspace_uuid);
    }
    if let Some(kind) = params
        .get(crate::workspace_sessions_routes::CALLER_KIND_KEY)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return format!("c:{kind}");
    }
    format!("c:{fallback}")
}

/// Run `send` once per (`route`, sender, `params[idempotency_key]`). No key:
/// `send` runs as before.
pub fn once(route: &str, params: &HashMap<String, String>, fallback_sender: &str, send: impl FnOnce() -> CliResponse) -> CliResponse {
    let key = params.get(PARAM).map(|s| s.trim()).filter(|s| !s.is_empty());
    once_keyed(route, &sender_of(params, fallback_sender), key, send)
}

/// [`once`] for a route whose key and sender the caller read itself (a JSON
/// body).
pub fn once_keyed(route: &str, sender: &str, key: Option<&str>, send: impl FnOnce() -> CliResponse) -> CliResponse {
    match key {
        None => send(),
        Some(k) if !is_valid(k) => invalid(),
        Some(k) => LEDGER.run(Id { route: route.to_string(), sender: sender.to_string(), key: k.to_string() }, send),
    }
}

/// The key a JSON body carries, if any (`idempotency_key`).
pub fn key_in_json(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get(PARAM)?.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A stable federation `msg_uuid` for a send with this key, so a send
/// repeated after this ledger forgot it (a restart, past [`TTL`]) still
/// reaches the peer with the uuid its durable seen-store dedupes on. `kind`
/// keeps a live message and a tray package with one key apart.
pub fn federation_msg_uuid(sender: &str, kind: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for part in ["k2-send-once-v1", sender, kind, key] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    // RFC 4122 layout: version 5 (name-based), variant 10.
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Id {
    route: String,
    sender: String,
    key: String,
}

#[derive(Clone)]
struct Answer {
    status: &'static str,
    content_type: &'static str,
    body: String,
}

enum Slot {
    Running,
    Done(Answer, Instant),
}

pub(crate) struct Ledger {
    slots: Mutex<HashMap<Id, Slot>>,
    done: Condvar,
    ttl: Duration,
    wait: Duration,
}

static LEDGER: LazyLock<Ledger> = LazyLock::new(|| Ledger::new(TTL, IN_FLIGHT_WAIT));

/// `"replayed": true` as the last key of a JSON object answer (every other
/// byte as it was); any other body unchanged.
fn mark_replayed(body: &str) -> String {
    let trimmed = body.trim_end();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return body.to_string();
    }
    let head = &trimmed[..trimmed.len() - 1];
    let empty = head.trim_start().strip_prefix('{').is_some_and(|rest| rest.trim().is_empty());
    format!("{head}{}\"replayed\":true}}", if empty { "" } else { "," })
}

impl Ledger {
    pub(crate) fn new(ttl: Duration, wait: Duration) -> Self {
        Self { slots: Mutex::new(HashMap::new()), done: Condvar::new(), ttl, wait }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<Id, Slot>> {
        // A panicking send is cleaned up by its guard; the map stays usable.
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sweep(&self, slots: &mut HashMap<Id, Slot>, now: Instant) {
        slots.retain(|_, s| match s {
            Slot::Running => true,
            Slot::Done(_, at) => now.duration_since(*at) < self.ttl,
        });
        if slots.len() > MAX_KEPT {
            let mut done: Vec<(Instant, Id)> = slots
                .iter()
                .filter_map(|(id, s)| match s {
                    Slot::Done(_, at) => Some((*at, id.clone())),
                    Slot::Running => None,
                })
                .collect();
            done.sort_by_key(|(at, _)| *at);
            let extra = slots.len() - MAX_KEPT;
            for (_, id) in done.into_iter().take(extra) {
                slots.remove(&id);
            }
        }
    }

    fn run(&self, id: Id, send: impl FnOnce() -> CliResponse) -> CliResponse {
        let deadline = Instant::now() + self.wait;
        {
            let mut slots = self.lock();
            loop {
                let now = Instant::now();
                self.sweep(&mut slots, now);
                match slots.get(&id) {
                    None => {
                        slots.insert(id.clone(), Slot::Running);
                        break;
                    }
                    Some(Slot::Done(a, _)) => {
                        return CliResponse { status: a.status, content_type: a.content_type, body: mark_replayed(&a.body) };
                    }
                    Some(Slot::Running) => {
                        if now >= deadline {
                            return in_progress(&id.key);
                        }
                        slots = self.done.wait_timeout(slots, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
                    }
                }
            }
        }
        // A send that panics leaves no Running slot behind (a waiter would
        // otherwise answer `in_progress` for the whole TTL).
        struct Release<'a> {
            ledger: &'a Ledger,
            id: Option<Id>,
        }
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                if let Some(id) = self.id.take() {
                    self.ledger.lock().remove(&id);
                    self.ledger.done.notify_all();
                }
            }
        }
        let mut release = Release { ledger: self, id: Some(id.clone()) };
        let resp = send();
        release.id = None;
        {
            let mut slots = self.lock();
            if resp.status.starts_with('2') {
                let answer = Answer { status: resp.status, content_type: resp.content_type, body: resp.body.clone() };
                slots.insert(id, Slot::Done(answer, Instant::now()));
            } else {
                slots.remove(&id);
            }
        }
        self.done.notify_all();
        resp
    }
}

#[cfg(test)]
#[path = "send_once_tests.rs"]
mod tests;
