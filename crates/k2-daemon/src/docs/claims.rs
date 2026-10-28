//! Agent presence on K2's side (kit amendments 13 and 14): claims bounce
//! agents' edits, the bounce reaches the agent and Chatter, the agent waits
//! in line, and is told when the lines are free; and every agent edit is
//! guarded by the version that agent last read.
//!
//! **Rosson's rule.** When an agent's edit steps on another's claim, the
//! bounce goes back into the agent's terminal (the CLI's stderr, exit 5)
//! and lands in Chatter. K2 puts the agent in the claim's queue
//! (`register_waiter`); when the claim ends (`ClaimEnded`) or its hand-off
//! passes on (`ReservationEnded`) with that agent next, K2 messages it that
//! the lines are free, with the version the doc is at. The agent re-reads
//! before editing: its next edit is guarded by what it last read.
//!
//! **Last read** ([`record_read`], [`last_read`]): the version each agent
//! last saw in each doc, from a read verb's `ToolOutput::version`,
//! `read`'s `ReadResult::version`, or its own edit's version after it. K2
//! passes it as `--if-version` on the agent's next edit verb (unless it
//! gave one), so an edit made from a stale read is refused (`doc changed
//! since V (now W)`, exit 1) instead of landing on text the agent never
//! saw. Memory only; a restart forgets it (the next edit is unguarded until
//! the agent reads again). People are never guarded.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::json;

use k2_core::log_debug;
use k2_mpc_base::session::{Session, SessionError, Stay};
use k2_mpc_base::store::{DocKey, Seq};

use super::caller::DocCaller;
use super::host::{self, Principal};
use super::routes::{self, Call};
use crate::cli_response::CliResponse;

/// Most (agent, doc) pairs remembered; past it the table starts over.
const LAST_READ_MAX: usize = 50_000;

fn table() -> &'static Mutex<HashMap<(String, DocKey), Seq>> {
    static T: OnceLock<Mutex<HashMap<(String, DocKey), Seq>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember that `caller` saw `key` at `version` (agents only).
pub fn record_read(caller: &DocCaller, key: &DocKey, version: Seq) {
    let DocCaller::Agent { .. } = caller else { return };
    let mut t = table().lock().expect("docs last-read table");
    if t.len() >= LAST_READ_MAX {
        t.clear();
    }
    t.insert((caller.pid(), key.clone()), version);
}

/// The version `caller` (an agent) last saw in `key`, if any.
pub fn last_read(caller: &DocCaller, key: &DocKey) -> Option<Seq> {
    let DocCaller::Agent { .. } = caller else { return None };
    table().lock().expect("docs last-read table").get(&(caller.pid(), key.clone())).copied()
}

/// A doc moved: what was read under the old key is the same doc.
pub fn rekey(old: &DocKey, new: &DocKey) {
    let mut t = table().lock().expect("docs last-read table");
    let moved: Vec<(String, Seq)> =
        t.iter().filter(|((_, k), _)| k == old).map(|((pid, _), v)| (pid.clone(), *v)).collect();
    for (pid, v) in moved {
        t.remove(&(pid.clone(), old.clone()));
        t.insert((pid, new.clone()), v);
    }
}

/// `argv` with `--if-version <last read>` after the verb, for an agent's
/// edit verb that names none. Returns the version it added.
pub fn guard_argv(caller: &DocCaller, key: &DocKey, argv: &mut Vec<String>) -> Option<Seq> {
    if argv.iter().take_while(|a| a.as_str() != "--").any(|a| a == "--if-version" || a.starts_with("--if-version=")) {
        return None;
    }
    let v = last_read(caller, key)?;
    argv.insert(1, "--if-version".into());
    argv.insert(2, v.to_string());
    Some(v)
}

/// An agent's edit bounced off someone's claim (exit 5): queue it for the
/// lines (`register_waiter`), put the bounce in Chatter, and answer with
/// what happened and what comes next. A person (never bounced by edits,
/// only by a claim of its own that overlaps) just gets the refusal.
pub(super) fn bounced(s: &Session, c: &Call, e: &SessionError) -> CliResponse {
    let SessionError::Claimed { id, by, name, lines, remaining_ms, note } = e else {
        return routes::session_error(e);
    };
    let DocCaller::Agent { project_id } = c.caller else {
        return routes::session_error(e);
    };
    let queued = s
        .join_shared(c.caller.participant(c.on_behalf), Stay::UntilIdle)
        .and_then(|j| s.register_waiter(j.conn, *id));
    let place = match &queued {
        Ok(w) => Some(w.position),
        Err(err) => {
            log_debug!("[docs] {} not queued for {id}: {err}", c.caller.pid());
            None
        }
    };
    let who = routes::pid_label(by);
    let what = note.as_deref().map(|n| format!(" (\"{n}\")")).unwrap_or_default();
    let next = match place {
        Some(p) => format!(
            "You're #{p} in line: K2 will message you when they're free, with the version to re-read (k2 doc cat {}) before you edit again",
            c.target.rel
        ),
        None => format!("Retry after it ends (in {}), re-reading first", human_secs(*remaining_ms)),
    };
    let hint = format!(
        "nothing applied: lines {}–{} of {} are claimed by {who}{what} for another {}. {next}.",
        lines.0,
        lines.1,
        c.target.rel,
        human_secs(*remaining_ms)
    );
    chatter_bounce(project_id.clone(), by.clone(), name.clone(), c.target.rel.clone(), hint.clone());
    let body = json!({
        "ok": false,
        "error": {
            "code": "claimed",
            "hint": hint,
            "claim": id.to_string(),
            "by": by,
            "name": name,
            "lines": [lines.0, lines.1],
            "remainingMs": remaining_ms,
            "retryAfterMs": remaining_ms,
            "queued": place.is_some(),
            "position": place,
            "exit": k2_mpc_base::tools::session_exit(e),
        },
    });
    CliResponse { status: "409 Conflict", content_type: "application/json", body: body.to_string() }
}

