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
//! saw. People are never guarded.
//!
//! **It survives a restart.** The table is kept in memory and written behind
//! to the docs store's host table (namespace [`LAST_READ_NS`], key
//! `<space>\x1f<path>\x1f<pid>`, value the version in decimal): the upkeep
//! tick flushes what changed (one transaction), release flushes the rest,
//! and a miss after a restart loads the row. So the `--if-version` safety
//! net holds across daemon restarts. A moved doc's rows move with it; a
//! removed workspace's go with its docs. No daemon-DB migration: the rows
//! live beside the docs they name.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::json;

use k2_core::log_debug;
use k2_mpc_base::session::{Session, SessionError, Stay};
use k2_mpc_base::store::{DocKey, DocStore, HostOp, Seq};

use super::caller::DocCaller;
use super::host::{self, Principal};
use super::routes::{self, Call};
use crate::cli_response::CliResponse;

/// The docs store's host-table namespace of the last-read rows.
pub const LAST_READ_NS: &str = "k2.docs.lastread";

/// Most (agent, doc) pairs kept in memory; past it the cache starts over
/// (after a flush: the store still has every row).
const LAST_READ_MAX: usize = 50_000;

#[derive(Default)]
struct LastRead {
    map: HashMap<(String, DocKey), Seq>,
    /// Recorded since the last flush.
    dirty: HashSet<(String, DocKey)>,
}

fn table() -> &'static Mutex<LastRead> {
    static T: OnceLock<Mutex<LastRead>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(LastRead::default()))
}

fn row_key(pid: &str, key: &DocKey) -> String {
    format!("{}\u{1f}{}\u{1f}{pid}", key.space, key.path)
}

/// Remember that `caller` saw `key` at `version` (agents only).
pub fn record_read(caller: &DocCaller, key: &DocKey, version: Seq) {
    let DocCaller::Agent { .. } = caller else { return };
    let mut t = table().lock().expect("docs last-read table");
    if t.map.len() >= LAST_READ_MAX && t.dirty.is_empty() {
        t.map.clear();
    }
    let k = (caller.pid(), key.clone());
    t.map.insert(k.clone(), version);
    t.dirty.insert(k);
}

/// The version `caller` (an agent) last saw in `key`, if any: memory, else
/// the store (after a restart).
pub fn last_read(caller: &DocCaller, key: &DocKey) -> Option<Seq> {
    let store = super::runtime().ok().and_then(|rt| rt.store());
    last_read_in(store.as_deref(), caller, key)
}

fn last_read_in(store: Option<&DocStore>, caller: &DocCaller, key: &DocKey) -> Option<Seq> {
    let DocCaller::Agent { .. } = caller else { return None };
    let pid = caller.pid();
    if let Some(v) = table().lock().expect("docs last-read table").map.get(&(pid.clone(), key.clone())) {
        return Some(*v);
    }
    let v = load(store?, &pid, key)?;
    table().lock().expect("docs last-read table").map.entry((pid, key.clone())).or_insert(v);
    Some(v)
}

/// One stored row (an unreadable one is logged and treated as no read:
/// the edit is then unguarded, never refused on a bad row).
fn load(store: &DocStore, pid: &str, key: &DocKey) -> Option<Seq> {
    match store.host_get(LAST_READ_NS, &row_key(pid, key)) {
        Ok(Some(b)) => std::str::from_utf8(&b).ok().and_then(|s| s.parse().ok()),
        Ok(None) => None,
        Err(e) => {
            log_debug!("[docs] WARN: last-read row of {pid} on {}: {e}", host::key_label(key));
            None
        }
    }
}

/// Write the reads recorded since the last flush, in one transaction (the
/// upkeep tick, and release). A failed write keeps them for the next one.
pub fn flush(store: &DocStore) {
    let rows: Vec<(String, String)> = {
        let mut t = table().lock().expect("docs last-read table");
        let dirty: Vec<(String, DocKey)> = t.dirty.drain().collect();
        dirty.into_iter().filter_map(|k| t.map.get(&k).map(|v| (row_key(&k.0, &k.1), v.to_string()))).collect()
    };
    if rows.is_empty() {
        return;
    }
    let ops: Vec<HostOp> =
        rows.iter().map(|(k, v)| HostOp::Put { ns: LAST_READ_NS, key: k, value: v.as_bytes() }).collect();
    if let Err(e) = store.host_batch(&ops) {
        log_debug!("[docs] WARN: last-read rows not written ({e}); retrying on the next tick");
        let mut t = table().lock().expect("docs last-read table");
        let again: Vec<(String, DocKey)> = t.map.keys().filter(|(p, k)| rows.iter().any(|(rk, _)| *rk == row_key(p, k))).cloned().collect();
        t.dirty.extend(again);
    }
}

