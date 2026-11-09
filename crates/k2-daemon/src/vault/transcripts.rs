//! `k2 vault doctor --transcripts` (prd-vault-for-agents A7, V19): which
//! vault secrets appear in which of the agent transcripts on this machine.
//!
//! - **Names and files only.** A finding says a secret's name, the CLI, the
//!   file and a date. It never carries a value or a line of the file, and a
//!   path that happens to hold a value is redacted before it is reported.
//! - **Not an oracle (V19).** A person gets every transcript against every
//!   value, retained older versions too. An agent gets its own workspace's
//!   transcripts against the latest value of what it can already read, so a
//!   guess it plants tells it nothing it couldn't check itself.
//! - **Report only.** Transcripts belong to the CLI tools; K2 doesn't edit
//!   them.
//! - **Bounded.** Newest files first, a byte and a time budget per run, one
//!   run at a time, on the caller's blocking thread. The vault's locks are
//!   released before the first file is opened. A scan that couldn't cover
//!   everything says so and is not `ok`.
//!
//! Symlinks inside a transcript folder are never followed, and only regular
//! files are opened, so a planted FIFO or link can't hang or redirect a
//! person's scan.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use k2_core::chat_history::{claude_project_hash, matches_project_family, md5_hex};
use k2_core::llm_accounts::{store, Tool};
use k2_vault::redact::{wipe, Redactor, SURFACE_MIN_LEN};
use k2_vault::{LocalVault, Requester, VaultError};
use serde_json::{json, Value};

use super::actor::VaultCaller;

/// Bytes one run may read.
pub const DEFAULT_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Under the CLI's 120 s, so a slow disk gets a "stopped" answer rather
/// than a timeout.
const TIME_BUDGET: Duration = Duration::from_secs(90);
const CHUNK: usize = 1024 * 1024;
const MAX_FILES_LISTED: usize = 200_000;
const MAX_DEPTH: u8 = 12;
/// A finding lists at most this many paths; `fileCount` has the total.
pub const FILES_SHOWN: usize = 50;
/// Enough for a transcript's header line, however long its instructions.
const FIRST_LINE_MAX: u64 = 4 * 1024 * 1024;

pub const ASK_OWNER: &str = "ask your owner to run k2 vault doctor --transcripts";

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub bytes: u64,
    pub time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self { bytes: DEFAULT_BUDGET_BYTES, time: TIME_BUDGET }
    }
}

/// One scan at a time: each costs up to 2 GB of reading.
static RUNNING: AtomicBool = AtomicBool::new(false);

struct RunGuard;