/// `12s`, `4m 10s`.
fn human_secs(ms: u64) -> String {
    let s = ms.div_ceil(1000);
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {}s", s / 60, s % 60)
    }
}

/// A workspace's root path by project id.
fn workspace_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Project::get(&conn, project_id).ok().map(|p| p.path)
}

/// The bounce in Chatter: from the claimant (its workspace when it is an
/// agent here) to the bounced agent. Off the request's critical path.
fn chatter_bounce(bounced_project: String, by: String, by_name: String, rel: String, text: String) {
    let _ = std::thread::Builder::new().name("k2-docs-bounce".into()).spawn(move || {
        let Some(to) = workspace_path(&bounced_project) else { return };
        let from = match Principal::parse(&by) {
            Some(Principal::Agent(id)) => workspace_path(&id).unwrap_or(by_name),
            _ => by_name,
        };
        let summary = format!("k2 doc: an edit to {rel} waited for a claim");
        crate::overlay_routes::record_inject_chatter(&to, &from, &format!("k2 doc: {text}"), "doc", None, Some(&summary));
    });
}

/// The lines a waiting agent queued for are free: tell it (a live message
/// into its session, which also lands in Chatter; Chatter alone when it
/// isn't running), with the version to re-read at. People aren't messaged.
pub fn notify_free(key: DocKey, waiter: String, holder: String, lines: (usize, usize), version: Seq) {
    let Some(Principal::Agent(project_id)) = Principal::parse(&waiter) else { return };
    let _ = std::thread::Builder::new().name("k2-docs-free".into()).spawn(move || {
        let Some(to) = workspace_path(&project_id) else { return };
        let ws = if project_id == key.space { String::new() } else { format!(" -w {}", workspace_path(&key.space).unwrap_or(key.space.clone())) };
        let text = format!(
            "k2 doc: lines {}–{} of {} are free now ({} finished). The doc is at v{version}: re-read before editing (k2 doc cat{ws} {}); your next edit there is checked against what you read.",
            lines.0,
            lines.1,
            key.path,
            routes::pid_label(&holder),
            key.path
        );
        let r = crate::workspace_msg::deliver_live_with_via(&to, &text, "k2", "doc", false, Duration::from_secs(3), "doc");
        if !r.success {
            log_debug!("[docs] free-now for {waiter} on {} not delivered live ({:?}); Chatter only", host::key_label(&key), r.reason);
            let summary = format!("k2 doc: {} is free now", key.path);
            crate::overlay_routes::record_inject_chatter(&to, "k2", &text, "doc", None, Some(&summary));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(p: &str) -> DocKey {
        DocKey { space: "ws".into(), path: p.into() }
    }

    #[test]
    fn an_agents_last_read_guards_its_next_edit_and_a_person_is_never_guarded() {
        let agent = DocCaller::Agent { project_id: "claims-test-a".into() };
        let k = key("claims-test.md");
        let mut argv = vec!["sed".to_string(), "-i".into(), "s/a/b/".into()];
        assert_eq!(guard_argv(&agent, &k, &mut argv), None, "never read: no guard");
        record_read(&agent, &k, 42);
        assert_eq!(guard_argv(&agent, &k, &mut argv), Some(42));
        assert_eq!(argv, ["sed", "--if-version", "42", "-i", "s/a/b/"]);
        let mut given = vec!["append".to_string(), "--if-version=7".into()];
        assert_eq!(guard_argv(&agent, &k, &mut given), None, "the agent's own --if-version wins");
        assert_eq!(given, ["append", "--if-version=7"]);
        let person = super::super::caller::owner();
        record_read(&person, &k, 9);
        assert_eq!(last_read(&person, &k), None);
        let moved = key("claims-test-moved.md");
        rekey(&k, &moved);
        assert_eq!(last_read(&agent, &moved), Some(42));
        assert_eq!(last_read(&agent, &k), None);
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(human_secs(1), "1s");
        assert_eq!(human_secs(18_000), "18s");
        assert_eq!(human_secs(250_000), "4m 10s");
    }
}
