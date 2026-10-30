//! Settings → Cores → Multiplayer (prd-multiplayer-cores-settings-v1): which
//! multiplayer cores are on for this box, and what turning one on or off does.
//!
//! - **The mirror** (§2.1): one flag per core ([`k2_core::docs_cores::CORES`]),
//!   set at boot from `settings.json` ([`boot`]) and on every settings update
//!   and reset ([`sync_multiplayer_cores`]). A missing key is off.
//! - **The gate** (MC-3, MC-4): [`gate`] refuses every docs open of a file
//!   whose core is off with `403 core_off` (exit 3). It sits at the top of
//!   `routes::open` and `tier0::open`, so it covers already-loaded sessions,
//!   the `Unloaded` retry and export's fallback. It is keyed on the file's
//!   name (`Tier::of`), so a Markdown file its pack refuses is still a
//!   Markdown-core file. It is never `SessionError::Declined`, which would
//!   send the file to tier 0.
//! - **The flip** (§6): turning a core off closes that kind's loaded sessions
//!   through the kit's host close (`SessionManager::close_where`, reason
//!   `core_off`: unwritten changes written back first, claims and presence
//!   ended, out of the pool), then ends that kind's Live sockets
//!   ([`super::live::close_core`], one path for both), audits
//!   one line per core with the actor's label (MC-9), then publishes
//!   `doc_cores_changed` to every client (MC-8) and `sync:settings` to local
//!   windows. A failed write-back never stops the flip (§6.2). Turning a core
//!   on starts the docs runtime if it isn't running (lazy start, MC-6).
//! - **The read** `GET /cli/docs/cores` (§2.3): the rows Settings and
//!   `k2 doc cores` show. The write is never a docs route (MC-2): it is
//!   `POST /cli/settings/update {"multiplayerCores": {...}}` on the
//!   Owner/Admin key list, which agents can't reach.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use k2_core::docs_cores::{self, CoreView, CORES};
use k2_core::log_debug;
use k2_mpc_base::session::DiskState;
use k2_mpc_base::store::DocKey;
use serde_json::{json, Value};

use super::caller::DocCaller;
use super::target::{Target, Tier};
use crate::cli_response::CliResponse;

/// `GET /cli/docs/cores`.
pub const CORES_ROUTE: &str = "/cli/docs/cores";

/// The close reason the kit's `close_where` gets (and the live socket's
/// final frame will carry).
pub const CLOSE_REASON: &str = "core_off";

/// Access Audit events.
pub const AUDIT_SWITCH: &str = "doc-core-switch";
pub const AUDIT_CLOSE_FAILED: &str = "doc-core-close-failed";

/// At most this many paths in a row's `open.paths`.
const MAX_PATHS: usize = 5;

// ── the mirror ──────────────────────────────────────────────────────────

fn flags() -> &'static [AtomicBool] {
    static FLAGS: OnceLock<Vec<AtomicBool>> = OnceLock::new();
    FLAGS.get_or_init(|| CORES.iter().map(|_| AtomicBool::new(false)).collect())
}

fn index(id: &str) -> Option<usize> {
    CORES.iter().position(|c| c.id == id)
}

/// Whether core `id` is on in this process (an unknown id: off).
pub fn is_on(id: &str) -> bool {
    index(id).is_some_and(|i| flags()[i].load(Ordering::SeqCst))
}

/// Whether any core is on (the docs runtime runs only then).
pub fn any_on() -> bool {
    flags().iter().any(|f| f.load(Ordering::SeqCst))
}

/// Every core's state, by id.
pub fn snapshot() -> BTreeMap<String, bool> {
    CORES.iter().map(|c| (c.id.to_string(), is_on(c.id))).collect()
}

fn set(id: &str, on: bool) {
    if let Some(i) = index(id) {
        flags()[i].store(on, Ordering::SeqCst);
    }
}

/// Test-only setter (MC-18): unit tests that call `docs::routes::handle`
/// directly turn a core on or off without a settings write.
#[cfg(test)]
pub fn set_for_test(id: &str, on: bool) {
    set(id, on);
}

/// The core a file belongs to, by its name (`Tier::of`, §4.1).
pub fn core_of(tier: Tier) -> &'static str {
    match tier {
        Tier::Markdown => docs_cores::MARKDOWN,
        Tier::Text(_) => docs_cores::TEXT,
        Tier::Blob => docs_cores::ANY_FILE,
    }
}