impl RunGuard {
    fn take() -> Result<Self, VaultError> {
        if RUNNING.swap(true, Ordering::SeqCst) {
            return Err(VaultError::new("transcript_scan_running", 409, "a transcript scan is already running on this server; run it again when that one ends"));
        }
        Ok(RunGuard)
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cli {
    Claude,
    Codex,
    Grok,
    Gemini,
    Cursor,
    Pi,
    Hermes,
}

impl Cli {
    fn id(self) -> &'static str {
        match self {
            Cli::Claude => "claude",
            Cli::Codex => "codex",
            Cli::Grok => "grok",
            Cli::Gemini => "gemini",
            Cli::Cursor => "cursor",
            Cli::Pi => "pi",
            Cli::Hermes => "hermes",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Cli::Claude => "Claude",
            Cli::Codex => "Codex",
            Cli::Grok => "Grok",
            Cli::Gemini => "Gemini",
            Cli::Cursor => "Cursor",
            Cli::Pi => "Pi",
            Cli::Hermes => "Hermes",
        }
    }
}

/// A folder of transcripts, or one file shared by every workspace.
struct Root {
    cli: Cli,
    path: PathBuf,
    shared_file: bool,
}

/// Where each CLI keeps transcripts under `home`, then the same layouts in
/// the config folders K2 points a session at (`llm-accounts` slots) and any
/// the daemon itself runs with. A folder reached twice is scanned once.
fn roots(home: &Path) -> Vec<Root> {
    let dir = |cli, path: PathBuf| Root { cli, path, shared_file: false };
    let file = |cli, path: PathBuf| Root { cli, path, shared_file: true };
    let mut claude_homes = vec![home.join(".claude")];
    let mut codex_homes = vec![home.join(".codex")];
    let mut grok_homes = vec![home.join(".grok")];
    for (var, homes) in [("CLAUDE_CONFIG_DIR", &mut claude_homes), ("CODEX_HOME", &mut codex_homes), ("GROK_HOME", &mut grok_homes)] {
        if let Some(d) = std::env::var_os(var).filter(|d| !d.is_empty()) {
            homes.push(PathBuf::from(d));
        }
    }
    for (tool, homes) in [(Tool::Claude, &mut claude_homes), (Tool::Codex, &mut codex_homes), (Tool::Grok, &mut grok_homes)] {
        if let Ok(slots) = fs::read_dir(store::tool_dir(tool)) {
            for s in slots.flatten() {
                if s.file_type().is_ok_and(|t| t.is_dir()) {
                    homes.push(s.path());
                }
            }
        }
    }
    let mut out = vec![];
    for h in claude_homes {
        out.push(dir(Cli::Claude, h.join("projects")));
        out.push(file(Cli::Claude, h.join("history.jsonl")));
    }
    for h in codex_homes {
        out.push(dir(Cli::Codex, h.join("sessions")));
        out.push(file(Cli::Codex, h.join("history.jsonl")));
    }
    for h in grok_homes {
        out.push(dir(Cli::Grok, h.join("sessions")));
    }
    out.push(dir(Cli::Gemini, home.join(".gemini").join("tmp")));
    out.push(dir(Cli::Cursor, home.join(".cursor").join("chats")));
    out.push(dir(Cli::Pi, home.join(".pi").join("agent").join("sessions")));
    out.push(file(Cli::Hermes, home.join(".hermes").join("state.db")));
    out.push(file(Cli::Hermes, home.join(".hermes").join("state.db-wal")));
    let mut seen = HashSet::new();
    out.retain(|r| seen.insert(dunce::canonicalize(&r.path).unwrap_or_else(|_| r.path.clone())));
    out
}

struct Candidate {
    cli: Cli,
    tree: PathBuf,
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
    shared: bool,
}

impl Candidate {
    /// The path below its transcript folder, one name per step.
    fn parts(&self) -> Vec<String> {
        match self.path.strip_prefix(&self.tree) {
            Ok(rel) => rel.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            Err(_) => vec![],
        }
    }
}

#[derive(Default)]
struct Listing {
    files: Vec<Candidate>,
    capped: bool,
    unreadable: usize,
}

fn candidate(root: &Root, path: PathBuf, md: &fs::Metadata) -> Option<Candidate> {
    if md.len() == 0 {
        return None;
    }
    Some(Candidate { cli: root.cli, tree: root.path.clone(), path, size: md.len(), mtime: md.modified().unwrap_or(SystemTime::UNIX_EPOCH), shared: root.shared_file })
}

fn list(roots: &[Root]) -> Listing {
    let mut l = Listing::default();
    for root in roots {
        if root.shared_file {
            if let Ok(md) = fs::symlink_metadata(&root.path) {
                if md.is_file() {
                    l.files.extend(candidate(root, root.path.clone(), &md));
                }
            }
            continue;
        }
        // The folder itself may be a link (dotfile setups); nothing under it
        // is followed.
        if !fs::metadata(&root.path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let mut stack = vec![(root.path.clone(), 0u8)];
        while let Some((dir, depth)) = stack.pop() {
            let rd = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) => {
                    if e.kind() != ErrorKind::NotFound {
                        l.unreadable += 1;
                    }
                    continue;
                }
            };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(md) = fs::symlink_metadata(&p) else { continue };
                if md.is_dir() {
                    if depth < MAX_DEPTH {
                        stack.push((p, depth + 1));
                    }
                } else if md.is_file() {
                    l.files.extend(candidate(root, p, &md));
                    if l.files.len() >= MAX_FILES_LISTED {
                        l.capped = true;
                        return l;
                    }
                }
            }
        }
    }
    l
}