/// A doc moved: what was read under the old key is the same doc (memory
/// and store).
pub fn rekey(store: Option<&DocStore>, old: &DocKey, new: &DocKey) {
    {
        let mut t = table().lock().expect("docs last-read table");
        let moved: Vec<(String, Seq)> =
            t.map.iter().filter(|((_, k), _)| k == old).map(|((pid, _), v)| (pid.clone(), *v)).collect();
        for (pid, v) in moved {
            t.map.remove(&(pid.clone(), old.clone()));
            t.dirty.remove(&(pid.clone(), old.clone()));
            t.map.insert((pid.clone(), new.clone()), v);
            t.dirty.insert((pid, new.clone()));
        }
    }
    let Some(store) = store else { return };
    let prefix = format!("{}\u{1f}{}\u{1f}", old.space, old.path);
    match store.host_scan(LAST_READ_NS, &prefix) {
        Ok(rows) => {
            let moved: Vec<(String, String, Vec<u8>)> = rows
                .into_iter()
                .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|pid| (k.clone(), row_key(pid, new), v)))
                .collect();
            let mut ops: Vec<HostOp> = Vec::new();
            for (oldk, newk, v) in &moved {
                ops.push(HostOp::Delete { ns: LAST_READ_NS, key: oldk });
                ops.push(HostOp::Put { ns: LAST_READ_NS, key: newk, value: v });
            }
            if !ops.is_empty() {
                if let Err(e) = store.host_batch(&ops) {
                    log_debug!("[docs] WARN: moving last-read rows of {}: {e}", host::key_label(old));
                }
            }
        }
        Err(e) => log_debug!("[docs] WARN: last-read rows of {}: {e}", host::key_label(old)),
    }
}

/// A removed workspace: its docs' last-read rows go with them. False when
/// the store refused (the sweep retries).
pub fn forget_space(store: Option<&DocStore>, space: &str) -> bool {
    {
        let mut t = table().lock().expect("docs last-read table");
        t.map.retain(|(_, k), _| k.space != space);
        t.dirty.retain(|(_, k)| k.space != space);
    }
    let Some(store) = store else { return true };
    let prefix = format!("{space}\u{1f}");
    let keys = match store.host_scan(LAST_READ_NS, &prefix) {
        Ok(rows) => rows.into_iter().map(|(k, _)| k).filter(|k| k.starts_with(&prefix)).collect::<Vec<_>>(),
        Err(e) => {
            log_debug!("[docs] WARN: last-read rows of {space}: {e}");
            return false;
        }
    };
    let ops: Vec<HostOp> = keys.iter().map(|k| HostOp::Delete { ns: LAST_READ_NS, key: k }).collect();
    ops.is_empty() || store.host_batch(&ops).map_err(|e| log_debug!("[docs] WARN: last-read rows of {space}: {e}")).is_ok()
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
        rekey(None, &k, &moved);
        assert_eq!(last_read(&agent, &moved), Some(42));
        assert_eq!(last_read(&agent, &k), None);
    }

    fn temp_store() -> (DocStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("k2-lastread-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        (DocStore::open(dir.join("docs.redb")).expect("open a temp docs store"), dir)
    }

    /// The --if-version safety net survives a restart: what was flushed
    /// loads back once memory is empty; a moved doc's rows move; a removed
    /// workspace's rows go.
    #[test]
    fn last_reads_survive_a_restart_through_the_docs_store() {
        let (store, dir) = temp_store();
        let agent = DocCaller::Agent { project_id: "lastread-agent".into() };
        let k = DocKey { space: "lastread-ws".into(), path: "notes/a.md".into() };
        record_read(&agent, &k, 1842);
        flush(&store);
        assert_eq!(load(&store, &agent.pid(), &k), Some(1842), "flushed to the store");
        // A restart: memory is gone, the row isn't.
        table().lock().expect("table").map.retain(|(p, _), _| *p != agent.pid());
        assert_eq!(last_read_in(Some(&store), &agent, &k), Some(1842), "loaded back after a restart");
        let moved = DocKey { space: "lastread-ws".into(), path: "notes/b.md".into() };
        rekey(Some(&store), &k, &moved);
        assert_eq!(load(&store, &agent.pid(), &k), None, "the old row moved");
        assert_eq!(load(&store, &agent.pid(), &moved), Some(1842), "to the new name");
        let other = DocKey { space: "lastread-other".into(), path: "x.md".into() };
        record_read(&agent, &other, 7);
        flush(&store);
        assert!(forget_space(Some(&store), "lastread-ws"));
        assert_eq!(load(&store, &agent.pid(), &moved), None, "a removed workspace's rows go");
        assert_eq!(last_read_in(Some(&store), &agent, &moved), None);
        assert_eq!(load(&store, &agent.pid(), &other), Some(7), "another workspace's stay");
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(human_secs(1), "1s");
        assert_eq!(human_secs(18_000), "18s");
        assert_eq!(human_secs(250_000), "4m 10s");
    }
}