/// The core of a workspace-relative path.
pub fn core_of_path(rel: &str) -> &'static str {
    core_of(Tier::of(rel))
}

/// The core as a sentence names it: "the Markdown core".
fn sentence_name(id: &str) -> &'static str {
    match id {
        docs_cores::MARKDOWN => "the Markdown core",
        docs_cores::TEXT => "the text and code core",
        _ => "the any-file core",
    }
}

// ── the gate ────────────────────────────────────────────────────────────

/// The refusal for a file whose core is off: `403 core_off`, exit 3, with
/// `core` and `path` for the permission wrapper (S6) and the CLI.
pub fn core_off(core: &str, rel: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": {
                "code": "core_off",
                "hint": format!(
                    "{rel}: {} is off on this server. The owner or an Admin turns it on in Settings → Cores → Multiplayer (k2 doc cores on {core}, people only). Until then, edit the file with ordinary tools.",
                    sentence_name(core)
                ),
                "core": core,
                "path": rel,
                "exit": k2_mpc_base::tools::exit::REFUSED,
            },
        })
        .to_string(),
    }
}

/// Refuse a docs open of `t` while its core is off (MC-4). Called before any
/// session is looked up or loaded, so a refused file leaves no session and no
/// presence.
pub fn gate(t: &Target) -> Result<(), CliResponse> {
    let core = core_of(t.tier);
    if is_on(core) {
        Ok(())
    } else {
        Err(core_off(core, &t.rel))
    }
}

// ── closing a core's sessions ───────────────────────────────────────────

/// One loaded session closed by a turn-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    pub key: DocKey,
    /// The file was written now (it held unwritten changes).
    pub written: bool,
    /// The write-back failed: the change stays in the docs store's log, and
    /// the next load writes it (§6.2).
    pub error: Option<String>,
}

/// Closes loaded sessions: the docs runtime, or a fake in tests.
pub trait CloseSessions {
    /// Close every loaded session (text and tier 0) whose key matches: write
    /// back unwritten changes now, then end claims and presence. One entry
    /// per session.
    fn close_where(&self, matches: &dyn Fn(&DocKey) -> bool) -> Vec<Closed>;
}

impl CloseSessions for super::Runtime {
    /// The kit's host close (v0.9.7.2, `SessionManager::close_where`, reason
    /// [`CLOSE_REASON`]): every picked session, text and tier 0, members or
    /// not. It writes back unwritten changes first, ends claims and presence,
    /// sends every roster subscription a last `RosterEvent::Closed`, audits
    /// `SessionClosed`, and takes the session out of the pool, so the tick no
    /// longer merges outside saves into it (the next open takes them in). A
    /// handle held elsewhere then fails with `SessionError::Closed`
    /// (MPC-E519): K2's routes reopen once and the gate answers `core_off`.
    /// Waiting outside-save notes are kept by the kit for the next open.
    ///
    /// The kit returns the keys; K2 adds, per key, whether the file held
    /// unwritten changes (the session's status just before the close) and the
    /// write-back's error when it failed (the kit's `DiskError`, reported on
    /// this thread while it closes).
    fn close_where(&self, matches: &dyn Fn(&DocKey) -> bool) -> Vec<Closed> {
        let dirty: HashSet<DocKey> =
            self.manager.loaded_keys().into_iter().filter(|k| matches(k) && self.holds_unwritten(k)).collect();
        let (closed, disk_errors) =
            super::host::collect_disk_errors(|| self.manager.close_where(CLOSE_REASON, |key, _pack| matches(key)));
        let keys = match closed {
            Ok(keys) => keys,
            Err(e) => {
                // The pool is gone (released) or poisoned: nothing loaded is
                // left to close, and nothing new opens past the gate.
                log_debug!("[docs] WARN: closing sessions ({CLOSE_REASON}): {e}");
                return Vec::new();
            }
        };
        keys.into_iter()
            .map(|key| {
                self.forget(&key);
                let error = disk_errors.iter().rev().find(|(k, _)| *k == key).map(|(_, e)| e.clone());
                Closed { written: error.is_none() && dirty.contains(&key), error, key }
            })
            .collect()
    }
}