/// A regular file, never through a link, and never blocking on a FIFO that
/// took its place after the listing.
fn open_regular(path: &Path) -> std::io::Result<File> {
    let mut o = fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = o.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::Error::new(ErrorKind::InvalidInput, "not a regular file"));
    }
    Ok(f)
}

fn first_line_json(path: &Path) -> Option<Value> {
    let f = open_regular(path).ok()?;
    let mut line = String::new();
    BufReader::new(f.take(FIRST_LINE_MAX)).read_line(&mut line).ok()?;
    serde_json::from_str(line.trim()).ok()
}

struct Project {
    name: String,
    path: String,
    canon: String,
    slug: String,
}

impl Project {
    fn owns_cwd(&self, cwd: &str) -> bool {
        matches_project_family(cwd, &self.path) || matches_project_family(cwd, &self.canon)
    }

    fn owns_slug(&self, slug: &str) -> bool {
        slug == self.slug || slug.strip_prefix(&self.slug).is_some_and(|rest| rest.starts_with("--"))
    }
}

/// Every registered workspace: `(id, project)`.
fn projects() -> Result<Vec<(String, Project)>, VaultError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn.prepare("SELECT id, name, path FROM projects").map_err(|e| VaultError::internal(format!("projects: {e}")))?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .map_err(|e| VaultError::internal(format!("projects: {e}")))?;
    let mut out = vec![];
    for r in rows {
        let (id, name, path) = r.map_err(|e| VaultError::internal(format!("projects: {e}")))?;
        let canon = dunce::canonicalize(&path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.clone());
        let slug = claude_project_hash(&path);
        out.push((id, Project { name, path, canon, slug }));
    }
    Ok(out)
}

/// Decides what is one workspace's. Anything it can't tie to this workspace
/// isn't scanned for its agent: shared history files, a transcript with no
/// readable working folder.
struct Workspace {
    project: Project,
    /// Slugs of the workspaces nested under this one's slug.
    other_slugs: Vec<String>,
    cursor_keys: HashSet<String>,
    gemini_slugs: HashSet<String>,
    grok_sessions: HashMap<PathBuf, bool>,
}

