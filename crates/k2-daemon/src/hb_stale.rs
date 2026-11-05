//! Is a heartbeat's session still good to wake? (0.46.9, field report
//! 2026-11-05.)
//!
//! A heartbeat fire reuses the live terminal under `<project_id>:hb:<name>`
//! and types its wake prompt into it. A reuse never runs the login pick
//! (`spawn.rs`), so a terminal that started on a token that is now out of
//! its weekly limit (or signed out, or no longer the token a new start
//! would use) kept getting every wake, and every wake failed.
//!
//! Before a fire injects into, reuses or resumes a heartbeat session, it
//! asks [`check_live`] / [`check_saved`]. A session is **stale** when:
//!
//! - its last turn was a limit or sign-in error: a `<synthetic>` assistant
//!   message in its transcript whose text matches [`SIGNED_OUT_TEXTS`] or
//!   [`LIMIT_TEXTS`]; or
//! - the token it started on is not the one a new start would pick now
//!   (the same rules `/cli/llm/accounts/for` uses for `nextStart`).
//!
//! A stale session is replaced: the fire kills it and starts a new one,
//! which runs the login pick. The fire record says why
//! ([`StaleReason::restart_note`]).
//!
//! The statusLine `rate_limits` reading is not used as a signal: it is per
//! token, not per session, and a token at 100% with extra usage on still
//! works, so restarting on it would drop a working session's context on
//! every fire.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use k2_core::llm_accounts::{self as wallet, pins, store, Tool};
use k2_core::session::SessionId;
use serde_json::{json, Value};

// ── The stale reasons ───────────────────────────────────────────────

/// Why a heartbeat session is replaced instead of woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    /// It started on a token a new start would not pick now.
    TokenChanged,
    /// Its last turn was a weekly-limit error.
    WeeklyLimit,
    /// Its last turn was another usage-limit error (5-hour, credits).
    UsageLimit,
    /// Its last turn was a sign-in error (invalid key, run /login).
    SignedOut,
}

impl StaleReason {
    /// Short code for JSON answers.
    pub fn code(self) -> &'static str {
        match self {
            StaleReason::TokenChanged => "token_changed",
            StaleReason::WeeklyLimit => "weekly_limit",
            StaleReason::UsageLimit => "usage_limit",
            StaleReason::SignedOut => "signed_out",
        }
    }

    /// What the fire record and the heartbeat log say.
    pub fn restart_note(self) -> &'static str {
        match self {
            StaleReason::TokenChanged => "restarted: token changed",
            StaleReason::WeeklyLimit => "restarted: weekly limit",
            StaleReason::UsageLimit => "restarted: usage limit",
            StaleReason::SignedOut => "restarted: signed out",
        }
    }
}

/// A stale verdict with the facts behind it (for the fire record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub reason: StaleReason,
    pub detail: String,
}

// ── The vendor texts ────────────────────────────────────────────────

/// Sign-in errors a CLI writes as its own (`<synthetic>`) last turn.
/// Matched lowercase, as substrings. Checked before [`LIMIT_TEXTS`].
pub const SIGNED_OUT_TEXTS: &[&str] = &[
    "invalid api key",
    "please run /login",
    "run /login",
    "oauth token has expired",
    "oauth token revoked",
    "token has been revoked",
    "authentication_error",
    "invalid bearer token",
    "not logged in",
    "organization has been disabled",
];

/// Usage-limit errors a CLI writes as its own last turn. Matched
/// lowercase, as substrings. A plain "rate limit" (a short 429 that passes
/// on its own) is NOT here: restarting would not help it.
pub const LIMIT_TEXTS: &[&str] = &[
    "you've hit your",
    "you\u{2019}ve hit your",
    "you have hit your",
    "you've reached your",
    "you\u{2019}ve reached your",
    "you have reached your",
    "usage limit",
    "weekly limit",
    "5-hour limit",
    "session limit reached",
    "credit balance is too low",
    "out of extra usage",
];

/// Classify one error text. `None` = not a limit or sign-in error.
pub fn classify_error_text(text: &str) -> Option<StaleReason> {
    let t = text.to_lowercase();
    if SIGNED_OUT_TEXTS.iter().any(|p| t.contains(p)) {
        return Some(StaleReason::SignedOut);
    }
    if LIMIT_TEXTS.iter().any(|p| t.contains(p)) {
        if t.contains("weekly") || t.contains("week limit") {
            return Some(StaleReason::WeeklyLimit);
        }
        return Some(StaleReason::UsageLimit);
    }
    None
}