impl super::Runtime {
    /// Whether the loaded doc at `key` holds changes its file doesn't have
    /// yet (never loads).
    fn holds_unwritten(&self, key: &DocKey) -> bool {
        let disk = if let Some(s) = self.manager.get(key) {
            s.status().ok().and_then(|st| st.disk)
        } else if let Some(b) = self.manager.get_blob(key) {
            b.status().ok().and_then(|st| st.disk)
        } else {
            None
        };
        matches!(disk, Some(DiskState::Dirty { .. }))
    }
}

// ── the flip ────────────────────────────────────────────────────────────

/// What one flip did, per changed core.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// `(core, on)` for every core that changed.
    pub changed: Vec<(String, bool)>,
    pub closed: usize,
    pub written: usize,
    /// `(workspace-relative path, error)` of failed write-backs.
    pub failed: Vec<(String, String)>,
    /// The closed docs' workspace-relative paths (at most 5, for the CLI's
    /// line).
    pub paths: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }

    /// `multiplayerCoresReport` on the settings answer.
    pub fn to_json(&self) -> Value {
        json!({
            "changed": self.changed.iter().map(|(c, on)| json!({ "core": c, "on": on })).collect::<Vec<_>>(),
            "closed": self.closed,
            "written": self.written,
            "failed": self.failed.iter().map(|(p, e)| json!({ "path": p, "error": e })).collect::<Vec<_>>(),
            "paths": self.paths,
        })
    }
}

/// The last change of one core (`GET /cli/docs/cores` `lastChange`; memory
/// only: the audit keeps it for good).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastChange {
    pub on: bool,
    pub by: String,
    pub at: i64,
    pub closed: usize,
    pub written: usize,
    pub failed: Vec<(String, String)>,
}

fn last_changes() -> &'static Mutex<BTreeMap<String, LastChange>> {
    static LAST: OnceLock<Mutex<BTreeMap<String, LastChange>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// One flip at a time (a turn-off's closes finish before the next flip).
fn flip_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// A person's display name for an actor label (`owner-token`, `user:anna`),
/// for the clients' banner. Anything else is shown as is.
fn display_name(by: &str) -> String {
    let people = k2_core::notify_prefs::People::load().unwrap_or_default();
    let key = if by == "owner-token" || by == "owner" {
        k2_core::notify_prefs::OWNER.to_string()
    } else if let Some(user) = by.strip_prefix("user:") {
        people.person_key(user)
    } else {
        return by.to_string();
    };
    k2_core::person_name::effective(&key, &people).display_name
}

/// Bring the process in line with the stored switches `after` (a settings
/// update or reset), as `by` (the actor's label: `owner-token`,
/// `user:anna`). See the module doc for the order. Headless: no client is
/// needed for any step.
pub fn sync_multiplayer_cores(after: &BTreeMap<String, bool>, by: &str) -> Report {
    let rt = super::runtime().ok();
    sync_with(after, by, rt.as_deref().map(|r| r as &dyn CloseSessions), start_runtime)
}

/// The runtime starts on its own thread (the store open waits for a
/// handing-off predecessor; `docs::start` is a no-op when it runs).
/// Unit tests never start the real runtime from a settings write (it is
/// process-wide); the headless scripts prove the real start.
fn start_runtime() {
    #[cfg(not(test))]
    let _ = std::thread::Builder::new().name("k2-docs-open".into()).spawn(super::start);
}