impl Workspace {
    fn new(id: &str, all: Vec<(String, Project)>, home: &Path) -> Result<Self, VaultError> {
        let mut project = None;
        let mut others = vec![];
        for (pid, p) in all {
            if pid == id {
                project = Some(p);
            } else {
                others.push(p);
            }
        }
        let project = project.ok_or_else(|| VaultError::not_found("this agent's workspace isn't registered on this server"))?;
        let other_slugs = others.iter().filter(|o| o.slug.len() > project.slug.len()).map(|o| o.slug.clone()).collect();
        let mut cursor_keys: HashSet<String> = [md5_hex(project.path.as_bytes()), md5_hex(project.canon.as_bytes())].into();
        if let Ok(wts) = fs::read_dir(Path::new(&project.path).join(".worktrees")) {
            for w in wts.flatten() {
                cursor_keys.insert(md5_hex(w.path().to_string_lossy().as_bytes()));
            }
        }
        let mut gemini_slugs = HashSet::new();
        let projects_json = home.join(".gemini").join("projects.json");
        if let Some(v) = fs::read_to_string(projects_json).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            if let Some(map) = v.get("projects").and_then(Value::as_object) {
                for (cwd, slug) in map {
                    if project.owns_cwd(cwd) {
                        gemini_slugs.extend(slug.as_str().map(String::from));
                    }
                }
            }
        }
        Ok(Self { project, other_slugs, cursor_keys, gemini_slugs, grok_sessions: HashMap::new() })
    }

    fn owns_claude(&self, slug: &str) -> bool {
        self.project.owns_slug(slug) && !self.other_slugs.iter().any(|o| slug == o || slug.strip_prefix(o.as_str()).is_some_and(|r| r.starts_with("--")))
    }

    fn owns_grok(&mut self, c: &Candidate, parts: &[String]) -> bool {
        let [cwd_dir, session, _, ..] = parts else { return false };
        let dir = c.tree.join(cwd_dir).join(session);
        if let Some(known) = self.grok_sessions.get(&dir) {
            return *known;
        }
        let owned = open_regular(&dir.join("summary.json"))
            .ok()
            .and_then(|f| serde_json::from_reader::<_, Value>(f.take(FIRST_LINE_MAX)).ok())
            .and_then(|v| v.pointer("/info/cwd").and_then(Value::as_str).map(|cwd| self.project.owns_cwd(cwd)))
            .unwrap_or(false);
        self.grok_sessions.insert(dir, owned);
        owned
    }

    fn owns(&mut self, c: &Candidate) -> bool {
        if c.shared {
            return false;
        }
        let parts = c.parts();
        let Some(first) = parts.first() else { return false };
        let cwd_at = |ptr: &str| first_line_json(&c.path).and_then(|v| v.pointer(ptr).and_then(Value::as_str).map(|cwd| self.project.owns_cwd(cwd))).unwrap_or(false);
        match c.cli {
            Cli::Claude => self.owns_claude(first),
            Cli::Codex => cwd_at("/payload/cwd"),
            Cli::Pi => cwd_at("/cwd"),
            Cli::Grok => self.owns_grok(c, &parts),
            Cli::Gemini => self.gemini_slugs.contains(first),
            Cli::Cursor => self.cursor_keys.contains(first),
            Cli::Hermes => false,
        }
    }
}

enum Scope {
    Person { projects: Vec<Project> },
    Workspace { requester: Requester, workspace: Box<Workspace> },
}

impl Scope {
    /// People who may see everything, an agent for its own workspace, and
    /// nobody else.
    fn of(c: &VaultCaller, home: &Path) -> Result<Self, VaultError> {
        if c.actor.is_admin_human() {
            return Ok(Scope::Person { projects: projects()?.into_iter().map(|(_, p)| p).collect() });
        }
        let Some(a) = c.actor.agent() else {
            return Err(VaultError::forbidden("doctor_transcripts_admin_only", format!("the transcript scan is for the owner, an Admin or a workspace's own agent; {ASK_OWNER}")));
        };
        let workspace = Workspace::new(&a.workspace_id, projects()?, home)?;
        Ok(Scope::Workspace { requester: c.requester()?, workspace: Box::new(workspace) })
    }

    fn name(&self) -> &'static str {
        match self {
            Scope::Person { .. } => "all",
            Scope::Workspace { .. } => "workspace",
        }
    }

    /// Whose transcript this is, for the finding's wording.
    fn workspace_of(&self, c: &Candidate) -> Option<String> {
        let projects = match self {
            Scope::Workspace { workspace, .. } => return Some(workspace.project.name.clone()),
            Scope::Person { projects } => projects,
        };
        if c.shared {
            return None;
        }
        let parts = c.parts();
        let owner = match c.cli {
            Cli::Claude => parts.first().and_then(|s| projects.iter().filter(|p| p.owns_slug(s)).max_by_key(|p| p.slug.len())),
            Cli::Codex | Cli::Pi => {
                let v = first_line_json(&c.path)?;
                let cwd = v.pointer(if c.cli == Cli::Codex { "/payload/cwd" } else { "/cwd" }).and_then(Value::as_str)?;
                projects.iter().filter(|p| p.owns_cwd(cwd)).max_by_key(|p| p.path.len())
            }
            _ => None,
        };
        owner.map(|p| p.name.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Bytes,
    Time,
}

struct Group {
    count: usize,
    newest: SystemTime,
    files: Vec<String>,
}

type Hits = BTreeMap<(String, Cli, Option<String>), Group>;

#[derive(Default)]
struct Reading {
    hits: Hits,
    files: u64,
    bytes: u64,
    unreadable: usize,
    stop: Option<Stop>,
    unchecked: usize,
}

/// One file against the set. `Err` is a file that couldn't be read.
fn read_one(path: &Path, size: u64, r: &Redactor, buf: &mut [u8], remaining: &mut u64, deadline: Instant, out: &mut Reading) -> std::io::Result<Vec<String>> {
    let mut f = open_regular(path)?;
    let mut scan = r.name_scan();
    let mut done = 0u64;
    loop {
        let want = usize::try_from((*remaining).min(CHUNK as u64)).expect("a chunk fits");
        if want == 0 {
            if done < size {
                out.stop = Some(Stop::Bytes);
            }
            break;
        }
        let n = match f.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if done == 0 => return Err(e),
            Err(_) => {
                out.unreadable += 1;
                break;
            }
        };
        scan.push(&buf[..n]);
        done += n as u64;
        *remaining -= n as u64;
        out.bytes += n as u64;
        if Instant::now() >= deadline && done < size {
            out.stop = Some(Stop::Time);
            break;
        }
    }
    Ok(scan.finish().into_iter().map(String::from).collect())
}

