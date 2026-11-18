//! Conflicting saves, K2's side: tier 0 (kit v0.14.0) and live keyed docs
//! that aren't tables, a deck or a board (kit v0.15.0).
//!
//! Since kit v0.14.0 an app's save over a version the kit wrote (an agent's
//! whole-file write, a restore, a merge) keeps both: the save is current and
//! the replaced version is kept as a version and as a conflict copy
//! (`DiskCheck::Conflict`, where it used to be `Ingested`). Since v0.15.0 a
//! live keyed doc does the same when the kit wrote the file since the last
//! outside save: one `BlobConflict` (its ids are the live doc's version ids,
//! `lease_holder` `None`). Two hooks are K2's:
//!
//! - **The copy's name** ([`copy_name`], `Host::conflict_copy_name`):
//!   when the replaced version was an agent's write, the copy is
//!   `<stem> (<agent>'s version)<ext>` (`notes (Ada's version).md`, or
//!   `notes (agent's version).md` for an agent without a name). The kit
//!   names the writer by display name, or (a live doc's writer that left)
//!   by pid: K2 reads a pid as the agent or person it is. Anything else
//!   keeps the kit's name. The kit passes over an unusable name and
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

use k2_mpc_base::session::RESERVED_PID_PREFIX;
use k2_mpc_base::store::{DocKey, DocStore, VersionId, BLOB_DOC_PACK};

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

/// The names [`copy_name`] needs from K2's records.
pub trait Names {
    /// Whether `name` is an agent's display name on this box.
    fn is_agent_name(&self, name: &str) -> bool;
    /// An agent's display name (by project id); `None` when it has none.
    fn agent_name(&self, project_id: &str) -> Option<String>;
    /// A person's display name (by person key).
    fn person_name(&self, person_key: &str) -> String;
}

/// `Host::conflict_copy_name`. `who` is the replaced version's writer as the
/// kit names it: a display name ([`copy_name_for`]), or a pid when the kit
/// has no name for it (a live keyed doc's writer that left the doc). An
/// agent's pid names the copy after the agent; a person's pid keeps the
/// kit's name with the person's name in it, never the pid.
pub fn copy_name(file_name: &str, at_ms: u64, who: &str, names: &dyn Names) -> Option<String> {
    match Principal::parse(who.trim()) {
        Some(Principal::Agent(id)) => {
            Some(agent_copy_name(file_name, &names.agent_name(&id).unwrap_or_else(|| UNNAMED_AGENT.to_string())))
        }
        Some(Principal::Person(key)) => {
            Some(k2_mpc_base::session::conflict_copy_name(file_name, at_ms, &names.person_name(&key)))
        }
        None => copy_name_for(file_name, who, |n| names.is_agent_name(n)),
    }
}

/// K2's names, for [`copy_name`].
pub struct K2Names;

impl Names for K2Names {
    fn is_agent_name(&self, name: &str) -> bool {
        is_agent_name(name)
    }

    fn agent_name(&self, project_id: &str) -> Option<String> {
        K2Lookup.agent_name(project_id)
    }

    fn person_name(&self, person_key: &str) -> String {
        let people = k2_core::notify_prefs::People::load().unwrap_or_default();
        k2_core::person_name::effective(person_key, &people).display_name
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

/// The writer of a live doc's version: the author of the change it was cut
/// at, or else the last participant in its authors (since the version
/// before), never one of the kit's own `kit:` pids (an outside save, a
/// merge, the import).
fn live_writer(at_change: Option<&str>, authors: &[String]) -> Option<String> {
    let person_or_agent = |a: &&str| !a.is_empty() && !a.starts_with(RESERVED_PID_PREFIX);
    at_change
        .filter(person_or_agent)
        .or_else(|| authors.iter().map(String::as_str).rev().find(person_or_agent))
        .map(str::to_owned)
}

/// The replaced version's writer, from the store: a tier-0 version records
/// its writer; a live doc's version (a keyed doc, kit v0.15.0) records who
/// changed it ([`live_writer`]).
pub fn replaced_in(store: &DocStore, key: &DocKey, kept: VersionId) -> Option<Replaced> {
    let id = store.find(key).ok()??;
    if store.meta(id).ok()?.pack == BLOB_DOC_PACK {
        let v = store.blob_version(id, kept).ok()?;
        return Some(Replaced { author: v.author, author_name: v.author_name });
    }
    let v = store.version(id, kept).ok()?;
    let at = store.change(id, v.seq).ok().flatten().map(|c| c.author);
    let author = live_writer(at.as_deref(), &v.authors)?;
    Some(Replaced { author_name: author.clone(), author })
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
        replaced_in(&store, key, kept)
    }

    fn workspace_path(&self, project_id: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, project_id).ok().map(|p| p.path)
    }