pub(crate) fn sync_with(
    after: &BTreeMap<String, bool>,
    by: &str,
    closer: Option<&dyn CloseSessions>,
    start: fn(),
) -> Report {
    let _flip = flip_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut report = Report::default();
    let by_name = display_name(by);
    for core in CORES {
        let on = docs_cores::is_on(after, core.id);
        if is_on(core.id) == on {
            continue;
        }
        // 1. The mirror: from here on every docs open of this kind is
        //    refused (core_off), or allowed.
        set(core.id, on);
        let (mut closed, mut written, mut failed) = (0usize, 0usize, Vec::new());
        if on {
            if super::runtime().is_err() {
                start();
            }
        } else if let Some(closer) = closer {
            // 2. Close this kind's sessions (the kit's host close): write
            //    back now, end claims and presence, out of the pool. 3. Each
            //    Live socket on them ends with `closed {code: core_off}`,
            //    naming the person and whether its doc is on disk. 4. They
            //    leave the watcher index (`forget`).
            let id = core.id;
            let done = closer.close_where(&|k: &DocKey| core_of_path(&k.path) == id);
            let on_disk: HashMap<DocKey, bool> = done.iter().map(|c| (c.key.clone(), c.error.is_none())).collect();
            super::live::close_core(id, Some(by_name.as_str()), &on_disk);
            for c in done {
                closed += 1;
                if report.paths.len() < MAX_PATHS {
                    report.paths.push(c.key.path.clone());
                }
                if c.written {
                    written += 1;
                }
                if let Some(e) = c.error {
                    super::host::audit(
                        AUDIT_CLOSE_FAILED,
                        by,
                        format!("core={id} path={} error={e}", c.key.path),
                    );
                    failed.push((c.key.path.clone(), e));
                }
            }
        }
        // 4. The audit line, naming the person (MC-9).
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            AUDIT_SWITCH,
            by,
            format!("core={} on={on} closed={closed} written={written} failed={}", core.id, failed.len()),
            "-",
            "-",
            "cli",
        ));
        log_debug!("[docs] the {} is {} (by {by}; closed {closed}, written {written}, failed {})", core.name, if on { "on" } else { "off" }, failed.len());
        last_changes().lock().unwrap_or_else(|p| p.into_inner()).insert(
            core.id.to_string(),
            LastChange { on, by: by.to_string(), at: chrono::Utc::now().timestamp(), closed, written, failed: failed.clone() },
        );
        // 5. Every client (MC-8), then the local windows' settings store.
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::DocCoresChanged {
            cores: snapshot(),
            core: core.id.to_string(),
            on,
            by: by_name.clone(),
        });
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::SyncSettings,
            json!({ "multiplayerCores": snapshot(), "core": core.id, "on": on }),
        );
        report.changed.push((core.id.to_string(), on));
        report.closed += closed;
        report.written += written;
        report.failed.extend(failed);
    }
    report
}

/// At boot: the mirror from `settings.json`, and the docs runtime only if a
/// core is on (MC-6: with every core off, `~/.k2/docs.redb` isn't opened and
/// no docs timer or watcher runs).
pub fn boot() {
    let stored = k2_core::app_settings::load().multiplayer_cores;
    for core in CORES {
        set(core.id, docs_cores::is_on(&stored, core.id));
    }
    if any_on() {
        start_runtime();
    } else {
        log_debug!("[docs] every multiplayer core is off: the docs store stays closed");
    }
}

// ── GET /cli/docs/cores ─────────────────────────────────────────────────

/// Whether this caller may flip a core: the owner token, an Owner or Admin
/// login. Never an agent, and never a shell tab's passport (a person in a K2
/// terminal flips from Settings or a terminal outside K2).
fn can_change(caller: &DocCaller) -> bool {
    match caller {
        DocCaller::Agent { .. } => false,
        DocCaller::Person { via, .. } if *via == super::caller::VIA_TERMINAL => false,
        DocCaller::Person { key, .. } if key == k2_core::notify_prefs::OWNER => true,
        DocCaller::Person { key, .. } => {
            k2_core::connect_users::role_for_user(key).is_some_and(|r| r >= k2_core::connect_users::Role::Admin)
        }
    }
}

/// Whether the caller may see this doc's path in `open.paths`.
fn may_see(caller: &DocCaller, key: &DocKey) -> bool {
    match caller {
        DocCaller::Agent { project_id } => &key.space == project_id,
        DocCaller::Person { key: person, .. } => super::host::person_rights(person).read,
    }
}

/// Per core: `(files open live, participants, the caller's readable paths)`.
fn open_now(caller: &DocCaller) -> BTreeMap<&'static str, (usize, usize, Vec<String>)> {
    let mut out: BTreeMap<&'static str, (usize, usize, Vec<String>)> = BTreeMap::new();
    let Ok(rt) = super::runtime() else { return out };
    for key in rt.manager.loaded_keys() {
        let participants = if let Some(s) = rt.manager.get(&key) {
            s.status().map(|st| st.participants).unwrap_or(0)
        } else if let Some(b) = rt.manager.get_blob(&key) {
            b.status().map(|st| st.participants).unwrap_or(0)
        } else {
            0
        };
        if participants == 0 {
            continue;
        }
        let e = out.entry(core_of_path(&key.path)).or_default();
        e.0 += 1;
        e.1 += participants;
        if may_see(caller, &key) {
            e.2.push(key.path.clone());
        }
    }
    for e in out.values_mut() {
        e.2.sort();
        e.2.truncate(MAX_PATHS);
    }
    out
}