fn read_all(files: Vec<Candidate>, r: &Redactor, scope: &Scope, limits: Limits) -> Reading {
    let mut out = Reading::default();
    let mut remaining = limits.bytes;
    let deadline = Instant::now() + limits.time;
    let mut buf = vec![0u8; CHUNK];
    let total = files.len();
    for (i, c) in files.into_iter().enumerate() {
        if remaining == 0 {
            out.stop = Some(Stop::Bytes);
        } else if Instant::now() >= deadline {
            out.stop = Some(Stop::Time);
        }
        if out.stop.is_some() {
            out.unchecked = total - i;
            break;
        }
        let names = match read_one(&c.path, c.size, r, &mut buf, &mut remaining, deadline, &mut out) {
            Ok(n) => n,
            Err(e) => {
                if e.kind() != ErrorKind::NotFound {
                    out.unreadable += 1;
                }
                continue;
            }
        };
        out.files += 1;
        if !names.is_empty() {
            let workspace = scope.workspace_of(&c).map(|w| r.redact_str(&w));
            let shown = r.redact_str(&c.path.to_string_lossy());
            for name in names {
                let g = out.hits.entry((name, c.cli, workspace.clone())).or_insert(Group { count: 0, newest: c.mtime, files: vec![] });
                g.count += 1;
                g.newest = g.newest.max(c.mtime);
                if g.files.len() < FILES_SHOWN {
                    g.files.push(shown.clone());
                }
            }
        }
        if out.stop.is_some() {
            out.unchecked = total - i - 1;
            break;
        }
    }
    wipe(&mut buf);
    out
}

fn day(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).format("%Y-%m-%d").to_string()
}