    fn agent_name(&self, project_id: &str) -> Option<String> {
        let name = super::caller::DocCaller::Agent { project_id: project_id.to_string(), session: None }.participant(None).name;
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
        if let Err(e) = crate::overlay_routes::post_workspace_note(workspace, line, THREAD_VIA, k2_core::overlay::SystemNotice::info("doc.conflict")) {
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

    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use k2_mpc_base::blob::{BlobConfig, BlobStore};
    use k2_mpc_base::doc::{Edit, Neutral};
    use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
    use k2_mpc_base::schema::{Kind, Schema};
    use k2_mpc_base::session::{
        AnyFile, AuditEvent, Config, Declined, DiskCheck, EditOptions, Host, LimitSource, Pack, PackContent, PackError,
        Participant, ParticipantKind, Reconciled, Rights, SessionError, SessionManager, Stay, View,
    };

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

    /// The names the tests know: agent `proj-1` is Ada, person `rosa` is Rosa.
    struct FakeNames;

    impl Names for FakeNames {
        fn is_agent_name(&self, name: &str) -> bool {
            name == "Ada"
        }
        fn agent_name(&self, project_id: &str) -> Option<String> {
            (project_id == "proj-1").then(|| "Ada".to_string())
        }
        fn person_name(&self, person_key: &str) -> String {
            match person_key {
                "rosa" => "Rosa".to_string(),
                other => other.to_string(),
            }
        }
    }

    /// Kit v0.15.0: a live keyed doc's writer that left is named by pid. K2
    /// reads an agent's pid as the agent, and a person's as the person's
    /// name in the kit's own name, never the pid.
    #[test]
    fn a_writers_pid_names_the_copy_as_its_name_does() {
        const AT: u64 = 1_793_111_400_000; // 2026-10-27 14:30:00 UTC
        assert_eq!(copy_name("talk.deck", AT, "agent:proj-1", &FakeNames).as_deref(), Some("talk (Ada's version).deck"));
        assert_eq!(
            copy_name("talk.deck", AT, "agent:proj-9", &FakeNames).as_deref(),
            Some("talk (agent's version).deck"),
            "an agent without a name"
        );
        assert_eq!(
            copy_name("talk.deck", AT, "person:rosa", &FakeNames).as_deref(),
            Some("talk (conflict Rosa 2026-10-27 143000).deck"),
            "the kit's name with the person's name"
        );
        assert_eq!(copy_name("talk.deck", AT, "Ada", &FakeNames).as_deref(), Some("talk (Ada's version).deck"), "a name, as before");
        assert_eq!(copy_name("talk.deck", AT, "Rosa", &FakeNames), None, "a person's name keeps the kit's name");
        assert_eq!(copy_name("talk.deck", AT, "kit:external", &FakeNames), None, "the kit's own pid");
    }

    #[test]
    fn a_live_versions_writer_is_its_last_change_or_last_author_never_the_kit() {
        let authors = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(live_writer(Some("agent:p"), &authors(&["person:r"])).as_deref(), Some("agent:p"), "the change it was cut at");
        assert_eq!(
            live_writer(Some("kit:external"), &authors(&["agent:p", "person:r", "kit:external"])).as_deref(),
            Some("person:r"),
            "a kit change: the last author who isn't the kit"
        );
        assert_eq!(live_writer(None, &authors(&["agent:p"])).as_deref(), Some("agent:p"));
        assert_eq!(live_writer(None, &authors(&["kit:import"])), None);
        assert_eq!(live_writer(Some(""), &[]), None);
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
            assert!(self.store.find(key).expect("find").is_some(), "the doc is stored");
            replaced_in(&self.store, key, kept)
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
        fn conflict_copy_name(&self, file_name: &str, at_ms: u64, who: &str) -> Option<String> {
            copy_name(file_name, at_ms, who, &FakeNames)
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

    // ---- a live keyed doc that isn't a table (kit v0.15.0) ----

    /// A deck-shaped keyed test pack: a MovableList `shapes` and a Map
    /// `fields` keyed `<shape>:<field>`. The file is one line per shape: its
    /// key, then `\t<field>=<value>` per field. `reconcile` pairs shapes by
    /// key and fields by map key.
    struct Deck;

    const SHAPES: &str = "shapes";
    const FIELDS: &str = "fields";
    const DECK: Schema = Schema { name: "deck-test", roots: &[(SHAPES, Kind::MovableList), (FIELDS, Kind::Map)] };

    type Parsed = (Vec<(Key, Value)>, BTreeMap<String, Value>);

    fn parse(b: &[u8]) -> Result<Parsed, Declined> {
        let bad = |message: String| Declined { code: "malformed", message };
        let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
        let mut shapes = Vec::new();
        let mut fields = BTreeMap::new();
        for line in s.lines() {
            let mut parts = line.split('\t');
            let k = parts.next().unwrap_or("");
            let key = Key::new(k).map_err(|e| bad(e.to_string()))?;
            for p in parts {
                let (f, v) = p.split_once('=').ok_or_else(|| bad(format!("no = in {p:?}")))?;
                fields.insert(format!("{k}:{f}"), Value::str(v));
            }
            shapes.push((key, Value::Null));
        }
        Ok((shapes, fields))
    }

    impl Pack for Deck {
        fn schema(&self) -> Schema {
            DECK
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            let (shapes, fields) = parse(b)?;
            Ok(PackContent {
                roots: vec![(SHAPES.into(), Neutral::List(shapes)), (FIELDS.into(), Neutral::Map(fields))],
                codec: vec![1],
            })
        }
        fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
            let mut shapes: &[(Key, Value)] = &[];
            let empty = BTreeMap::new();
            let mut fields = &empty;
            for (r, n) in c {
                match n {
                    Neutral::List(l) if r == SHAPES => shapes = l,
                    Neutral::Map(m) if r == FIELDS => fields = m,
                    _ => return Err(PackError(format!("unexpected root {r}"))),
                }
            }
            let mut out = String::new();
            for (k, _) in shapes {
                out.push_str(k.as_str());
                let prefix = format!("{}:", k.as_str());
                for (fk, v) in fields.range(prefix.clone()..) {
                    let Some(f) = fk.strip_prefix(&prefix) else { break };
                    out.push_str(&format!("\t{f}={}", v.as_str().unwrap_or("")));
                }
                out.push('\n');
            }
            Ok(out.into_bytes())
        }
        fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
            let (shapes, fields) = parse(bytes)?;
            let old: &[(Key, Value)] = synced.list(SHAPES).unwrap_or(&[]);
            let hidden = synced.hidden(SHAPES);
            let empty = BTreeMap::new();
            let old_fields = synced.map(FIELDS).unwrap_or(&empty);
            let old_keys: HashSet<&Key> = old.iter().map(|(k, _)| k).collect();
            let new_keys: HashSet<&Key> = shapes.iter().map(|(k, _)| k).collect();
            let mut list: Vec<ListOp> =
                old.iter().filter(|(k, _)| !new_keys.contains(k)).map(|(k, _)| ListOp::Delete { key: k.clone() }).collect();
            let mut prev: Option<Key> = None;
            for (k, _) in &shapes {
                if !old_keys.contains(k) {
                    list.push(match hidden.is_some_and(|h| h.contains_key(k)) {
                        true => ListOp::Restore { key: k.clone() },
                        false => ListOp::Insert {
                            key: k.clone(),
                            at: prev.clone().map_or(Anchor::Start, Anchor::After),
                            value: Value::Null,
                        },
                    });
                }
                prev = Some(k.clone());
            }
            let mut map: Vec<MapOp> = fields
                .iter()
                .filter(|(k, v)| old_fields.get(*k) != Some(*v))
                .map(|(k, v)| MapOp::Set { key: k.clone(), value: v.clone() })
                .collect();
            map.extend(old_fields.keys().filter(|k| !fields.contains_key(*k)).map(|k| MapOp::Delete { key: k.clone() }));
            let mut edits = Vec::new();
            if !list.is_empty() {
                edits.push(Edit::List { root: SHAPES.into(), ops: list });
            }
            if !map.is_empty() {
                edits.push(Edit::Map { root: FIELDS.into(), ops: map });
            }
            Ok(Reconciled { edits, codec: vec![1], ..Reconciled::default() })
        }
    }

    struct KeyedFx {
        dir: PathBuf,
        ws: PathBuf,
        host: Arc<TestHost>,
        store: Arc<DocStore>,
        mgr: SessionManager,
    }

    impl Drop for KeyedFx {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn keyed_fx(tag: &str, keyed_max_entries: Option<usize>) -> KeyedFx {
        keyed_fx_with(tag, |config| {
            if let Some(n) = keyed_max_entries {
                config.keyed_max_entries = n;
            }
        })
    }

    fn keyed_fx_with(tag: &str, set: impl FnOnce(&mut Config)) -> KeyedFx {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2-conflicts-keyed-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws")).expect("workspace");
        let ws = std::fs::canonicalize(dir.join("ws")).expect("canonical workspace");
        let host = Arc::new(TestHost { conflicts: Mutex::new(Vec::new()), now: AtomicU64::new(1_793_111_400_000) });
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
        let h = host.clone();
        let blobs = Arc::new(
            BlobStore::open(store.clone(), dir.join("blobs"), BlobConfig::default(), Arc::new(move || h.now_ms())).expect("blobs"),
        );
        let mut config = Config::default();
        config.edit_rate_per_s = 1_000_000;
        config.edit_burst = 1_000_000;
        set(&mut config);
        let mgr = SessionManager::new(store.clone(), host.clone(), config).with_blob_store(blobs);
        KeyedFx { dir, ws, host, store, mgr }
    }

    fn deck_key() -> DocKey {
        DocKey { space: "proj-1".into(), path: "talk.deck".into() }
    }

    /// Kit v0.15.0: an app's save over the kit's write of a live keyed doc
    /// keeps both, with one `BlobConflict` whose ids are the live doc's
    /// versions. K2 names the copy after the agent (the kit gives its pid:
    /// it left the doc) and tells the person once: one tab notice and one
    /// Thread line, as for tier 0.
    #[test]
    fn an_app_save_over_an_agents_write_to_a_live_keyed_doc_keeps_both_and_tells_the_person_once() {
        let f = keyed_fx("save", None);
        let path = f.ws.join("talk.deck");
        std::fs::write(&path, "s1\ttitle=A\ns2\ttitle=B\n").expect("the file");
        let s = f.mgr.open_file(Arc::new(Deck), &deck_key(), &path, &f.ws).expect("open live");
        let agent = Participant::new("agent:proj-1", "Ada", "#654321", ParticipantKind::Agent, "cli");
        let bot = s.join(agent, Stay::Held).expect("join").conn;
        let mut opts = EditOptions::new();
        opts.base = Some(s.read(bot).expect("read").version);
        let ops = vec![MapOp::Set { key: "s1:title".into(), value: Value::str("Agent") }];
        s.edit_ops(bot, vec![Edit::Map { root: FIELDS.into(), ops }], &opts).expect("the agent's change");
        assert!(s.flush(bot).expect("flush").written, "the kit wrote the file");
        assert_eq!(std::fs::read_to_string(&path).expect("the file"), "s1\ttitle=Agent\ns2\ttitle=B\n");
        s.leave(bot).expect("the agent leaves");

        // An app that opened the file before the agent's write saves over it.
        f.host.now.fetch_add(1_000, Ordering::SeqCst);
        let tmp = f.ws.join(".app-save.tmp");
        std::fs::write(&tmp, "s1\ttitle=A\ns2\ttitle=Person\n").expect("the app's save");
        std::fs::rename(&tmp, &path).expect("the app's rename");
        let out = s.file_changed().expect("file_changed");
        let DiskCheck::Conflict { kept, copy: Some(copy), .. } = out else {
            panic!("kit v0.15.0: a save over the kit's write of a keyed doc keeps both, got {out:?}");
        };
        assert_eq!(copy.file_name().and_then(|n| n.to_str()), Some("talk (Ada's version).deck"), "K2's name, from the pid");
        assert_eq!(std::fs::read_to_string(&copy).expect("the copy"), "s1\ttitle=Agent\ns2\ttitle=B\n");
        assert_eq!(std::fs::read_to_string(&path).expect("the file"), "s1\ttitle=A\ns2\ttitle=Person\n", "the save stays");

        let conflicts = f.host.conflicts.lock().expect("conflicts").clone();
        assert_eq!(conflicts.len(), 1, "one BlobConflict for the save: {conflicts:?}");
        assert_eq!(conflicts[0].kept, kept);
        assert_eq!(conflicts[0].lease_holder, None);
        let replaced = replaced_in(&f.store, &deck_key(), kept).expect("the kept version's writer");
        assert_eq!(replaced.author, "agent:proj-1", "the live version's writer: {replaced:?}");

        let lookup = FakeLookup {
            store: f.store.clone(),
            paths: HashMap::from([("proj-1".to_string(), f.ws.display().to_string())]),
            agents: HashMap::from([("proj-1".to_string(), "Ada".to_string())]),
        };
        let sink = Recorded::default();
        for c in &conflicts {
            handle(c, &lookup, &sink);
        }
        let tabs = sink.tabs.lock().expect("tabs").clone();
        assert_eq!(tabs.len(), 1, "exactly one tab notice: {tabs:?}");
        let text = "Someone saved over Ada's change to talk.deck. Both are kept: talk (Ada's version).deck";
        assert_eq!(tabs[0].text, text);
        assert_eq!(tabs[0].rel, "talk.deck");
        assert_eq!(tabs[0].kept, kept);
        let threads = sink.threads.lock().expect("threads").clone();
        assert_eq!(threads, vec![(f.ws.display().to_string(), text.to_string())], "exactly one Thread line");
        assert_eq!(sink.inboxes.lock().expect("inboxes").len(), 1, "one inbox note for the workspace's agent");
    }

    /// Kit v0.15.0 (MPC-E415): a keyed file over the live size limit is
    /// declined `live_too_large`, which K2 opens at tier 0 as any declined
    /// file (the `Declined` arm of `routes::open` and of export); its answer
    /// names the reason, exit 4.
    #[test]
    fn a_keyed_file_over_the_live_limit_is_declined_and_opens_at_tier_0() {
        let f = keyed_fx("big", Some(4));
        let path = f.ws.join("talk.deck");
        std::fs::write(&path, "s1\ns2\ns3\ns4\ns5\n").expect("the file");
        let e = match f.mgr.open_file(Arc::new(Deck), &deck_key(), &path, &f.ws) {
            Ok(_) => panic!("a file over the limit doesn't open live"),
            Err(e) => e,
        };
        let SessionError::Declined(d) = &e else { panic!("declined, got {e:?}") };
        assert_eq!(d.code, "live_too_large");
        let r = super::super::routes::session_error(&e);
        let body: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(body["error"]["reason"], "live_too_large", "{body}");
        assert_eq!(body["error"]["exit"], 4, "{body}");
        let blob = f.mgr.open_blob_file(Arc::new(AnyFile), &deck_key(), &path, &f.ws).expect("it opens at tier 0");
        blob.status().expect("the tier-0 doc answers");
    }

    /// Grow a live two-shape deck by one field under `f`'s limit of 2: the
    /// kit refuses it (`KeyedAtLimit`, nothing applied) and K2 answers.
    fn refuse_a_change_past_the_limit(f: &KeyedFx) -> (SessionError, serde_json::Value, &'static str) {
        let path = f.ws.join("talk.deck");
        std::fs::write(&path, "s1\ns2\n").expect("the file");
        let s = f.mgr.open_file(Arc::new(Deck), &deck_key(), &path, &f.ws).expect("two parts open live at a limit of 2");
        let agent = Participant::new("agent:proj-1", "Ada", "#654321", ParticipantKind::Agent, "cli");
        let bot = s.join(agent, Stay::Held).expect("join").conn;
        let mut opts = EditOptions::new();
        let before = s.read(bot).expect("read").version;
        opts.base = Some(before);
        let ops = vec![MapOp::Set { key: "s1:title".into(), value: Value::str("Agent") }];
        let e = match s.edit_ops(bot, vec![Edit::Map { root: FIELDS.into(), ops }], &opts) {
            Ok(_) => panic!("a third part past a limit of 2 is refused, but it was applied"),
            Err(e) => e,
        };
        assert!(matches!(e, SessionError::KeyedAtLimit { limit: 2, would_be: 3, .. }), "kit v0.18.0's keyed refusal: {e:?}");
        assert_eq!(s.read(bot).expect("read").version, before, "nothing applied");
        let r = super::super::routes::session_error(&e);
        let body: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        (e, body["error"].clone(), r.status)
    }

    /// MPC-E822: a change that would grow a live keyed doc (a deck, a board)
    /// past its size limit reaches the person or agent in plain words: the
    /// limit, whose limit it is (here the server's for every file type),
    /// that nothing changed, and what to do; the details name the limit and
    /// its source as the kit gives them.
    #[test]
    fn a_keyed_doc_at_the_servers_limit_answers_in_plain_words() {
        let f = keyed_fx("at-limit", Some(2));
        let (e, err, status) = refuse_a_change_past_the_limit(&f);
        assert!(matches!(e, SessionError::KeyedAtLimit { from: LimitSource::HostWide, .. }), "{e:?}");
        assert!(status.starts_with("400"), "{status}");
        assert_eq!(err["id"], "MPC-E822", "{err}");
        assert_eq!(err["code"], "keyed_at_limit", "{err}");
        assert_eq!(err["exit"], 2, "{err}");
        assert_eq!(err["details"]["limit"], 2, "{err}");
        assert_eq!(err["details"]["limit_source"], "host_wide", "{err}");
        let hint = err["hint"].as_str().expect("hint");
        assert_eq!(hint, super::super::routes::keyed_at_limit_hint(2, LimitSource::HostWide), "{err}");
        assert!(hint.contains("2 parts (this server's limit for every file type;"), "{hint}");
        assert!(hint.contains("Nothing was changed"), "{hint}");
        assert!(!hint.contains("MPC-") && !hint.contains("keyed_max_entries"), "no kit jargon: {hint}");
    }

    /// MPC-E822 under a limit the server sets for the file type
    /// (`keyed_max_entries_by_pack`, which wins over the host-wide one): the
    /// answer says it is this file type's limit.
    #[test]
    fn a_keyed_doc_at_its_file_types_limit_says_so() {
        let f = keyed_fx_with("at-type-limit", |c| {
            c.keyed_max_entries_by_pack.insert(DECK.name.to_string(), 2);
        });
        let (e, err, _) = refuse_a_change_past_the_limit(&f);
        assert!(matches!(e, SessionError::KeyedAtLimit { from: LimitSource::HostPerPack, .. }), "{e:?}");
        assert_eq!(err["id"], "MPC-E822", "{err}");
        assert_eq!(err["details"]["limit_source"], "host_per_pack", "{err}");
        let hint = err["hint"].as_str().expect("hint");
        assert_eq!(hint, super::super::routes::keyed_at_limit_hint(2, LimitSource::HostPerPack), "{err}");
        assert!(hint.contains("2 parts (the limit this server sets for this file type;"), "{hint}");
    }
}
