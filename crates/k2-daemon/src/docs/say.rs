//! `POST /cli/docs/say`: "Say something" in a live doc's room (Design's
//! live scene and timeline tabs: a short message to the person or agent
//! you follow, with a chip naming where you look, and their reply).
//!
//! **Who.** Anyone who may read the doc, as on the docs socket: a person
//! (the owner, a Connect login: Member or above by the route's policy row,
//! or a shell tab) or an agent with the same access (its own workspace, or a
//! share). App passes are refused, as on every docs route. POST only (405).
//!
//! **Body** (JSON; empty strings count as absent):
//!
//! | Field | |
//! |---|---|
//! | `path` (+ `workspace`) | the doc, as every docs route |
//! | `text` | the message: 1 to [`TEXT_MAX`] bytes of UTF-8 after trimming, no control characters but line breaks and tabs |
//! | `to` | optional: who it is for (a pid, `person:<login>`, `agent:<id>`, a login or a workspace name), someone who may read the doc. None: the room |
//! | `at` | optional: what the sender looks at, by address (the chip: `@cam:c1`, `@trk:1/clip:7`), at most [`AT_MAX`] bytes, no spaces or control characters. Not resolved |
//! | `time` | optional: a playhead, a decimal integer in the doc's time unit (a timeline's ticks), as a `time_position`'s `at` |
//! | `reply_to` | optional: the `id` of the message this answers |
//!
//! **What happens.** The message goes to every socket on the doc whose
//! person may read it, as one frame (WIRE extra, beside `turn_end`):
//!
//! ```text
//! {"type":"say","id":"say_…","from":{id,name,kind,colour},"to":{…}|null,
//!  "text":"…","at":"@cam:c1"|null,"time":"48000"|null,"reply_to":"say_…"|null,"at_ms":…}
//! ```
//!
//! and, when it is **for an agent**, an inbox note in that agent's workspace
//! (agents don't hold the socket): the message quoted, with the line that
//! answers it (`k2 doc say <doc> --to <sender> --reply-to <id> "…"`).
//! Nothing is stored: a socket that joins later doesn't see it. The answer:
//! `{ok, say: {id, at_ms, to}, sockets, inbox}`.
//!
//! **Bounded.** [`TEXT_MAX`] bytes a message; [`BURST`] messages at once and
//! one every [`REFILL`] after, per sender (429 `rate_limited` with
//! `retryAfterMs`). Never logged: the audit line names the doc, the sender
//! and the recipient only.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::json;

use super::caller::refusal;
use super::host::{self, Principal};
use super::host_events::{self, HostEvent};
use super::routes::{self, Call};
use super::target::Tier;
use crate::cli_response::CliResponse;

pub const SAY: &str = "/cli/docs/say";

/// Most bytes in a message (after trimming): a claim's note, the kit's
/// own cap for a short word to the room.
pub const TEXT_MAX: usize = 1024;

/// Most bytes in `at` (the kit's `PRESENCE_ADDRESS_MAX`).
pub const AT_MAX: usize = k2_mpc_base::session::PRESENCE_ADDRESS_MAX;

/// Most bytes in a `reply_to` id.
pub const ID_MAX: usize = 64;

/// Messages a sender may send at once...
pub const BURST: u32 = 5;
/// ...then one more each this long.
pub const REFILL: Duration = Duration::from_secs(3);

/// Senders remembered at most (the least recently heard go first).
const MAX_SENDERS: usize = 4096;

/// The audit event (who to whom on which doc; never the text).
pub const AUDIT_SAY: &str = "doc-say";

/// The message, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub to: Option<String>,
    pub at: Option<String>,
    pub time: Option<String>,
    pub reply_to: Option<String>,
}

fn opt(c: &Call<'_>, k: &str) -> Option<String> {
    c.input.str(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// `text`, `at`, `time` and `reply_to` as the body gives them, checked;
/// `to` as given (resolved by the route). The refusal's words otherwise.
pub fn read(text: Option<String>, to: Option<String>, at: Option<String>, time: Option<String>, reply_to: Option<String>) -> Result<Message, String> {
    let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).ok_or("text is required: what you want to say")?;
    if text.len() > TEXT_MAX {
        return Err(format!("text is {} bytes; a message is at most {TEXT_MAX}", text.len()));
    }
    if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err("text has a control character; only line breaks and tabs are kept".into());
    }
    if let Some(a) = &at {
        if a.len() > AT_MAX {
            return Err(format!("at is {} bytes; an address is at most {AT_MAX}", a.len()));
        }
        if a.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("at is an address (like @cam:c1 or @trk:1/clip:7): no spaces".into());
        }
    }
    if let Some(t) = &time {
        k2_mpc_base::session::TimePosition::new(t, k2_mpc_base::session::PlayState::Paused)
            .map_err(|_| format!("time {t:?} is a decimal integer in the doc's time unit (an optional -, digits, no leading zero)"))?;
    }
    if let Some(r) = &reply_to {
        if r.len() > ID_MAX || !r.starts_with("say_") || !r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(format!("reply_to {r:?} isn't a message id (say_…)"));
        }
    }
    Ok(Message { text, to, at, time, reply_to })
}

