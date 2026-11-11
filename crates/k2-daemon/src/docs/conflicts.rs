//! Tier 0's conflicting saves, K2's side (kit v0.14.0).
//!
//! Since kit v0.14.0 an app's save over a version the kit wrote (an agent's
//! whole-file write, a restore, a merge) keeps both: the save is current and
//! the replaced version is kept as a version and as a conflict copy
//! (`DiskCheck::Conflict`, where it used to be `Ingested`). Two hooks are
//! K2's:
//!
//! - **The copy's name** ([`copy_name_for`], `Host::conflict_copy_name`):
//!   when the replaced version was an agent's write, the copy is
//!   `<stem> (<agent>'s version)<ext>` (`notes (Ada's version).md`, or
//!   `notes (agent's version).md` for an agent without a name). Anything
//!   else keeps the kit's name. The kit passes over an unusable name and
//!   never overwrites a file (it adds ` 2`, ` 3`), so the name stays plain.
//! - **Telling the person once** ([`handle`], from `AuditEvent::BlobConflict`,
//!   which the kit emits once per copy): one notice in the file's open tabs
//!   (`doc_conflict_copy` on the session bus) and one line on the
//!   workspace's Thread, both with the same text. The workspace's agent (and
//!   a lease holder in another workspace) also gets an inbox note
//!   (K-T0-3), as before.
//!
//! The kit calls the host with the session locked, so [`notify`] does its
//! work on a thread of its own.

use std::path::{Component, Path, PathBuf};

use k2_mpc_base::store::{DocKey, VersionId};

use super::host::Principal;

/// The name K2 gives an agent participant that has no display name
/// ([`super::caller::DocCaller::participant`]).
pub const UNNAMED_AGENT: &str = "agent";
/// The `via` of K2's Thread line.
pub const THREAD_VIA: &str = "k2-docs";
/// Most characters of a name in a copy's file name (as the kit's default).
const NAME_MAX: usize = 64;

/// `name` made safe inside a file name: no control characters and none of
/// `/\:*?"<>|` (each becomes `_`), at most [`NAME_MAX`] characters, trimmed.
fn plain_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .take(NAME_MAX)
        .collect();
    cleaned.trim().to_string()
}

/// `notes.md` → (`notes`, `.md`); a leading dot is part of the stem.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// `<stem> (<agent>'s version)<ext>`: the copy of an agent's replaced write.
/// An agent without a usable name reads as `agent`.
pub fn agent_copy_name(file_name: &str, agent: &str) -> String {
    let (stem, ext) = split_ext(file_name);
    let who = match plain_name(agent) {
        n if n.is_empty() => UNNAMED_AGENT.to_string(),
        n => n,
    };
    format!("{stem} ({who}'s version){ext}")
}

/// `Host::conflict_copy_name`: `who` is the replaced version's author name
/// (or the lease holder's). An agent's name ([`UNNAMED_AGENT`], or one
/// `is_agent_name` knows) names the copy after the agent; anything else is
/// `None`, the kit's default name.
pub fn copy_name_for(file_name: &str, who: &str, is_agent_name: impl Fn(&str) -> bool) -> Option<String> {
    let who = who.trim();
    if who == UNNAMED_AGENT || (!who.is_empty() && is_agent_name(who)) {
        Some(agent_copy_name(file_name, who))
    } else {
        None
    }
}

/// Whether `name` is an agent's display name on this box. Called with a
/// session locked: the DB lock only to list the workspaces, then the cached
/// display names (a stat each). Runs only when a conflict copy is made.
pub fn is_agent_name(name: &str) -> bool {
    let paths: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::db::schema::Project::list(&conn) {
            Ok(ps) => ps.into_iter().map(|p| p.path).filter(|p| !p.is_empty()).collect(),
            Err(e) => {
                k2_core::log_debug!("[docs] WARN: conflict copy name: listing workspaces failed: {e}");
                return false;
            }
        }
    };
    paths.iter().any(|p| k2_core::workspace::display::agent_display_name(p) == name)
}

/// One `AuditEvent::BlobConflict`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub key: DocKey,
    pub current: VersionId,
    pub kept: VersionId,
    /// The copy, relative to the file's folder (`None`: it couldn't be written).
    pub copy: Option<String>,
    pub lease_holder: Option<String>,
}

/// Who wrote the replaced version (from the store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub author: String,
    pub author_name: String,
}