/// The text of one transcript message's `content` (a string, or blocks
/// with `text`).
fn message_text(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The last turn of a Claude transcript tail (JSONL, newest last): the
/// newest `user` or `assistant` line. A `<synthetic>` assistant line (the
/// CLI's own error, `isApiErrorMessage`) is classified; anything else is
/// healthy (`None`). A cut first line (we read a tail) is skipped.
pub fn last_turn_error(tail: &str) -> Option<StaleReason> {
    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let msg = v.get("message").cloned().unwrap_or(Value::Null);
                let synthetic = msg.get("model").and_then(Value::as_str) == Some("<synthetic>")
                    || v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true);
                if !synthetic {
                    return None;
                }
                return classify_error_text(&message_text(&msg));
            }
            Some("user") => return None,
            _ => continue,
        }
    }
    None
}

/// Bytes of a transcript's end that are read.
const TAIL_BYTES: u64 = 64 * 1024;

fn read_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len > TAIL_BYTES {
        f.seek(SeekFrom::Start(len - TAIL_BYTES)).ok()?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// `<home>/projects/*/<conversation>.jsonl` in the first home that has it.
fn find_claude_transcript(conversation: &str, homes: &[PathBuf]) -> Option<PathBuf> {
    if !pins::valid_conversation_id(conversation) {
        return None;
    }
    let name = format!("{conversation}.jsonl");
    for home in homes {
        let Ok(entries) = std::fs::read_dir(home.join("projects")) else { continue };
        for e in entries.flatten() {
            let p = e.path().join(&name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The last-turn verdict of a Claude conversation, looked up in `homes`
/// (the home it ran in first, then the live home).
pub fn transcript_verdict(conversation: &str, homes: &[PathBuf]) -> Option<StaleReason> {
    let path = find_claude_transcript(conversation, homes)?;
    last_turn_error(&read_tail(&path)?)
}

// ── The decision table ──────────────────────────────────────────────

/// What [`decide`] weighs. Pure, so the table is unit-tested.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaleInputs {
    /// The token the session started on (`Some(None)` = the live login
    /// with no wallet entry). `None` = not known: no token check.
    pub started_on: Option<Option<String>>,
    /// The token a new start would pick now. `None` = not known.
    pub fresh_pick: Option<Option<String>>,
    /// The transcript's last turn, when it was a limit or sign-in error.
    pub last_turn: Option<StaleReason>,
}

/// The stale verdict: the last turn's error first (it is what actually
/// happened), then a token change. `None` = healthy.
pub fn decide(i: &StaleInputs) -> Option<StaleReason> {
    if let Some(r) = i.last_turn {
        return Some(r);
    }
    match (&i.started_on, &i.fresh_pick) {
        (Some(a), Some(b)) if a != b => Some(StaleReason::TokenChanged),
        _ => None,
    }
}

// ── The token a session started on ──────────────────────────────────

/// The login a heartbeat session started on, kept per session id (and in
/// its ptyd meta, so an adopted session keeps it across a restart).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedOn {
    pub tool: Tool,
    /// The wallet token (or a person's item). For the server default it is
    /// the token that was the default at spawn time.
    pub account: Option<String>,
    /// The home its conversation runs in (`None` = the live home).
    pub home: Option<PathBuf>,
}

impl StartedOn {
    pub fn to_json(&self) -> Value {
        json!({
            "tool": self.tool.as_str(),
            "account": self.account,
            "home": self.home.as_ref().map(|h| h.to_string_lossy().into_owned()),
        })
    }

    pub fn from_json(v: &Value) -> Option<StartedOn> {
        let tool = Tool::parse(v.get("tool")?.as_str()?)?;
        Some(StartedOn {
            tool,
            account: v.get("account").and_then(Value::as_str).map(str::to_string),
            home: v.get("home").and_then(Value::as_str).map(PathBuf::from),
        })
    }
}

fn started_map() -> &'static Mutex<HashMap<String, StartedOn>> {
    static M: std::sync::OnceLock<Mutex<HashMap<String, StartedOn>>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember what a heartbeat session started on.
pub fn note_started_on(session_id: &SessionId, s: StartedOn) {
    let mut m = started_map().lock().unwrap_or_else(|p| p.into_inner());
    // Bounded: drop entries whose session is gone.
    if m.len() > 256 {
        m.retain(|sid, _| {
            SessionId::parse(sid).is_some_and(|id| crate::v2_session_map::lookup_by_session_id(&id).is_some())
        });
    }
    m.insert(session_id.to_string(), s);
}

/// What a session started on, if K2 spawned it (or adopted its meta).
pub fn started_on(session_id: &SessionId) -> Option<StartedOn> {
    started_map().lock().unwrap_or_else(|p| p.into_inner()).get(&session_id.to_string()).cloned()
}

/// k2-ptyd: an adopted heartbeat session keeps its record.
pub fn reseed_after_adoption(session_id: &SessionId, meta: &Value) {
    if let Some(s) = StartedOn::from_json(meta) {
        note_started_on(session_id, s);
    }
}

/// The server default's token as a session would run on it: the pool's
/// active API key, else the login in the tool's live store.
fn pool_effective(conn: &rusqlite::Connection, tool: Tool) -> Option<String> {
    if let Some(id) = wallet::active_id(conn, tool).ok().flatten() {
        if wallet::get(conn, &id).ok().flatten().is_some_and(|e| e.is_api_key()) {
            return Some(id);
        }
    }
    wallet::live_owner(conn, tool).ok().flatten()
}

/// What a spawn starts on, from its login pick (`None` login = the pool
/// with nothing to inject). `None` for programs that are not an LLM CLI.
pub fn started_on_for_spawn(program: Option<&str>, login: Option<&pins::SpawnLogin>) -> Option<StartedOn> {
    let tool = program.and_then(Tool::from_command)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let account = match login.and_then(|l| l.account_id.clone()) {
        Some(id) => Some(id),
        None => pool_effective(&conn, tool),
    };
    Some(StartedOn { tool, account, home: login.and_then(|l| l.home.clone()) })
}

/// The token a NEW start of `key` would run on now: `(decision, effective)`.
/// `decision` keeps the server default as `None` (the spawn's own record
/// shape); `effective` resolves it to the token that is the default now.
pub fn fresh_pick_now(
    conn: &rusqlite::Connection,
    tool: Tool,
    key: &str,
    project_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    if let Ok(Some(c)) = k2_core::person_wallet::decide::candidate(conn, tool, key, Some(project_id), None, false) {
        return Some((Some(c.item_id.clone()), Some(c.item_id)));
    }
    let d = pins::decide(conn, tool, key, Some(project_id), None, false).ok()?;
    match d.account {
        Some(e) => Some((Some(e.id.clone()), Some(e.id))),
        None => Some((None, pool_effective(conn, tool))),
    }
}

/// The test seam's stand-in agent (`K2SO_WAKE_HEADLESS_TEST_COMMAND`)
/// keeps Claude's transcript keying, as `wake_headless` does.
fn transcript_tool(program: Option<&str>) -> Option<Tool> {
    if let Some(t) = program.and_then(Tool::from_command) {
        return Some(t);
    }
    let over = std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND").ok().filter(|c| !c.is_empty())?;
    let p = program?;
    (crate::spawn::program_basename(p) == crate::spawn::program_basename(&over)).then_some(Tool::Claude)
}

fn homes_for(tool: Tool, first: Option<PathBuf>) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = first.into_iter().collect();
    let live = store::live_home(tool).0;
    if !homes.contains(&live) {
        homes.push(live);
    }
    homes
}

fn token_words(t: &Option<String>) -> String {
    match t {
        Some(id) => format!("token {id}"),
        None => "the CLI's own login".to_string(),
    }
}

fn detail_for(reason: StaleReason, started: &Option<Option<String>>, fresh: &Option<Option<String>>) -> String {
    match reason {
        StaleReason::TokenChanged => format!(
            "it started on {}; a new start now uses {}",
            started.as_ref().map(token_words).unwrap_or_default(),
            fresh.as_ref().map(token_words).unwrap_or_default(),
        ),
        StaleReason::WeeklyLimit => "its last turn was a weekly-limit error".to_string(),
        StaleReason::UsageLimit => "its last turn was a usage-limit error".to_string(),
        StaleReason::SignedOut => "its last turn was a sign-in error".to_string(),
    }
}

/// Is the live heartbeat terminal `session` (under `key`) stale?
/// `saved_conversation` (the heartbeat's `last_session_id`) names the
/// conversation when the argv doesn't.
pub fn check_live(
    project_id: &str,
    key: &str,
    session: &k2_core::terminal::DaemonPtySession,
    saved_conversation: Option<&str>,
) -> Option<Stale> {
    let program = session.program.as_deref();
    let conversation = k2_core::workspace::provider_resume::session_id_from_spawn_argv(program.unwrap_or(""), &session.args)
        .or_else(|| saved_conversation.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
    let started = started_on(&session.session_id);
    let tool = program.and_then(Tool::from_command);

    let last_turn = match (transcript_tool(program), conversation.as_deref()) {
        (Some(Tool::Claude), Some(c)) => {
            transcript_verdict(c, &homes_for(Tool::Claude, started.as_ref().and_then(|s| s.home.clone())))
        }
        _ => None,
    };
    let (started_acct, fresh) = match tool {
        Some(t) => {
            let fresh = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                fresh_pick_now(&conn, t, key, project_id)
            };
            let started_acct = started.as_ref().filter(|s| s.tool == t).map(|s| s.account.clone());
            (started_acct, fresh.map(|(_, eff)| eff))
        }
        None => (None, None),
    };
    let inputs = StaleInputs { started_on: started_acct, fresh_pick: fresh, last_turn };
    let reason = decide(&inputs)?;
    Some(Stale { reason, detail: detail_for(reason, &inputs.started_on, &inputs.fresh_pick) })
}

/// Is the heartbeat's saved conversation (no live terminal; the next fire
/// would resume it with `command`) stale? Compares the token the
/// conversation was recorded on with a new start's pick (the server
/// default stays `None` on both sides: a resume of a default-token
/// conversation runs on whatever the default is now).
pub fn check_saved(project_id: &str, key: &str, command: &str, conversation: &str) -> Option<Stale> {
    let tool = Tool::from_command(command)?;
    let (recorded, fresh) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let recorded = pins::recorded_account(&conn, tool, key, Some(conversation)).ok().flatten();
        let fresh = fresh_pick_now(&conn, tool, key, project_id).map(|(d, _)| d);
        (recorded, fresh)
    };
    let last_turn = if tool == Tool::Claude {
        let first = recorded.as_ref().and_then(|a| a.as_ref()).map(|id| store::slot_dir(tool, id));
        transcript_verdict(conversation, &homes_for(tool, first))
    } else {
        None
    };
    let inputs = StaleInputs { started_on: recorded, fresh_pick: fresh, last_turn };
    let reason = decide(&inputs)?;
    Some(Stale { reason, detail: detail_for(reason, &inputs.started_on, &inputs.fresh_pick) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(text: &str) -> String {
        json!({"type": "assistant", "isApiErrorMessage": true,
               "message": {"model": "<synthetic>", "role": "assistant",
                           "content": [{"type": "text", "text": text}]}})
        .to_string()
    }

    fn user(text: &str) -> String {
        json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
    }

    fn reply(text: &str) -> String {
        json!({"type": "assistant", "message": {"model": "claude-opus-4", "role": "assistant",
               "content": [{"type": "text", "text": text}]}})
        .to_string()
    }

    /// Every vendor text in the named lists classifies, and as the right kind.
    #[test]
    fn every_listed_text_classifies() {
        for t in SIGNED_OUT_TEXTS {
            assert_eq!(classify_error_text(t), Some(StaleReason::SignedOut), "{t}");
        }
        for t in LIMIT_TEXTS {
            let got = classify_error_text(t);
            assert!(
                matches!(got, Some(StaleReason::WeeklyLimit | StaleReason::UsageLimit)),
                "{t}: {got:?}"
            );
        }
    }

    /// The texts the CLIs actually print.
    #[test]
    fn real_vendor_messages() {
        let cases = [
            ("You've hit your weekly limit · resets Nov 9, 10am", Some(StaleReason::WeeklyLimit)),
            ("Weekly limit reached ∙ resets Oct 9 at 10am", Some(StaleReason::WeeklyLimit)),
            ("5-hour limit reached ∙ resets 3pm", Some(StaleReason::UsageLimit)),
            ("Claude AI usage limit reached|1762300800", Some(StaleReason::UsageLimit)),
            ("You've hit your limit · resets 3pm (America/Denver)", Some(StaleReason::UsageLimit)),
            ("Credit balance is too low", Some(StaleReason::UsageLimit)),
            ("Invalid API key · Please run /login", Some(StaleReason::SignedOut)),
            ("OAuth token has expired. Please obtain a new token or refresh your existing token.", Some(StaleReason::SignedOut)),
            ("API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\"}}", Some(StaleReason::SignedOut)),
            // A passing 429 and ordinary failures are not stale.
            ("API Error: Rate limit reached", None),
            ("API Error: 529 Overloaded", None),
            ("Request timed out", None),
            ("No response requested.", None),
        ];
        for (text, want) in cases {
            assert_eq!(classify_error_text(text), want, "{text}");
        }
    }

    #[test]
    fn last_turn_reads_the_newest_turn_only() {
        // The weekly limit after the wake prompt: stale.
        let tail = [user("wake up"), synthetic("You've hit your weekly limit · resets Nov 9")].join("\n");
        assert_eq!(last_turn_error(&tail), Some(StaleReason::WeeklyLimit));
        // A system line after it doesn't hide it.
        let tail = format!("{tail}\n{}", json!({"type": "system", "subtype": "info"}));
        assert_eq!(last_turn_error(&tail), Some(StaleReason::WeeklyLimit));
        // A limit long ago, then a real reply: healthy.
        let tail = [synthetic("You've hit your weekly limit"), user("again"), reply("Done.")].join("\n");
        assert_eq!(last_turn_error(&tail), None);
        // A user turn waiting for its answer: healthy.
        let tail = [reply("Done."), user("next")].join("\n");
        assert_eq!(last_turn_error(&tail), None);
        // A real reply that quotes the words is not a synthetic error.
        assert_eq!(last_turn_error(&reply("You've hit your weekly limit, the CLI said.")), None);
        // A cut first line (we read a tail) and an empty file: healthy.
        let tail = format!("pe\":\"assistant\"}}\n{}", synthetic("Invalid API key · Please run /login"));
        assert_eq!(last_turn_error(&tail), Some(StaleReason::SignedOut));
        assert_eq!(last_turn_error(""), None);
    }

    /// The decision table: token changed, limit, signed out, healthy.
    #[test]
    fn decision_table() {
        let a = || Some(Some("acct-a".to_string()));
        let b = || Some(Some("acct-b".to_string()));
        let rows: Vec<(StaleInputs, Option<StaleReason>)> = vec![
            // healthy: same token, no error
            (StaleInputs { started_on: a(), fresh_pick: a(), last_turn: None }, None),
            // token changed
            (StaleInputs { started_on: a(), fresh_pick: b(), last_turn: None }, Some(StaleReason::TokenChanged)),
            // the default had no wallet entry, a token is picked now
            (StaleInputs { started_on: Some(None), fresh_pick: a(), last_turn: None }, Some(StaleReason::TokenChanged)),
            // weekly limit on the same token
            (StaleInputs { started_on: a(), fresh_pick: a(), last_turn: Some(StaleReason::WeeklyLimit) }, Some(StaleReason::WeeklyLimit)),
            // signed out
            (StaleInputs { started_on: a(), fresh_pick: a(), last_turn: Some(StaleReason::SignedOut) }, Some(StaleReason::SignedOut)),
            // both: the error names it
            (StaleInputs { started_on: a(), fresh_pick: b(), last_turn: Some(StaleReason::UsageLimit) }, Some(StaleReason::UsageLimit)),
            // unknown start (spawned before this build): no token check
            (StaleInputs { started_on: None, fresh_pick: b(), last_turn: None }, None),
            // the pick can't be read: no token check
            (StaleInputs { started_on: a(), fresh_pick: None, last_turn: None }, None),
        ];
        for (inputs, want) in rows {
            assert_eq!(decide(&inputs), want, "{inputs:?}");
        }
    }

    #[test]
    fn restart_notes_are_the_fire_record_words() {
        assert_eq!(StaleReason::TokenChanged.restart_note(), "restarted: token changed");
        assert_eq!(StaleReason::WeeklyLimit.restart_note(), "restarted: weekly limit");
        assert_eq!(StaleReason::SignedOut.restart_note(), "restarted: signed out");
        assert_eq!(StaleReason::UsageLimit.restart_note(), "restarted: usage limit");
    }

    #[test]
    fn transcript_is_found_in_a_slot_home_or_the_live_home() {
        let dir = std::env::temp_dir().join(format!("k2-hbstale-{}", uuid::Uuid::new_v4().simple()));
        let slot = dir.join("slot");
        let live = dir.join("live");
        let conv = uuid::Uuid::new_v4().to_string();
        let proj = slot.join("projects").join("-tmp-acme");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(live.join("projects")).unwrap();
        std::fs::write(proj.join(format!("{conv}.jsonl")), synthetic("You've hit your weekly limit")).unwrap();
        assert_eq!(
            transcript_verdict(&conv, &[live.clone(), slot.clone()]),
            Some(StaleReason::WeeklyLimit),
            "found in the second home"
        );
        assert_eq!(transcript_verdict(&conv, &[live.clone()]), None, "not in the live home");
        assert_eq!(transcript_verdict("../etc/passwd", &[slot.clone()]), None, "a bad id is never a path");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn started_on_round_trips_through_ptyd_meta() {
        let s = StartedOn { tool: Tool::Claude, account: Some("acct-a".into()), home: Some(PathBuf::from("/tmp/slot")) };
        assert_eq!(StartedOn::from_json(&s.to_json()), Some(s));
        let s = StartedOn { tool: Tool::Codex, account: None, home: None };
        assert_eq!(StartedOn::from_json(&s.to_json()), Some(s));
        assert_eq!(StartedOn::from_json(&json!({"tool": "nope"})), None);
    }
}