fn size_text(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{} MB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

fn leak_finding(name: &str, cli: Cli, workspace: Option<&str>, g: &Group) -> Value {
    let owner = workspace.map(|w| format!("{w}'s ")).unwrap_or_default();
    let files = if g.count == 1 { "1 file".to_string() } else { format!("{} files", g.count) };
    let newest = day(g.newest);
    json!({
        "level": "error",
        "code": "transcript_leak",
        "secret": name,
        "cli": cli.id(),
        "workspace": workspace,
        "fileCount": g.count,
        "newest": newest,
        "files": g.files,
        "message": format!(
            "{name} appears in {files} of {owner}{} transcripts (newest {newest}). The provider may already hold a copy, so rotate it: k2 vault rotate {name}. K2 doesn't edit other tools' transcripts; to remove this copy, move the listed files to the Trash.",
            cli.label()
        ),
    })
}

/// The verb. Route entry: default limits.
pub fn doctor(vault: &LocalVault, c: &VaultCaller) -> Result<Value, VaultError> {
    run(vault, c, Limits::default())
}

pub fn run(vault: &LocalVault, c: &VaultCaller, limits: Limits) -> Result<Value, VaultError> {
    let home = k2_core::paths::home_dir().ok_or_else(|| VaultError::internal("no home folder"))?;
    let scope = Scope::of(c, &home)?;
    let _running = RunGuard::take()?;
    let who = match &scope {
        Scope::Person { .. } => None,
        Scope::Workspace { requester, .. } => Some(requester),
    };
    let set = vault.transcript_scan_set(who)?;
    vault.audit_note("doctor_transcripts", &c.actor, scope.name())?;

    let mut scope = scope;
    let mut listing = list(&roots(&home));
    if let Scope::Workspace { workspace, .. } = &mut scope {
        listing.files.retain(|f| workspace.owns(f));
    }
    listing.files.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
    let considered = listing.files.len();
    let reading = if set.secrets == 0 { Reading::default() } else { read_all(listing.files, &set.redactor, &scope, limits) };

    let mut findings: Vec<Value> = reading.hits.iter().map(|((name, cli, ws), g)| leak_finding(name, *cli, ws.as_deref(), g)).collect();
    let leaks = findings.len();
    match reading.stop {
        Some(Stop::Bytes) => findings.push(json!({
            "level": "warning",
            "code": "transcript_scan_capped",
            "unchecked": reading.unchecked,
            "message": format!("stopped at the {} budget for one run (newest files first); {} older file(s) weren't checked, so a leak there wouldn't show", size_text(limits.bytes), reading.unchecked),
        })),
        Some(Stop::Time) => findings.push(json!({
            "level": "warning",
            "code": "transcript_scan_capped",
            "unchecked": reading.unchecked,
            "message": format!("stopped at the {} s limit for one run (newest files first); {} older file(s) weren't checked, so a leak there wouldn't show", limits.time.as_secs(), reading.unchecked),
        })),
        None => {}
    }
    if listing.capped {
        findings.push(json!({ "level": "warning", "code": "transcript_scan_capped", "message": format!("listed only the first {MAX_FILES_LISTED} transcript files; the rest weren't checked") }));
    }
    let unreadable = listing.unreadable + reading.unreadable;
    if unreadable > 0 {
        findings.push(json!({ "level": "warning", "code": "transcript_files_unreadable", "count": unreadable, "message": format!("{unreadable} transcript file(s) or folder(s) couldn't be read, so they weren't checked") }));
    }
    if set.unreadable > 0 {
        findings.push(json!({ "level": "warning", "code": "transcript_values_unreadable", "count": set.unreadable, "message": format!("{} stored value(s) couldn't be opened, so they weren't looked for; run k2 vault doctor", set.unreadable) }));
    }
    if set.below_floor > 0 {
        findings.push(json!({ "level": "info", "code": "transcript_values_too_short", "count": set.below_floor, "message": format!("{} value(s) under {SURFACE_MIN_LEN} bytes are too short to match reliably and weren't looked for", set.below_floor) }));
    }
    if matches!(scope, Scope::Workspace { .. }) {
        findings.push(json!({ "level": "info", "code": "transcript_scan_scope", "message": format!("this covered this workspace's transcripts and the latest value of the secrets it can already read (file, env or api). For anything else, {ASK_OWNER}") }));
    }
    let complete = reading.stop.is_none() && !listing.capped && unreadable == 0 && set.unreadable == 0;
    let summary = format!(
        "transcripts: read {} of {considered} file(s), {}, for {} secret(s); scope: {}{}",
        reading.files,
        size_text(reading.bytes),
        set.secrets,
        scope.name(),
        if complete { "" } else { "; not complete" }
    );
    Ok(json!({
        "ok": leaks == 0 && complete,
        "complete": complete,
        "scope": scope.name(),
        "findings": findings,
        "summary": summary,
        "scan": {
            "files": reading.files,
            "filesConsidered": considered,
            "bytes": reading.bytes,
            "budgetBytes": limits.bytes,
            "secretsChecked": set.secrets,
        },
    }))
}

#[cfg(test)]
#[path = "transcripts_tests.rs"]
mod tests;