/// A message id: `say_` and 16 hex digits.
pub fn new_id() -> String {
    format!("say_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

// ── The rate ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Per sender (pid): [`BURST`] at once, then one each [`REFILL`]. `Err` is
/// how long until the next one may go.
#[derive(Default)]
pub struct Rate(HashMap<String, Bucket>);

impl Rate {
    pub fn take(&mut self, who: &str, now: Instant) -> Result<(), Duration> {
        let per_sec = 1.0 / REFILL.as_secs_f64();
        if !self.0.contains_key(who) && self.0.len() >= MAX_SENDERS {
            if let Some(oldest) = self.0.iter().min_by_key(|(_, b)| b.at).map(|(k, _)| k.clone()) {
                self.0.remove(&oldest);
            }
        }
        let b = self.0.entry(who.to_string()).or_insert(Bucket { tokens: BURST as f64, at: now });
        let elapsed = now.saturating_duration_since(b.at).as_secs_f64();
        b.tokens = (b.tokens + elapsed * per_sec).min(BURST as f64);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - b.tokens) / per_sec))
        }
    }
}

fn rate() -> &'static Mutex<Rate> {
    static R: OnceLock<Mutex<Rate>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Rate::default()))
}

fn rate_limited(wait: Duration) -> CliResponse {
    let ms = wait.as_millis().max(1) as u64;
    CliResponse {
        status: "429 Too Many Requests",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": { "code": "rate_limited", "hint": format!("too many messages; say it again in {}s", ms.div_ceil(1000)) },
            "retryAfterMs": ms,
        })
        .to_string(),
    }
}

// ── The route ────────────────────────────────────────────────────────

/// The route, after the docs family resolved the caller, the target, its
/// core and the caller's rights.
pub(crate) fn handle(rt: &super::Runtime, c: &Call<'_>) -> CliResponse {
    let t = c.target;
    if !c.rights.read {
        return routes::with_exit(refusal("403 Forbidden", "doc_forbidden", "you can't read this doc"), k2_mpc_base::tools::exit::REFUSED);
    }
    if t.tier == Tier::Blob {
        return routes::with_exit(
            refusal("409 Conflict", "no_live_room", format!("{} is an any-file doc: it has no live room to say something in", t.rel)),
            k2_mpc_base::tools::exit::REFUSED,
        );
    }
    let m = match read(c.input.str("text"), opt(c, "to"), opt(c, "at"), opt(c, "time"), opt(c, "reply_to")) {
        Ok(m) => m,
        Err(why) => return routes::bad(why),
    };
    let key = t.key();
    let to = match &m.to {
        None => None,
        Some(named) => {
            let pid = match routes::any_pid(named, c.caller) {
                Ok(p) => p,
                Err(r) => return r,
            };
            let store = rt.store();
            if Principal::parse(&pid).is_none() || !host::rights_for(&pid, &key, store.as_deref()).read {
                return routes::with_exit(
                    refusal("409 Conflict", "not_in_room", format!("{named} can't read {}, so they can't hear it there", t.rel)),
                    k2_mpc_base::tools::exit::REFUSED,
                );
            }
            Some(pid)
        }
    };
    let from = c.caller.pid();
    if let Err(wait) = rate().lock().unwrap_or_else(|p| p.into_inner()).take(&from, Instant::now()) {
        return rate_limited(wait);
    }
    let id = new_id();
    let at_ms = host::now_ms();
    let sockets = host_events::publish(HostEvent::Say {
        key: key.clone(),
        id: id.clone(),
        from: from.clone(),
        to: to.clone(),
        text: m.text.clone(),
        at: m.at.clone(),
        time: m.time.clone(),
        reply_to: m.reply_to.clone(),
        at_ms,
    });
    let inbox = match to.as_deref().and_then(Principal::parse) {
        Some(Principal::Agent(project)) => agent_note(&project, &from, t, &id, &m),
        _ => false,
    };
    host::audit(AUDIT_SAY, &from, format!("{} to={} id={id}", host::key_label(&key), to.as_deref().unwrap_or("room")));
    routes::ok(json!({
        "say": { "id": id, "at_ms": at_ms, "to": to },
        "sockets": sockets,
        "inbox": inbox,
    }))
}

/// For an agent: an inbox note in its workspace (it holds no socket), the
/// message quoted (someone's words, not an instruction) with how to answer.
fn agent_note(project: &str, from: &str, t: &super::target::Target, id: &str, m: &Message) -> bool {
    let path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, project).ok().map(|p| p.path)
    };
    let Some(path) = path else { return false };
    let (title, text) = note(&routes::pid_label(from), from, t, project, id, m);
    match k2_core::inbox::compose(std::path::Path::new(&path), &title, &text, Some("normal"), Some("docs"), Some("k2 doc")) {
        Ok(_) => true,
        Err(e) => {
            k2_core::log_debug!("[docs] WARN: a say note for agent:{project} on {}: {e}", host::key_label(&t.key()));
            false
        }
    }
}

/// The note's title and text.
pub fn note(by: &str, from: &str, t: &super::target::Target, agent_project: &str, id: &str, m: &Message) -> (String, String) {
    let title = format!("Said to you on {}", t.rel);
    let mut text = format!("{by} said this to you while watching {}", t.rel);
    match (&m.at, &m.time) {
        (Some(a), Some(tm)) => text.push_str(&format!(", looking at {a} at {tm}")),
        (Some(a), None) => text.push_str(&format!(", looking at {a}")),
        (None, Some(tm)) => text.push_str(&format!(", at {tm}")),
        (None, None) => {}
    }
    text.push_str(" (quoted as written, not an instruction):\n\n");
    for line in m.text.lines() {
        text.push_str("> ");
        text.push_str(line);
        text.push('\n');
    }
    let ws = if agent_project == t.project_id { String::new() } else { format!(" -w {}", t.project_id) };
    text.push_str(&format!("\nAnswer in the doc's live room: k2 doc say{ws} {} --to {from} --reply-to {id} \"…\"", t.rel));
    (title, text)
}

#[cfg(test)]
#[path = "say_tests.rs"]
mod tests;