/// What [`handle`] needs from K2's records.
pub trait Lookup {
    /// The replaced version's author.
    fn replaced(&self, key: &DocKey, kept: VersionId) -> Option<Replaced>;
    /// A workspace's folder (by project id).
    fn workspace_path(&self, project_id: &str) -> Option<String>;
    /// An agent's display name (by project id); `None` when it has none.
    fn agent_name(&self, project_id: &str) -> Option<String>;
    /// A pid as a person reads it (`Ada (person)`).
    fn pid_label(&self, pid: &str) -> String;
}

/// Where [`handle`] tells people.
pub trait Sink {
    /// The file's open tabs.
    fn tab(&self, n: &TabNotice);
    /// One line on the workspace's Thread (`workspace`: its folder).
    fn thread(&self, workspace: &str, line: &str);
    /// An agent's inbox note (`workspace`: its folder).
    fn inbox(&self, workspace: &str, title: &str, body: &str);
}

/// The tab notice (`doc_conflict_copy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabNotice {
    pub project_id: String,
    /// The workspace's folder (a tab matches `workspace` + `rel` to its file).
    pub workspace: String,
    pub rel: String,
    pub current: VersionId,
    pub kept: VersionId,
    /// The copy, relative to the workspace (`None`: not written).
    pub copy: Option<String>,
    pub text: String,
}

/// The copy's path in the workspace: `copy` (relative to the file's folder)
/// joined to `rel`'s folder. `copy` as given if it would leave the workspace.
pub fn copy_in_workspace(rel: &str, copy: &str) -> String {
    let mut out: Vec<String> = Path::new(rel)
        .parent()
        .map(|p| p.components().filter_map(|c| c.as_os_str().to_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    for c in Path::new(copy).components() {
        match c {
            Component::ParentDir => {
                if out.pop().is_none() {
                    return copy.to_string();
                }
            }
            Component::CurDir => {}
            Component::Normal(s) => match s.to_str() {
                Some(s) => out.push(s.to_string()),
                None => return copy.to_string(),
            },
            Component::RootDir | Component::Prefix(_) => return copy.to_string(),
        }
    }
    out.iter().collect::<PathBuf>().to_string_lossy().replace('\\', "/")
}

/// Whose work the save replaced, as the notice says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Whose {
    /// An agent's write (its name, or `None`: "the agent").
    Agent(Option<String>),
    /// Saved while someone held the file's lease.
    Holder(String),
    /// Anything else (a person's version the file didn't have yet).
    Other,
}

/// The notice's text: the same in the tab and on the Thread.
pub fn notice_text(rel: &str, whose: &Whose, copy: Option<&str>, kept: VersionId) -> String {
    let what = match whose {
        Whose::Agent(Some(name)) => format!("Someone saved over {name}'s change to {rel}."),
        Whose::Agent(None) => format!("Someone saved over the agent's change to {rel}."),
        Whose::Holder(h) => format!("Someone saved {rel} while {h} held its lock."),
        Whose::Other => format!("Someone saved over a newer version of {rel}."),
    };
    match copy {
        Some(c) => format!("{what} Both are kept: {c}"),
        None => format!("{what} Both are kept: the replaced one is version {kept}."),
    }
}

/// Who the replaced version was.
pub fn whose(c: &Conflict, replaced: Option<&Replaced>, lookup: &dyn Lookup) -> Whose {
    if let Some(Principal::Agent(id)) = replaced.and_then(|r| Principal::parse(&r.author)) {
        return Whose::Agent(lookup.agent_name(&id).filter(|n| n != UNNAMED_AGENT && !n.trim().is_empty()));
    }
    match &c.lease_holder {
        Some(h) => Whose::Holder(lookup.pid_label(h)),
        None => Whose::Other,
    }
}

/// The agent's inbox note (K-T0-3): file names and version numbers only.
pub fn inbox_note(c: &Conflict, holder_label: Option<&str>, copy: Option<&str>) -> (String, String) {
    let title = format!("Conflicting save: {}", c.key.path);
    let mut body = format!(
        "{} was saved by another program {}. Nothing was lost:\n\n- the save is current: version {}\n- what it replaced is version {}",
        c.key.path,
        match holder_label {
            Some(h) => format!("while {h} held its lease"),
            None => "over a version it hadn't seen (a write through K2, a restore or a merge)".to_string(),
        },
        c.current,
        c.kept,
    );
    if let Some(copy) = copy {
        body.push_str(&format!(", also kept as the copy \"{copy}\" in the workspace"));
    }
    body.push_str(&format!(
        "\n\nk2 doc versions {p} lists them; k2 doc restore --version {kept} {p} makes the replaced one current again.",
        p = c.key.path,
        kept = c.kept,
    ));
    (title, body)
}

/// Tell everyone about one `BlobConflict`, once: one tab notice and one
/// Thread line for the person, and the agents' inbox notes.
pub fn handle(c: &Conflict, lookup: &dyn Lookup, sink: &dyn Sink) {
    let replaced = lookup.replaced(&c.key, c.kept);
    let copy = c.copy.as_deref().map(|cp| copy_in_workspace(&c.key.path, cp));
    let text = notice_text(&c.key.path, &whose(c, replaced.as_ref(), lookup), copy.as_deref(), c.kept);
    let Some(workspace) = lookup.workspace_path(&c.key.space) else {
        k2_core::log_debug!("[docs] WARN: conflict on {}: its workspace is gone", super::host::key_label(&c.key));
        return;
    };
    sink.tab(&TabNotice {
        project_id: c.key.space.clone(),
        workspace: workspace.clone(),
        rel: c.key.path.clone(),
        current: c.current,
        kept: c.kept,
        copy: copy.clone(),
        text: text.clone(),
    });
    sink.thread(&workspace, &text);
    let holder_label = c.lease_holder.as_deref().map(|h| lookup.pid_label(h));
    let (title, body) = inbox_note(c, holder_label.as_deref(), copy.as_deref());
    sink.inbox(&workspace, &title, &body);
    if let Some(Principal::Agent(id)) = c.lease_holder.as_deref().and_then(Principal::parse) {
        if id != c.key.space {
            if let Some(other) = lookup.workspace_path(&id) {
                sink.inbox(&other, &title, &body);
            }
        }
    }
}

/// K2's records, for [`handle`].
pub struct K2Lookup;

impl Lookup for K2Lookup {
    fn replaced(&self, key: &DocKey, kept: VersionId) -> Option<Replaced> {
        let store = super::runtime().ok()?.store()?;
        let id = store.find(key).ok()??;
        let v = store.blob_version(id, kept).ok()?;
        Some(Replaced { author: v.author, author_name: v.author_name })
    }

    fn workspace_path(&self, project_id: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, project_id).ok().map(|p| p.path)
    }

    fn agent_name(&self, project_id: &str) -> Option<String> {
        let name = super::caller::DocCaller::Agent { project_id: project_id.to_string() }.participant(None).name;
        (name != UNNAMED_AGENT).then_some(name)
    }

    fn pid_label(&self, pid: &str) -> String {
        super::routes::pid_label(pid)
    }
}