/// `GET /cli/docs/cores`: one row per core, from the daemon (Settings and
/// `k2 doc cores` say the same thing). Read-only; any caller.
pub fn cores_route(caller: &DocCaller) -> CliResponse {
    let open = open_now(caller);
    let last = last_changes().lock().unwrap_or_else(|p| p.into_inner()).clone();
    let rows: Vec<Value> = CORES
        .iter()
        .map(|c| {
            let (files, participants, paths) = open.get(c.id).cloned().unwrap_or_default();
            json!({
                "id": c.id,
                "name": c.name,
                "status": c.status,
                "description": c.description,
                "short": c.short,
                "view": c.view.wire(),
                "on": is_on(c.id),
                "open": { "files": files, "participants": participants, "paths": paths },
                "lastChange": last.get(c.id).map(|l| json!({
                    "on": l.on,
                    "by": l.by,
                    "at": l.at,
                    "closed": l.closed,
                    "written": l.written,
                    "failed": l.failed.iter().map(|(p, e)| json!({ "path": p, "error": e })).collect::<Vec<_>>(),
                })),
            })
        })
        .collect();
    super::routes::ok(json!({
        "canChange": can_change(caller),
        "cores": rows,
        "later": docs_cores::LATER,
    }))
}

/// Whether a core with a multiplayer view for people is on (S5 routes on it).
pub fn any_live_view_on() -> bool {
    CORES.iter().any(|c| c.view == CoreView::Live && is_on(c.id))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Tests that flip the process-wide mirror hold this (with a temp HOME).
    pub(crate) fn mirror_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Every core off again (test cleanup).
    pub(crate) fn all_off() {
        for c in CORES {
            set(c.id, false);
        }
    }

    fn key(space: &str, path: &str) -> DocKey {
        DocKey { space: space.into(), path: path.into() }
    }

    /// The kit's close, faked: loaded keys, and which paths fail to write.
    struct Fake {
        loaded: Vec<DocKey>,
        dirty: Vec<&'static str>,
        failing: Vec<(&'static str, &'static str)>,
        calls: AtomicUsize,
        closed: Mutex<Vec<DocKey>>,
    }

    impl CloseSessions for Fake {
        fn close_where(&self, matches: &dyn Fn(&DocKey) -> bool) -> Vec<Closed> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.loaded
                .iter()
                .filter(|k| matches(k))
                .map(|k| {
                    self.closed.lock().expect("closed").push(k.clone());
                    let error = self.failing.iter().find(|(p, _)| *p == k.path).map(|(_, e)| e.to_string());
                    Closed { key: k.clone(), written: error.is_none() && self.dirty.contains(&k.path.as_str()), error }
                })
                .collect()
        }
    }

    static STARTS: AtomicUsize = AtomicUsize::new(0);
    fn count_start() {
        STARTS.fetch_add(1, Ordering::SeqCst);
    }

    fn map(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn audit_lines(event: &str) -> Vec<Value> {
        k2_core::auth_audit::tail(1000).expect("audit tail").into_iter().filter(|e| e["event"] == event).collect()
    }

    #[test]
    fn a_file_belongs_to_its_core_by_name() {
        assert_eq!(core_of_path("notes.md"), "markdown");
        assert_eq!(core_of_path("a/b.markdown"), "markdown");
        assert_eq!(core_of_path("src/main.rs"), "text");
        assert_eq!(core_of_path("data.csv"), "text");
        assert_eq!(core_of_path("photo.png"), "any-file");
        assert_eq!(core_of_path("deck.pptx"), "any-file");
    }

    #[test]
    fn core_off_is_403_exit_3_with_the_core_and_path_and_never_declined() {
        let r = core_off("markdown", "notes.md");
        assert_eq!(r.status, "403 Forbidden");
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "core_off");
        assert_eq!(v["error"]["core"], "markdown");
        assert_eq!(v["error"]["path"], "notes.md");
        assert_eq!(v["error"]["exit"], 3);
        assert_eq!(
            v["error"]["hint"],
            "notes.md: the Markdown core is off on this server. The owner or an Admin turns it on in Settings → Cores → Multiplayer (k2 doc cores on markdown, people only). Until then, edit the file with ordinary tools."
        );
    }

    /// The gate keys on the file's name: a `.md` file (even one its pack
    /// would refuse and serve at tier 0) needs the Markdown core, never
    /// `any-file`; each kind needs its own core.
    #[test]
    fn the_gate_needs_the_files_own_core() {
        let _m = mirror_lock();
        all_off();
        let t = |rel: &str| Target {
            project_id: "p1".into(),
            root: "/w".into(),
            rel: rel.into(),
            abs: format!("/w/{rel}").into(),
            tier: Tier::of(rel),
        };
        for rel in ["notes.md", "main.rs", "photo.png"] {
            let r = gate(&t(rel)).expect_err("every core off");
            assert!(r.body.contains("\"core_off\""), "{rel}: {}", r.body);
        }
        set("any-file", true);
        assert!(gate(&t("photo.png")).is_ok());
        let r = gate(&t("big.md")).expect_err("a Markdown file needs the Markdown core, not any-file");
        assert!(r.body.contains("\"core\":\"markdown\""), "{}", r.body);
        assert!(gate(&t("main.rs")).is_err(), "text needs the text core");
        set("markdown", true);
        set("text", true);
        for rel in ["notes.md", "main.rs", "photo.png"] {
            assert!(gate(&t(rel)).is_ok(), "{rel}");
        }
        all_off();
    }

    /// Turning Markdown off closes only Markdown-core sessions (a refused
    /// `.md` served at tier 0 included), counts written and failed
    /// write-backs, audits one exact line per core with the actor's label
    /// plus one per failed path, emits `doc_cores_changed` once with the
    /// person's name, and keeps the last change for the read route. The
    /// flip completes despite the failure.
    #[test]
    fn turning_markdown_off_closes_its_sessions_audits_and_emits_once() {
        let _m = mirror_lock();
        crate::test_support::with_temp_home(|| {
            all_off();
            set("markdown", true);
            set("text", true);
            let fake = Fake {
                loaded: vec![key("p1", "notes.md"), key("p1", "plan.md"), key("p1", "big.md"), key("p1", "main.rs"), key("p2", "photo.png")],
                dirty: vec!["notes.md", "big.md"],
                failing: vec![("plan.md", "no space left on the disk")],
                calls: AtomicUsize::new(0),
                closed: Mutex::new(Vec::new()),
            };
            let mut rx = crate::session_events::subscribe();
            let r = sync_with(&map(&[("markdown", false), ("text", true)]), "user:anna", Some(&fake), count_start);
            assert!(!is_on("markdown"), "the mirror is off");
            assert!(is_on("text"), "text is untouched");
            assert_eq!(fake.calls.load(Ordering::SeqCst), 1, "one close for the one core that went off");
            let closed: Vec<String> = fake.closed.lock().expect("closed").iter().map(|k| k.path.clone()).collect();
            assert_eq!(closed, ["notes.md", "plan.md", "big.md"], "only Markdown-core sessions");
            assert_eq!(r.changed, vec![("markdown".to_string(), false)]);
            assert_eq!((r.closed, r.written), (3, 2));
            assert_eq!(r.failed, vec![("plan.md".to_string(), "no space left on the disk".to_string())]);
            assert_eq!(
                r.to_json(),
                json!({"changed": [{"core": "markdown", "on": false}], "closed": 3, "written": 2,
                       "failed": [{"path": "plan.md", "error": "no space left on the disk"}],
                       "paths": ["notes.md", "plan.md", "big.md"]})
            );
            let sw = audit_lines(AUDIT_SWITCH);
            assert_eq!(sw.len(), 1, "{sw:?}");
            assert_eq!(sw[0]["user"], "user:anna");
            assert_eq!(sw[0]["outcome"], "core=markdown on=false closed=3 written=2 failed=1");
            let cf = audit_lines(AUDIT_CLOSE_FAILED);
            assert_eq!(cf.len(), 1, "{cf:?}");
            assert_eq!(cf[0]["outcome"], "core=markdown path=plan.md error=no space left on the disk");
            let mut events = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                let v = serde_json::to_value(&ev).expect("event json");
                if v["kind"] == "doc_cores_changed" {
                    events.push(v);
                }
            }
            assert_eq!(events.len(), 1, "one event per flip: {events:?}");
            assert_eq!(
                events[0],
                json!({"kind": "doc_cores_changed", "cores": {"any-file": false, "markdown": false, "text": true},
                       "core": "markdown", "on": false, "by": "anna"})
            );
            let last = last_changes().lock().expect("last").get("markdown").cloned().expect("last change");
            assert_eq!((last.on, last.by.as_str(), last.closed, last.written), (false, "user:anna", 3, 2));
            assert_eq!(last.failed.len(), 1);
            let page: Value = serde_json::from_str(&cores_route(&crate::docs::caller::owner()).body).expect("json");
            let md = &page["cores"][0];
            assert_eq!(md["lastChange"]["failed"][0]["path"], "plan.md");
            all_off();
        });
    }

    /// Turning a core on closes nothing, starts the runtime when it isn't
    /// running, and an unchanged core is no flip at all (no audit, no event).
    #[test]
    fn turning_on_starts_the_runtime_and_an_unchanged_core_is_no_flip() {
        let _m = mirror_lock();
        crate::test_support::with_temp_home(|| {
            all_off();
            let fake = Fake {
                loaded: vec![key("p1", "notes.md")],
                dirty: vec![],
                failing: vec![],
                calls: AtomicUsize::new(0),
                closed: Mutex::new(Vec::new()),
            };
            let before = STARTS.load(Ordering::SeqCst);
            let r = sync_with(&map(&[("markdown", true)]), "owner-token", Some(&fake), count_start);
            assert!(is_on("markdown"));
            assert_eq!(fake.calls.load(Ordering::SeqCst), 0, "turning on closes nothing");
            assert_eq!(r.changed, vec![("markdown".to_string(), true)]);
            if super::super::runtime().is_err() {
                assert_eq!(STARTS.load(Ordering::SeqCst), before + 1, "the runtime is started (lazy start)");
            }
            let again = sync_with(&map(&[("markdown", true)]), "owner-token", Some(&fake), count_start);
            assert!(again.is_empty(), "no change, no flip");
            assert_eq!(audit_lines(AUDIT_SWITCH).len(), 1, "one audit line, for the one flip");
            assert_eq!(audit_lines(AUDIT_SWITCH)[0]["outcome"], "core=markdown on=true closed=0 written=0 failed=0");
            all_off();
        });
    }

    /// The read route: three rows from the registry, every core off by
    /// default, all-zero `open` without a runtime, and `canChange` only for
    /// the owner token and Owner/Admin logins.
    #[test]
    fn the_read_route_lists_three_cores_and_says_who_can_change_them() {
        let _m = mirror_lock();
        crate::test_support::with_temp_home(|| {
            all_off();
            k2_core::connect_users::add_user("anna", "password1").expect("add anna");
            k2_core::connect_users::set_role("anna", k2_core::connect_users::Role::Admin).expect("admin");
            k2_core::connect_users::add_user("mo", "password1").expect("add mo");
            let page = |c: &DocCaller| -> Value { serde_json::from_str(&cores_route(c).body).expect("json") };
            let owner = page(&crate::docs::caller::owner());
            assert_eq!(owner["ok"], true);
            assert_eq!(owner["canChange"], true);
            let ids: Vec<&str> = owner["cores"].as_array().expect("rows").iter().map(|r| r["id"].as_str().expect("id")).collect();
            assert_eq!(ids, ["markdown", "text", "any-file"]);
            for row in owner["cores"].as_array().expect("rows") {
                assert_eq!(row["on"], false, "{row}");
                assert_eq!(row["status"], "beta");
                assert_eq!(row["open"], json!({"files": 0, "participants": 0, "paths": []}));
            }
            assert_eq!(owner["cores"][2]["view"], "none");
            assert_eq!(owner["later"], json!(["CSV grid", "Word"]));
            assert_eq!(page(&crate::docs::caller::login("anna"))["canChange"], true, "an Admin login");
            assert_eq!(page(&crate::docs::caller::login("mo"))["canChange"], false, "a Member login");
            assert_eq!(page(&DocCaller::Agent { project_id: "p1".into() })["canChange"], false, "an agent");
            let shell = DocCaller::Person { key: k2_core::notify_prefs::OWNER.into(), via: crate::docs::caller::VIA_TERMINAL };
            assert_eq!(page(&shell)["canChange"], false, "a K2 terminal's passport");
            set("text", true);
            assert_eq!(page(&crate::docs::caller::owner())["cores"][1]["on"], true);
            all_off();
        });
    }
}