/// The daemon's sinks: the session bus, the Thread and the inbox.
pub struct K2Sink;

impl Sink for K2Sink {
    fn tab(&self, n: &TabNotice) {
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::DocConflictCopy {
            project_id: n.project_id.clone(),
            workspace: n.workspace.clone(),
            rel: n.rel.clone(),
            current: n.current,
            kept: n.kept,
            copy: n.copy.clone(),
            text: n.text.clone(),
        });
    }

    fn thread(&self, workspace: &str, line: &str) {
        if let Err(e) = crate::overlay_routes::post_workspace_note(workspace, line, THREAD_VIA) {
            k2_core::log_debug!("[docs] WARN: conflict line on the Thread of {workspace}: {e}");
        }
    }

    fn inbox(&self, workspace: &str, title: &str, body: &str) {
        if let Err(e) = k2_core::inbox::compose(Path::new(workspace), title, body, Some("high"), Some("docs"), Some("k2 doc")) {
            k2_core::log_debug!("[docs] WARN: conflict note in {workspace}: {e}");
        }
    }
}

/// From `Host::on_event` (session locked): [`handle`] on a thread of its own.
pub fn notify(c: Conflict) {
    let _ = std::thread::Builder::new().name("k2-docs-conflict".into()).spawn(move || handle(&c, &K2Lookup, &K2Sink));
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use k2_mpc_base::blob::{BlobConfig, BlobStore};
    use k2_mpc_base::session::{
        AnyFile, AuditEvent, Config, DiskCheck, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    };
    use k2_mpc_base::store::DocStore;

    #[test]
    fn an_agents_write_is_named_after_the_agent() {
        let agents = |n: &str| n == "Ada";
        assert_eq!(copy_name_for("notes.md", "Ada", agents).as_deref(), Some("notes (Ada's version).md"));
        assert_eq!(copy_name_for("poster.psd", "Ada", agents).as_deref(), Some("poster (Ada's version).psd"));
        assert_eq!(copy_name_for("Makefile", "Ada", agents).as_deref(), Some("Makefile (Ada's version)"));
        assert_eq!(copy_name_for(".env", "Ada", agents).as_deref(), Some(".env (Ada's version)"));
        assert_eq!(copy_name_for("a.tar.gz", " Ada ", agents).as_deref(), Some("a.tar (Ada's version).gz"));
    }

    #[test]
    fn an_unnamed_agent_reads_as_agent() {
        assert_eq!(copy_name_for("notes.md", UNNAMED_AGENT, |_| false).as_deref(), Some("notes (agent's version).md"));
        assert_eq!(agent_copy_name("notes.md", "  "), "notes (agent's version).md");
    }

    #[test]
    fn a_person_or_the_kit_keeps_the_kits_name() {
        let agents = |n: &str| n == "Ada";
        assert_eq!(copy_name_for("notes.md", "Rosa", agents), None, "a person's version");
        assert_eq!(copy_name_for("notes.md", "merge", agents), None, "the kit's merge");
        assert_eq!(copy_name_for("notes.md", "", |_| true), None, "no name never asks");
    }

    #[test]
    fn a_name_stays_plain_in_a_file_name() {
        let n = agent_copy_name("notes.md", "R2/D2: \"the\\bot\"\n");
        assert_eq!(n, "notes (R2_D2_ _the_bot__'s version).md", "the closing quote and the newline are each `_`");
        assert!(!n.contains(['/', '\\']), "{n}");
        let long = "x".repeat(200);
        let n = agent_copy_name("a.md", &long);
        assert_eq!(n, format!("a ({}'s version).md", "x".repeat(NAME_MAX)));
    }

    #[test]
    fn a_copy_resolves_inside_the_workspace() {
        assert_eq!(copy_in_workspace("notes.md", "notes (Ada's version).md"), "notes (Ada's version).md");
        assert_eq!(copy_in_workspace("docs/a/notes.md", "../../.k2/conflicts/x.md"), ".k2/conflicts/x.md");
        assert_eq!(copy_in_workspace("notes.md", "../outside.md"), "../outside.md", "never past the workspace");
    }

    #[test]
    fn the_notice_says_whose_change_and_where_the_copy_is() {
        let c = Some(".k2/conflicts/notes (Ada's version).md");
        assert_eq!(
            notice_text("notes.md", &Whose::Agent(Some("Ada".into())), c, 3),
            "Someone saved over Ada's change to notes.md. Both are kept: .k2/conflicts/notes (Ada's version).md"
        );
        assert_eq!(
            notice_text("notes.md", &Whose::Agent(None), Some("x.md"), 3),
            "Someone saved over the agent's change to notes.md. Both are kept: x.md"
        );
        assert_eq!(
            notice_text("a.png", &Whose::Holder("Rosa (person)".into()), None, 7),
            "Someone saved a.png while Rosa (person) held its lock. Both are kept: the replaced one is version 7."
        );
        assert_eq!(notice_text("a.png", &Whose::Other, Some("b.png"), 2), "Someone saved over a newer version of a.png. Both are kept: b.png");
    }

    // ---- one BlobConflict, end to end through the vendored kit ----

    /// Tests' records: the replaced author from the real test store.
    struct FakeLookup {
        store: Arc<DocStore>,
        paths: HashMap<String, String>,
        agents: HashMap<String, String>,
    }

    impl Lookup for FakeLookup {
        fn replaced(&self, key: &DocKey, kept: VersionId) -> Option<Replaced> {
            let id = self.store.find(key).expect("find")?;
            let v = self.store.blob_version(id, kept).expect("the kept version");
            Some(Replaced { author: v.author, author_name: v.author_name })
        }
        fn workspace_path(&self, project_id: &str) -> Option<String> {
            self.paths.get(project_id).cloned()
        }
        fn agent_name(&self, project_id: &str) -> Option<String> {
            self.agents.get(project_id).cloned()
        }
        fn pid_label(&self, pid: &str) -> String {
            pid.to_string()
        }
    }

    #[derive(Default)]
    struct Recorded {
        tabs: Mutex<Vec<TabNotice>>,
        threads: Mutex<Vec<(String, String)>>,
        inboxes: Mutex<Vec<(String, String)>>,
    }

    impl Sink for Recorded {
        fn tab(&self, n: &TabNotice) {
            self.tabs.lock().expect("tabs").push(n.clone());
        }
        fn thread(&self, workspace: &str, line: &str) {
            self.threads.lock().expect("threads").push((workspace.to_string(), line.to_string()));
        }
        fn inbox(&self, workspace: &str, title: &str, _body: &str) {
            self.inboxes.lock().expect("inboxes").push((workspace.to_string(), title.to_string()));
        }
    }

    /// The kit's host in the test: every right, K2's copy-name hook with a
    /// known agent name, and the `BlobConflict`s it is told about.
    struct TestHost {
        conflicts: Mutex<Vec<Conflict>>,
        now: AtomicU64,
    }

    impl Host for TestHost {
        fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
            Rights::full()
        }
        fn now_ms(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }
        fn conflict_copy_name(&self, file_name: &str, _at_ms: u64, who: &str) -> Option<String> {
            copy_name_for(file_name, who, |n| n == "Ada")
        }
        fn on_event(&self, ev: &AuditEvent) {
            if let AuditEvent::BlobConflict { key, current, kept, copy, lease_holder } = ev {
                self.conflicts.lock().expect("conflicts").push(Conflict {
                    key: key.clone(),
                    current: *current,
                    kept: *kept,
                    copy: copy.clone(),
                    lease_holder: lease_holder.clone(),
                });
            }
        }
    }

    #[test]
    fn an_app_save_over_an_agents_write_keeps_both_and_tells_the_person_once() {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("k2-conflicts-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).expect("workspace");
        let ws = std::fs::canonicalize(&ws).expect("canonical workspace");
        let host = Arc::new(TestHost { conflicts: Mutex::new(Vec::new()), now: AtomicU64::new(1_800_000_000_000) });
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
        let h = host.clone();
        let blobs = Arc::new(
            BlobStore::open(store.clone(), dir.join("blobs"), BlobConfig::default(), Arc::new(move || h.now_ms())).expect("blobs"),
        );
        let mut config = Config::default();
        config.edit_rate_per_s = 1_000_000;
        config.edit_burst = 1_000_000;
        let mgr = SessionManager::new(store.clone(), host.clone(), config).with_blob_store(blobs);
        let key = DocKey { space: "proj-1".into(), path: "notes.md".into() };
        let path = ws.join("notes.md");
        std::fs::write(&path, b"# notes\n").expect("the file");
        let s = mgr.open_blob_file(Arc::new(AnyFile), &key, &path, &ws).expect("open the file");
        let agent = Participant::new("agent:proj-1", "Ada", "#654321", ParticipantKind::Agent, "cli");
        let g = s.join(agent, Stay::Held).expect("join").conn;
        let w = s.write_bytes(g, b"# notes\nthe agent's line\n", 1).expect("the agent's write");
        assert!(w.written, "the agent's write is on disk");

        // An app that opened the file before the agent's write saves over it.
        host.now.fetch_add(1_000, Ordering::SeqCst);
        let tmp = ws.join(".app-save.tmp");
        std::fs::write(&tmp, b"# notes\nthe person's line\n").expect("the app's save");
        std::fs::rename(&tmp, &path).expect("the app's rename");
        let out = s.file_changed().expect("file_changed");
        let DiskCheck::Conflict { kept, copy: Some(copy), .. } = out else {
            panic!("kit v0.14.0: a save over an agent's write is a conflict with a copy, got {out:?}");
        };
        assert_eq!(kept, w.version, "the agent's write is the kept version");
        assert_eq!(copy.file_name().and_then(|n| n.to_str()), Some("notes (Ada's version).md"), "K2's name");
        assert_eq!(std::fs::read(&copy).expect("the copy"), b"# notes\nthe agent's line\n");
        assert_eq!(std::fs::read(&path).expect("the file"), b"# notes\nthe person's line\n", "the app's save stays");

        let conflicts = host.conflicts.lock().expect("conflicts").clone();
        assert_eq!(conflicts.len(), 1, "the kit emits one BlobConflict per copy: {conflicts:?}");

        let lookup = FakeLookup {
            store: store.clone(),
            paths: HashMap::from([("proj-1".to_string(), ws.display().to_string())]),
            agents: HashMap::from([("proj-1".to_string(), "Ada".to_string())]),
        };
        let sink = Recorded::default();
        for c in &conflicts {
            handle(c, &lookup, &sink);
        }
        let tabs = sink.tabs.lock().expect("tabs").clone();
        assert_eq!(tabs.len(), 1, "exactly one tab notice: {tabs:?}");
        let text = "Someone saved over Ada's change to notes.md. Both are kept: notes (Ada's version).md";
        assert_eq!(tabs[0].text, text);
        assert_eq!(tabs[0].rel, "notes.md");
        assert_eq!(tabs[0].workspace, ws.display().to_string());
        assert_eq!(tabs[0].copy.as_deref(), Some("notes (Ada's version).md"));
        assert_eq!(tabs[0].kept, kept);
        let threads = sink.threads.lock().expect("threads").clone();
        assert_eq!(threads, vec![(ws.display().to_string(), text.to_string())], "exactly one Thread line");
        let inboxes = sink.inboxes.lock().expect("inboxes").clone();
        assert_eq!(inboxes.len(), 1, "one inbox note for the workspace's agent: {inboxes:?}");

        drop(s);
        drop(mgr);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
