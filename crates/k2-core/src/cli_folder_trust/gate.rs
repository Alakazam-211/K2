//! The folder-trust gate (prd-security-sweep-0464-v1 rev 2, SS9–SS11).
//!
//! Before 0.46.4 K2 told Claude, Codex, Grok, Gemini and Cursor that every
//! workspace folder was trusted, silently. That turns off the one prompt
//! that stands between a repo's "run this on start" settings and the
//! person's machine (CVE-2025-59536, CVE-2026-21852; the Tensorlake and
//! ChainDrop worms), and K2's presets also skip tool approvals. Now K2
//! looks first ([`super::scan`]):
//!
//! - a **clean** folder is trusted as before;
//! - a **risky** folder is trusted only under a person's decision that
//!   covers what the scan sees: `always`, or `once` for this exact
//!   fingerprint. Otherwise it is **held**: K2 writes no grant, removes the
//!   grants it wrote before, starts no AI tool there, and asks the person
//!   with a one-tap Thread card (the daemon's presenter, [`set_presenter`]);
//! - a **denied** folder (`deny`) gets no AI tool from K2 at all, and K2
//!   removes the grants it wrote and reports the rest.
//!
//! The daemon owns this: every door (add, register, hire, clone, every
//! harness spawn including `claude -p`, pinned and wallet slot homes, the
//! boot sweep, `POST /cli/trust-folders`) calls [`ensure`] or [`check`].
//!
//! **Keying (A5).** A decision's `scope` is the canonical work tree of the
//! repo's git common dir, or the folder outside git: every worktree of a
//! repo shares one decision. The fingerprint covers every tree the scan
//! reads for this cwd, so a worktree on a branch with other hooks asks
//! again (except under `always`).
//!
//! **What the scan reads (U0).** The cwd, its work tree's top, the main
//! checkout (the grant keys), every folder between the cwd and the top
//! (Codex reads each `.codex/config.toml` from the project root down to
//! the cwd, Grok each hook folder, Gemini the nearest `.env`), and nested
//! git repos at any depth below each granted folder (at most
//! [`NESTED_MAX_DIRS`] folders; over that, or an unreadable folder, is a
//! finding): Gemini and Cursor let a nested clone inherit the parent's
//! trust, so K2 trusting the parent trusts them too. Nested findings are kept for the CLIs that
//! inherit (Gemini, Cursor, and Claude's settings files, which Cursor
//! also runs). A clone made later is seen at the next scan; the daemon's
//! watcher re-scans folders with live sessions.
//!
//! **Re-scan on change (U0 finding 1).** Claude reloads `hooks` and
//! `apiKeyHelper` mid-session, so a start-time snapshot isn't enough. The
//! daemon re-runs [`ensure`] whenever the risky files of a folder with a
//! live K2 session change (`folder_trust_watch`); a changed fingerprint on
//! a `once` folder is undecided again: grants come off and the card is
//! presented.
//!
//! What this does and doesn't stop is in `k2 study folder-trust` (§3.6):
//! it stops K2's own silent grant and any K2-started session running
//! outside start-up code no person has seen. It is not a wall against an
//! agent that runs as the same OS user (it can read the owner token, edit
//! the trust files, or run the CLIs itself).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::scan::{self, Finding};
use super::{
    canonical_abs, current_home, grant_keys, is_refused_path, leftover_grants, main_checkout, revoke_grants,
    revoke_slot_homes, skip_unisolated_home, upgrade_k2_grants, worktree_top, write_grants, GrantedKeys,
};

/// The 409 code while a card is waiting (or nobody decided yet).
pub const PENDING: &str = "folder_trust_pending";
/// The 409 code after "Don't trust".
pub const DENIED: &str = "folder_trust_denied";
/// Findings listed in a refusal or a card; the rest are counted.
pub const MAX_LISTED: usize = 20;
/// Nested repos: the walk below a granted folder looks at at most this many
/// folders, at any depth. Over the budget, or a folder it can't read, is a
/// finding ("not fully scanned"): the scan fails closed (review A-H1).
pub const NESTED_MAX_DIRS: usize = 20_000;
/// Folders the nested walk never enters: git's own data and package
/// caches that can't hold a nested checkout's `.git` (npm, cargo and pip
/// strip it).
const NESTED_SKIP: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".venv", "venv"];
/// Folders between the cwd and its work tree top that are scanned.
const MAX_BETWEEN: usize = 8;
/// Tools whose settings a nested clone inherits through the parent's
/// trust (U0 Q4: Gemini and Cursor; Cursor also runs `.claude` hooks).
const NESTED_TOOLS: &[&str] = &["gemini", "cursor", "claude"];
/// The marker a refusal leaves in an error string (see [`Refusal::to_error_string`]).
const TOKEN_OPEN: &str = "[k2ft:";
/// The one skip a gated spawn may pass: a test build whose HOME isn't a
/// temp folder writes and checks nothing (never in a release build).
pub const SKIP_TEST_HOME: &str = "test HOME is not isolated";

// ── Types ─────────────────────────────────────────────────────────────

/// A person's decision on a risky folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// "Trust it": this fingerprint only.
    Once,
    /// "Always trust this folder".
    Always,
    /// "Don't trust".
    Deny,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Once => "once",
            Decision::Always => "always",
            Decision::Deny => "deny",
        }
    }

    pub fn parse(s: &str) -> Option<Decision> {
        match s.trim() {
            "once" | "yes" | "trust" => Some(Decision::Once),
            "always" => Some(Decision::Always),
            "deny" | "no" => Some(Decision::Deny),
            _ => None,
        }
    }
}

/// Which door asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// Add / register / hire / clone / unpack: present the card when held.
    Add,
    /// A harness spawn (and the watcher's re-scan): present the card when held.
    Spawn,
    /// Boot sweep and `POST /cli/trust-folders`: re-assert decisions that
    /// already grant; never a card, never a grant for an undecided risky
    /// folder.
    Sweep,
    /// Look only: no grant written, nothing removed, no card (slot homes,
    /// heartbeat pre-checks, status).
    Check,
    /// An agent's `k2 workspace trust request`: present the card when held,
    /// but never write or remove a grant (review A-H1).
    Request,
}

/// Who asks (for the card's place and wording, and the audit).
#[derive(Debug, Clone, Default)]
pub struct Actor {
    /// The workspace (`projects.id`) whose Thread should carry the card
    /// when the folder's own workspace has none: the hiring or cloning
    /// agent's workspace, or the spawn's workspace.
    pub project_id: Option<String>,
    /// `owner`, `agent:<addr>`, `v1`, `sweep`, `watch`… (audit only).
    pub by: String,
    /// An agent's reason (`k2 workspace trust request --why`).
    pub why: Option<String>,
    /// The watcher saw the risky files change while a K2 session runs here.
    pub changed_live: bool,
    /// The harness about to start (`claude`, `codex`…), for the card's words.
    pub tool: Option<String>,
    /// Who really asked, as the card names them (review B#4): the agent
    /// whose passport made the request, a login, `the owner`, or "<agent>'s
    /// heartbeat". `None`: the presenter works it out.
    pub caller: Option<String>,
}

impl Actor {
    pub fn by(by: &str) -> Actor {
        Actor { by: by.to_string(), ..Default::default() }
    }
}

/// What the scan saw for one cwd.
#[derive(Debug, Clone, Serialize)]
pub struct Evaluation {
    /// The canonical cwd.
    pub path: String,
    /// The decision key (git common dir's work tree, or the folder).
    pub scope: String,
    /// The folders K2 writes grants for (cwd, git root / main checkout).
    pub keys: Vec<String>,
    /// Every tree the scan read.
    pub roots: Vec<String>,
    pub in_git: bool,
    /// Findings; `file` is relative to `path` (a root outside it keeps
    /// its absolute path as a prefix).
    pub findings: Vec<Finding>,
    pub fingerprint: Option<String>,
}

impl Evaluation {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// The open ask for a scope (`folder_trust.pending_card`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    pub at: i64,
}

/// One `folder_trust` row.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub scope: String,
    pub decision: Option<Decision>,
    pub fingerprint: Option<String>,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub k2_granted: bool,
    pub granted: GrantedKeys,
    pub pending: Option<Pending>,
    pub upgrade: bool,
}

/// Why K2 won't start an AI tool here. Serializes to the 409 body:
/// `{"error":"folder_trust_pending","findings":[…],"decide":"k2 workspace
/// trust <folder> --yes|--always|--no (owner)","ticket":"<id>",…}`.
#[derive(Debug, Clone, Serialize)]
pub struct Refusal {
    /// [`PENDING`] or [`DENIED`].
    pub error: String,
    /// One plain sentence for people and the CLI.
    pub message: String,
    pub path: String,
    pub scope: String,
    pub findings: Vec<Finding>,
    /// Findings not listed (over [`MAX_LISTED`]).
    pub more: usize,
    pub fingerprint: Option<String>,
    pub card: Option<String>,
    pub ticket: Option<String>,
    /// How the owner decides.
    pub decide: String,
    /// Held or denied: grants K2 didn't write (or can't tell from the
    /// person's) that still trust this folder; [`forget_hint`] removes them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub leftover: Vec<String>,
    /// A fresh random token per refusal (review B#6): what the error
    /// string carries, so nothing can name a refusal it didn't get.
    #[serde(skip)]
    token: String,
}

impl Refusal {
    fn token_id(&self) -> String {
        self.token.clone()
    }

    /// For the `Result<_, String>` spawn paths: the code, the sentence and
    /// a token (`[k2ft:<id>]`, ASCII, no quotes) the daemon's response
    /// writer turns back into this refusal's 409 body
    /// ([`refusal_in_text`]), whatever prefix or quoting the error picked
    /// up on the way.
    pub fn to_error_string(&self) -> String {
        remember(self);
        format!("{}: {} {TOKEN_OPEN}{}]", self.error, self.message, self.token_id())
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({ "error": self.error }))
    }
}

/// What [`ensure`] decided.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Trust may stand (written unless the door only looks).
    Granted { scope: String, clean: bool },
    /// Risky, no decision covers it: no grant, a card is waiting.
    Held(Refusal),
    /// "Don't trust".
    Denied(Refusal),
    /// Not gated: an empty or refused path (`$HOME`, `/`, a parent of
    /// `$HOME`), no HOME, or a test HOME that isn't isolated.
    Skipped(String),
}

impl Outcome {
    /// `folderTrust` in an add/hire/clone answer: `{state, findings, …}`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Outcome::Granted { scope, clean } => serde_json::json!({
                "state": if *clean { "clean" } else { "trusted" },
                "scope": scope,
                "findings": [],
            }),
            Outcome::Held(r) => {
                let mut v = r.to_json();
                v["state"] = serde_json::json!("held");
                v
            }
            Outcome::Denied(r) => {
                let mut v = r.to_json();
                v["state"] = serde_json::json!("denied");
                v
            }
            Outcome::Skipped(why) => serde_json::json!({ "state": "skipped", "reason": why, "findings": [] }),
        }
    }

    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Outcome::Held(r) | Outcome::Denied(r) => Some(r),
            _ => None,
        }
    }
}

// ── The card presenter (registered by the daemon) ────────────────────

/// What the presenter is asked to put in front of the person.
pub struct Ask<'a> {
    pub eval: &'a Evaluation,
    pub actor: &'a Actor,
    /// A card for an older fingerprint of this scope is waiting.
    pub replaces: Option<&'a Pending>,
}

/// Where the ask went.
#[derive(Debug, Clone, Default)]
pub struct Asked {
    pub card: Option<String>,
    pub conversation: Option<String>,
    pub project: Option<String>,
    pub ticket: Option<String>,
}

/// How asks reach a person: `present` posts the card (or files the owner
/// ticket) and `is_open` says whether a recorded ask is still waiting (the
/// card pending and not expired, the ticket open).
pub struct Presenter {
    pub present: Box<dyn Fn(&Ask<'_>) -> Asked + Send + Sync>,
    pub is_open: Box<dyn Fn(&Pending) -> bool + Send + Sync>,
}
static PRESENTER: OnceLock<Presenter> = OnceLock::new();

/// The daemon registers the Thread card (owner ticket where there is no
/// Thread) once at boot. Without one (a bare library caller) a held folder
/// records an ask that reached nobody, and the next door asks again.
pub fn set_presenter(p: Presenter) {
    if PRESENTER.set(p).is_err() {
        log_debug!("[folder-trust] presenter already set");
    }
}

/// True once a presenter is registered.
pub fn has_presenter() -> bool {
    PRESENTER.get().is_some()
}

// ── Refusal registry (error-string token → body) ──────────────────────

static REFUSALS: Mutex<Option<HashMap<String, Refusal>>> = Mutex::new(None);
const REFUSALS_MAX: usize = 512;

fn remember(r: &Refusal) {
    let mut g = REFUSALS.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() >= REFUSALS_MAX {
        m.clear();
    }
    m.insert(r.token_id(), r.clone());
}

/// The refusal whose token appears in `text` (an error string or a JSON
/// body that carries one), if K2 made it in this process.
pub fn refusal_in_text(text: &str) -> Option<Refusal> {
    let start = text.find(TOKEN_OPEN)? + TOKEN_OPEN.len();
    let rest = &text[start..];
    let end = rest.find(']')?;
    let id = &rest[..end];
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let g = REFUSALS.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref()?.get(id).cloned()
}

/// True when `text` carries a refusal token (cheap; no lookup).
pub fn has_refusal_token(text: &str) -> bool {
    text.contains(TOKEN_OPEN)
}

// ── Scanning ──────────────────────────────────────────────────────────

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_process_home(home: &Path) -> bool {
    current_home()
        .and_then(|h| canonical_abs(&h))
        .is_some_and(|h| canonical_abs(home).as_deref() == Some(h.as_path()))
}

/// `git ls-files --error-unmatch` for one root-relative path (only asked
/// for Claude's `settings.local.json`, A5).
fn git_tracked(root: &Path, rel: &Path) -> bool {
    let Ok(out) = crate::git_safe::git(root)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(rel)
        .output()
    else {
        return false;
    };
    out.status.success()
}

/// What the nested-repo walk found below one granted folder.
#[derive(Default, Clone)]
struct NestedWalk {
    repos: Vec<PathBuf>,
    /// Why the walk is incomplete (over budget, a folder it couldn't
    /// read): each one is a finding, so the folder isn't called clean.
    problems: Vec<String>,
    /// Every folder the walk listed and every `.git` it saw (the cache's
    /// stamp).
    dirs: Vec<PathBuf>,
    dotgits: Vec<PathBuf>,
}

/// A `.git` FILE pointing into `<main>/.git/worktrees/…`: a worktree of the
/// same repo (K2's and Claude Code's own worktrees), not a nested clone.
fn same_repo_worktree(dir: &Path, main: Option<&Path>) -> bool {
    let Some(main) = main else { return false };
    let dotgit = dir.join(".git");
    if !std::fs::symlink_metadata(&dotgit).map(|m| m.is_file()).unwrap_or(false) {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(&dotgit) else { return false };
    let Some(gitdir) = text.lines().find_map(|l| l.trim().strip_prefix("gitdir:")).map(str::trim) else { return false };
    let gitdir = if Path::new(gitdir).is_absolute() { PathBuf::from(gitdir) } else { dir.join(gitdir) };
    match (dunce::canonicalize(&gitdir), dunce::canonicalize(&main.join(".git").join("worktrees"))) {
        (Ok(g), Ok(w)) => g.starts_with(w),
        _ => false,
    }
}

/// Git repos below `root` (a `.git` file or folder), at any depth, never
/// following a symlink. Worktrees of the same repo are skipped (not
/// entered); everything else is entered, hidden folders included (a
/// submodule may sit in one). Over [`NESTED_MAX_DIRS`] folders or a folder
/// that can't be read makes the walk incomplete.
///
/// Breadth first, each folder's entries in name order (review round 2 M4):
/// the same tree always stops at the same folder when it is over budget,
/// so the fingerprint can't flap between scans. A folder a rootless
/// container's user owns (uid ≥ 100000, not ours: podman volume data) that
/// we can't read is left out, not a finding: no CLI running as us can
/// read it either.
fn nested_repos(root: &Path, main: Option<&Path>) -> NestedWalk {
    let mut w = NestedWalk::default();
    let mut queue: std::collections::VecDeque<PathBuf> = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut visited = 0usize;
    while let Some(dir) = queue.pop_front() {
        w.dirs.push(dir.clone());
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            // A folder that isn't there (a cwd not made yet, one removed
            // mid-walk) holds nothing to trust; anything else fails closed.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && container_owned(&dir) => {
                log_debug!("[folder-trust] nested walk: {} is a container's (left out)", dir.display());
                continue;
            }
            Err(e) => {
                w.problems.push(format!("can't read {} ({:?})", dir.display(), e.kind()));
                continue;
            }
        };
        let mut entries: Vec<(String, PathBuf)> = Vec::new();
        for e in rd {
            let e = match e {
                Ok(e) => e,
                Err(err) => {
                    w.problems.push(format!("can't list {} ({:?})", dir.display(), err.kind()));
                    continue;
                }
            };
            let ft = match e.file_type() {
                Ok(ft) => ft,
                Err(err) => {
                    w.problems.push(format!("can't read {} ({:?})", e.path().display(), err.kind()));
                    continue;
                }
            };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if NESTED_SKIP.contains(&name.as_str()) {
                continue;
            }
            entries.push((name, e.path()));
        }
        entries.sort();
        for (_, p) in entries {
            visited += 1;
            if visited > NESTED_MAX_DIRS {
                w.problems.push(format!("over {NESTED_MAX_DIRS} folders below {}", root.display()));
                w.repos.sort();
                return w;
            }
            let dotgit = p.join(".git");
            if std::fs::symlink_metadata(&dotgit).is_ok() {
                w.dotgits.push(dotgit);
                if same_repo_worktree(&p, main) {
                    continue;
                }
                w.repos.push(p.clone());
            }
            queue.push_back(p);
        }
    }
    w.repos.sort();
    w
}

/// A folder owned by another uid in the rootless-container range (podman
/// and docker rootless map container users to subuids ≥ 100000).
#[cfg(unix)]
fn container_owned(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and can't fail.
    let me = unsafe { libc::geteuid() };
    std::fs::symlink_metadata(dir).map(|m| m.uid() >= 100_000 && m.uid() != me).unwrap_or(false)
}

#[cfg(not(unix))]
fn container_owned(_dir: &Path) -> bool {
    false
}

// ── Scan caches (review round 2 M4) ──────────────────────────────────
//
// Spawns, heartbeats, status and the watcher evaluate the same folders
// over and over. Each cache entry holds a stamp of what its result read:
// inode, size, mode and ctime of every entry (ctime can't be set back by
// a process, unlike mtime). A stamp taken within [`RACY_SECS`] of the
// scan's start isn't cached (git's "racily clean" rule), so an edit
// during a scan is never hidden.

/// Entries stamped for one cached result; more and the result isn't cached.
const SIG_MAX: usize = 6000;
/// A stamp this close to the scan's start isn't trusted to cache.
const RACY_SECS: i64 = 2;
/// Cached folders kept (all dropped when full).
const CACHE_MAX: usize = 1024;
/// Config folders the scanner reads (walked whole for the stamp).
const SIG_DIRS: &[&str] = &[
    ".claude",
    ".cursor",
    ".gemini",
    ".codex",
    ".grok",
    ".vscode",
    ".git/hooks",
    ".k2/agent",
    ".k2/agents",
    ".k2/skills",
    ".k2/agent-templates",
    ".k2so/agent",
    ".k2so/agents",
    ".k2so/skills",
    ".k2so/agent-templates",
];
/// Entries of `.claude` the scanner skips (its worktrees).
const SIG_SKIP_TOP: &[&str] = &["worktrees"];

/// (path, inode, size, mode, ctime seconds, ctime nanos); a missing path
/// stamps as zeros.
type Stamp = (String, u64, u64, u32, i64, i64);

fn stamp_of(p: &Path) -> Stamp {
    let key = p.to_string_lossy().to_string();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(p) {
            Ok(m) => (key, m.ino(), m.size(), m.mode(), m.ctime(), m.ctime_nsec()),
            Err(_) => (key, 0, 0, 0, 0, 0),
        }
    }
    #[cfg(not(unix))]
    {
        match std::fs::symlink_metadata(p) {
            Ok(m) => {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
                (key, 0, m.len(), u32::from(m.is_dir()), t.as_secs() as i64, i64::from(t.subsec_nanos()))
            }
            Err(_) => (key, 0, 0, 0, 0, 0),
        }
    }
}

/// The stamp of everything [`scan::scan_folder`] reads in `root`: the root
/// and each of its entries, every entry of the config folders, git's
/// config and index (the tracked check), and `inputs` with their parents.
/// `None` when there is too much to stamp.
fn scan_signature(root: &Path, inputs: &[String]) -> Option<Vec<Stamp>> {
    let mut out: Vec<Stamp> = vec![stamp_of(root)];
    let rd = match std::fs::read_dir(root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(out),
        Err(_) => return None,
    };
    let mut top: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    if top.len() > SIG_MAX {
        return None;
    }
    top.sort();
    out.extend(top.iter().map(|p| stamp_of(p)));
    for rel in [".git/config", ".git/index"] {
        out.push(stamp_of(&root.join(rel)));
    }
    // A worktree's index lives in its gitdir.
    if let Ok(text) = std::fs::read_to_string(root.join(".git")) {
        if let Some(g) = text.lines().find_map(|l| l.trim().strip_prefix("gitdir:")).map(str::trim) {
            let g = if Path::new(g).is_absolute() { PathBuf::from(g) } else { root.join(g) };
            out.push(stamp_of(&g.join("index")));
        }
    }
    for d in SIG_DIRS {
        let base = root.join(d);
        out.push(stamp_of(&base));
        let mut stack = vec![(base, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut names: Vec<(PathBuf, bool)> = Vec::new();
            for e in rd.filter_map(|e| e.ok()) {
                if depth == 0 && *d == ".claude" && SIG_SKIP_TOP.iter().any(|s| e.file_name() == *s) {
                    continue;
                }
                let is_dir = e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false);
                names.push((e.path(), is_dir));
            }
            names.sort();
            for (p, is_dir) in names {
                out.push(stamp_of(&p));
                if out.len() > SIG_MAX {
                    return None;
                }
                if is_dir {
                    stack.push((p, depth + 1));
                }
            }
        }
    }
    for rel in inputs {
        let p = root.join(rel);
        out.push(stamp_of(&p));
        if let Some(parent) = p.parent() {
            out.push(stamp_of(parent));
        }
    }
    if out.len() > SIG_MAX {
        return None;
    }
    Some(out)
}

/// The stamp of a nested walk: every folder it listed and every `.git` it
/// saw (a folder's ctime moves when an entry is added, removed or renamed
/// in it; a `.git` file's when it is rewritten).
fn nested_signature(w: &NestedWalk) -> Vec<Stamp> {
    w.dirs.iter().chain(w.dotgits.iter()).map(|p| stamp_of(p)).collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Every stamp is older than `started` by [`RACY_SECS`].
fn settled(sig: &[Stamp], started: i64) -> bool {
    sig.iter().all(|s| s.4 < started - RACY_SECS)
}

type ScanCache = Mutex<HashMap<PathBuf, (Vec<Stamp>, scan::ScanResult)>>;
type NestedCache = Mutex<HashMap<(PathBuf, Option<PathBuf>), (Vec<Stamp>, NestedWalk)>>;

fn scan_cache() -> &'static ScanCache {
    static C: OnceLock<ScanCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn nested_cache() -> &'static NestedCache {
    static C: OnceLock<NestedCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Scans served from the cache (tests read it).
static CACHE_HITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn cache_hits() -> usize {
    CACHE_HITS.load(std::sync::atomic::Ordering::SeqCst)
}

/// [`scan::scan_folder`] with git's tracked check, from the cache when
/// nothing it read has changed.
fn scan_cached(root: &Path) -> scan::ScanResult {
    let cached = scan_cache().lock().unwrap_or_else(|e| e.into_inner()).get(root).cloned();
    if let Some((sig, result)) = cached {
        if scan_signature(root, &result.inputs).as_ref() == Some(&sig) {
            CACHE_HITS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return result;
        }
    }
    let started = now_secs();
    let tracked = |rel: &Path| git_tracked(root, rel);
    let result = scan::scan_folder(root, &tracked);
    if let Some(sig) = scan_signature(root, &result.inputs) {
        if settled(&sig, started) {
            let mut g = scan_cache().lock().unwrap_or_else(|e| e.into_inner());
            if g.len() >= CACHE_MAX {
                g.clear();
            }
            g.insert(root.to_path_buf(), (sig, result.clone()));
        }
    }
    result
}

/// [`nested_repos`] from the cache when no folder it listed changed.
fn nested_cached(root: &Path, main: Option<&Path>) -> NestedWalk {
    let key = (root.to_path_buf(), main.map(Path::to_path_buf));
    let cached = nested_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
    if let Some((sig, walk)) = cached {
        if nested_signature(&walk) == sig {
            CACHE_HITS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return walk;
        }
    }
    let started = now_secs();
    let walk = nested_repos(root, main);
    let sig = nested_signature(&walk);
    if walk.dirs.len() + walk.dotgits.len() <= NESTED_MAX_DIRS + 1 && settled(&sig, started) {
        let mut g = nested_cache().lock().unwrap_or_else(|e| e.into_inner());
        if g.len() >= CACHE_MAX {
            g.clear();
        }
        g.insert(key, (sig, walk.clone()));
    }
    walk
}

fn label_for(root: &Path, cwd: &Path) -> String {
    if root == cwd {
        String::new()
    } else if let Ok(rel) = root.strip_prefix(cwd) {
        format!("{}/", rel.to_string_lossy().replace('\\', "/"))
    } else {
        format!("{}/", root.to_string_lossy())
    }
}

/// Scan everything K2 should look at before trusting `path` for `home`.
/// `None` for a refused path (`$HOME`, `/`, a parent of `$HOME`).
pub fn evaluate_in(path: &str, home: &Path) -> Option<Evaluation> {
    let cwd = canonical_abs(Path::new(path.trim()))?;
    let home = canonical_abs(home).unwrap_or_else(|| home.to_path_buf());
    if is_refused_path(&cwd, &home) {
        return None;
    }
    let keys = grant_keys(&cwd, &home);
    let top = worktree_top(&cwd, &home);
    let in_git = top.is_some();
    // The main checkout only when a `.git` file's link is verified (A-M4);
    // else the work tree is its own scope.
    let common = main_checkout(&cwd, &home);
    let scope = common
        .clone()
        .or_else(|| top.clone())
        .unwrap_or_else(|| cwd.clone());

    // Roots: cwd, every folder between cwd and its work tree top, the top,
    // the main checkout.
    let mut roots: Vec<PathBuf> = vec![cwd.clone()];
    if let Some(top) = &top {
        let mut cur = cwd.parent();
        let mut n = 0;
        while let Some(d) = cur {
            if !d.starts_with(top) || n >= MAX_BETWEEN {
                break;
            }
            roots.push(d.to_path_buf());
            if d == top.as_path() {
                break;
            }
            n += 1;
            cur = d.parent();
        }
        roots.push(top.clone());
    }
    if let Some(c) = &common {
        roots.push(c.clone());
    }
    let mut uniq: Vec<PathBuf> = Vec::new();
    for r in roots {
        if !uniq.contains(&r) && !is_refused_path(&r, &home) {
            uniq.push(r);
        }
    }

    let mut findings: Vec<Finding> = Vec::new();
    // The fingerprint is a set of per-tree fingerprints (content and the
    // tree-relative path of each risky file), not where the tree sits: a
    // worktree with the same settings as its main checkout has the same
    // fingerprint, so one "Trust it" covers both (A5). Nested repos keep
    // their position (an inherited clone is a different thing).
    let mut prints: BTreeSet<String> = BTreeSet::new();
    for root in &uniq {
        let r = scan_cached(root);
        let label = label_for(root, &cwd);
        if let Some(fp) = &r.fingerprint {
            prints.insert(fp.clone());
        }
        for mut f in r.findings {
            f.file = format!("{label}{}", f.file);
            findings.push(f);
        }
    }
    // Nested repos below each folder K2 writes a grant for (the cwd, the
    // main checkout): Gemini and Cursor trust them through that grant.
    // Only what the inheriting CLIs read.
    let mut seen_nested: Vec<PathBuf> = Vec::new();
    for base in keys.iter().map(PathBuf::from) {
        let walk = nested_cached(&base, common.as_deref());
        for problem in &walk.problems {
            // Fail closed: a walk that couldn't see everything is a finding.
            let label = label_for(&base, &cwd);
            prints.insert(format!("incomplete:{label}:{problem}"));
            findings.push(Finding {
                tool: "k2",
                file: if label.is_empty() { ".".into() } else { label.trim_end_matches('/').to_string() },
                what: "not-fully-scanned".into(),
                detail: super::scan::clip_detail(&format!("nested repos not fully scanned: {problem}")),
            });
        }
        for repo in walk.repos {
            if uniq.contains(&repo) || seen_nested.contains(&repo) {
                continue;
            }
            seen_nested.push(repo.clone());
            let r = scan_cached(&repo);
            let nested_fp = r.fingerprint.clone().unwrap_or_default();
            let kept: Vec<Finding> = r.findings.into_iter().filter(|f| NESTED_TOOLS.contains(&f.tool)).collect();
            if kept.is_empty() {
                continue;
            }
            let label = label_for(&repo, &cwd);
            // The nested repo's whole scan fingerprint (every byte of its
            // risky files), at its position: a change there asks again.
            prints.insert(format!("nested:{label}:{nested_fp}"));
            for mut f in kept {
                f.file = format!("{label}{}", f.file);
                f.detail = super::scan::clip_detail(&format!("nested repo: {}", f.detail));
                findings.push(f);
            }
        }
    }
    let fingerprint = if prints.is_empty() {
        None
    } else {
        let body: Vec<&str> = prints.iter().map(String::as_str).collect();
        Some(sha_hex(body.join("\n").as_bytes()))
    };
    Some(Evaluation {
        path: cwd.to_string_lossy().to_string(),
        scope: scope.to_string_lossy().to_string(),
        keys,
        roots: uniq.iter().map(|r| r.to_string_lossy().to_string()).collect(),
        in_git,
        findings,
        fingerprint,
    })
}

/// [`evaluate_in`] for the process HOME.
pub fn evaluate(path: &str) -> Option<Evaluation> {
    let home = current_home()?;
    evaluate_in(path, &home)
}

// ── Store ─────────────────────────────────────────────────────────────

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let decision: Option<String> = r.get(1)?;
    let granted: String = r.get(6)?;
    let pending: Option<String> = r.get(7)?;
    Ok(Row {
        scope: r.get(0)?,
        decision: decision.as_deref().and_then(Decision::parse),
        fingerprint: r.get(2)?,
        decided_by: r.get(3)?,
        decided_at: r.get(4)?,
        k2_granted: r.get::<_, i64>(5)? != 0,
        granted: serde_json::from_str(&granted).unwrap_or_default(),
        pending: pending.as_deref().and_then(|p| serde_json::from_str(p).ok()),
        upgrade: r.get::<_, i64>(8)? != 0,
    })
}

const ROW_COLS: &str = "scope, decision, fingerprint, decided_by, decided_at, k2_granted, granted_keys, pending_card, upgrade";

pub fn load_row(conn: &Connection, scope: &str) -> Result<Option<Row>, String> {
    conn.query_row(&format!("SELECT {ROW_COLS} FROM folder_trust WHERE scope = ?1"), params![scope], row_from)
        .optional()
        .map_err(|e| format!("folder_trust read: {e}"))
}

fn ensure_row(conn: &Connection, scope: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO folder_trust (scope, updated_at) VALUES (?1, ?2)",
        params![scope, now()],
    )
    .map(|_| ())
    .map_err(|e| format!("folder_trust write: {e}"))
}

fn record_granted(conn: &Connection, scope: &str, changed: &GrantedKeys) -> Result<(), String> {
    if changed.is_empty() {
        return Ok(());
    }
    ensure_row(conn, scope)?;
    let mut granted = load_row(conn, scope)?.map(|r| r.granted).unwrap_or_default();
    granted.merge(changed);
    conn.execute(
        "UPDATE folder_trust SET k2_granted = 1, granted_keys = ?2, updated_at = ?3 WHERE scope = ?1",
        params![scope, serde_json::to_string(&granted).unwrap_or_else(|_| "{}".into()), now()],
    )
    .map(|_| ())
    .map_err(|e| format!("folder_trust write: {e}"))
}

fn set_pending(conn: &Connection, scope: &str, pending: Option<&Pending>) -> Result<(), String> {
    ensure_row(conn, scope)?;
    let json = pending.map(|p| serde_json::to_string(p).unwrap_or_default());
    conn.execute(
        "UPDATE folder_trust SET pending_card = ?2, updated_at = ?3 WHERE scope = ?1",
        params![scope, json, now()],
    )
    .map(|_| ())
    .map_err(|e| format!("folder_trust write: {e}"))
}

fn set_decision(conn: &Connection, scope: &str, d: Option<Decision>, fingerprint: Option<&str>, by: Option<&str>) -> Result<(), String> {
    ensure_row(conn, scope)?;
    let at = d.map(|_| now());
    conn.execute(
        "UPDATE folder_trust SET decision = ?2, fingerprint = ?3, decided_by = ?4, decided_at = ?5, pending_card = NULL, updated_at = ?6 WHERE scope = ?1",
        params![scope, d.map(Decision::as_str), fingerprint, by, at, now()],
    )
    .map(|_| ())
    .map_err(|e| format!("folder_trust write: {e}"))
}

/// Remove the grants K2 wrote for this scope from `home` (only the process
/// HOME is tracked). Returns removal errors (never silent).
fn revoke_k2(conn: &Connection, scope: &str, home: &Path) -> Vec<String> {
    if !is_process_home(home) {
        return Vec::new();
    }
    let row = match load_row(conn, scope) {
        Ok(Some(r)) => r,
        Ok(None) => return Vec::new(),
        Err(e) => return vec![e],
    };
    if row.granted.is_empty() {
        return Vec::new();
    }
    let errors = revoke_grants(home, &row.granted);
    if errors.is_empty() {
        if let Err(e) = conn.execute(
            "UPDATE folder_trust SET k2_granted = 0, granted_keys = '{}', updated_at = ?2 WHERE scope = ?1",
            params![scope, now()],
        ) {
            return vec![format!("folder_trust write: {e}")];
        }
        log_debug!("[folder-trust] removed K2's grants for {scope}");
    } else {
        log_debug!("[folder-trust] removing K2's grants for {scope}: {errors:?}");
    }
    errors
}

/// Is a recorded ask still waiting? An ask that reached nobody (no card,
/// no ticket) never is, so the next door asks again.
fn pending_open(p: &Pending) -> bool {
    if p.card.is_none() && p.ticket.is_none() {
        return false;
    }
    PRESENTER.get().is_some_and(|pr| (pr.is_open)(p))
}

/// The daemon's `is_open` for a Thread card or an owner ticket.
pub fn thread_card_or_ticket_open(p: &Pending) -> bool {
    let now = crate::overlay::now_secs();
    if let (Some(conv), Some(card)) = (p.conversation.as_deref(), p.card.as_deref()) {
        return match crate::overlay::get_thread_doc(conv, card) {
            Ok(Some(d)) => d.is_pending_choice() && !d.is_due(now),
            _ => false,
        };
    }
    if let Some(t) = p.ticket.as_deref() {
        return crate::feedback::get_item(t).is_some_and(|i| i.status != "answered" && i.status != "resolved");
    }
    false
}

// ── Refusals ──────────────────────────────────────────────────────────

/// Shell-quote a path for a command line people paste.
pub fn shell_quote(p: &str) -> String {
    if !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c)) {
        p.to_string()
    } else {
        format!("'{}'", p.replace('\'', "'\\''"))
    }
}

/// `k2 workspace trust <folder> --yes|--always|--no (owner)`.
pub fn decide_hint(path: &str) -> String {
    format!("k2 workspace trust {} --yes|--always|--no (owner)", shell_quote(path))
}

/// `k2 workspace trust forget <folder> (owner)`: remove every trust entry
/// for the folder in every CLI's store, the person's own included.
pub fn forget_hint(path: &str) -> String {
    format!("k2 workspace trust forget {} (owner)", shell_quote(path))
}

/// A gated program in a folder K2 can't check at all (no folder, `$HOME`,
/// `/`, a parent of `$HOME`): refused, never passed (review round 2 H1).
pub fn refusal_unchecked(path: &str, why: &str) -> Refusal {
    Refusal {
        error: DENIED.to_string(),
        message: format!(
            "K2 won't start programs in {path}: {why}. Only a plain terminal tab runs there; start the program in a project folder instead."
        ),
        path: path.to_string(),
        scope: path.to_string(),
        findings: Vec::new(),
        more: 0,
        fingerprint: None,
        card: None,
        ticket: None,
        decide: "start it in a project folder".to_string(),
        leftover: Vec::new(),
        token: uuid::Uuid::new_v4().simple().to_string(),
    }
}

/// One finding as a card / message line: "`file`: detail".
pub fn finding_line(f: &Finding) -> String {
    format!("{}: {}", f.file, f.detail)
}

fn refusal(code: &str, eval: &Evaluation, pending: Option<&Pending>, leftover: Vec<String>) -> Refusal {
    let more = eval.findings.len().saturating_sub(MAX_LISTED);
    let first = eval.findings.first().map(finding_line).unwrap_or_default();
    let leftover_note = if leftover.is_empty() {
        String::new()
    } else {
        format!(
            " Trust K2 didn't write still covers it ({}); to remove it: {}.",
            leftover.join("; "),
            forget_hint(&eval.path)
        )
    };
    let message = if code == DENIED {
        format!(
            "K2 won't start programs in {}: the owner chose \"Don't trust\" for its settings ({first}). To change it: Settings → Workspaces → Folder trust → Reset, or {}.{leftover_note}",
            eval.path,
            decide_hint(&eval.path)
        )
    } else {
        format!(
            "Waiting for the owner to trust {}: it has settings that run commands when programs start there ({first}{}). Answer the card on Thread, or: {}.{leftover_note}",
            eval.path,
            if eval.findings.len() > 1 { format!(", and {} more", eval.findings.len() - 1) } else { String::new() },
            decide_hint(&eval.path)
        )
    };
    Refusal {
        error: code.to_string(),
        message,
        path: eval.path.clone(),
        scope: eval.scope.clone(),
        findings: eval.findings.iter().take(MAX_LISTED).cloned().collect(),
        more,
        fingerprint: eval.fingerprint.clone(),
        card: pending.and_then(|p| p.card.clone()),
        ticket: pending.and_then(|p| p.ticket.clone()),
        decide: decide_hint(&eval.path),
        leftover,
        token: uuid::Uuid::new_v4().simple().to_string(),
    }
}

// ── The gate ──────────────────────────────────────────────────────────

/// Run the gate for `path`: scan, look up the decision, then grant
/// (writing into `home`, default the process HOME), hold or deny. See the
/// module doc for the rules.
pub fn ensure(path: &str, door: Door, actor: &Actor, home: Option<&Path>) -> Outcome {
    ensure_inner(path, door, actor, home, !matches!(door, Door::Check | Door::Request))
}

/// An agent's ask (`k2 workspace trust request`, review A-H1): scan, and
/// when the folder is held present the card. Never writes or removes a
/// grant, whatever the folder's state.
pub fn request(path: &str, actor: &Actor) -> Outcome {
    ensure_inner(path, Door::Request, actor, None, false)
}

/// [`ensure`] without writing or removing any grant (a guest HOME that
/// isn't on the host); a held folder still gets its card.
pub fn ensure_no_write(path: &str, door: Door, actor: &Actor) -> Outcome {
    ensure_inner(path, door, actor, None, false)
}

/// Look only: the decision for `path` as it is now (no grant, no card).
pub fn check(path: &str) -> Outcome {
    ensure_inner(path, Door::Check, &Actor::by("check"), None, false)
}

fn ensure_inner(path: &str, door: Door, actor: &Actor, home: Option<&Path>, write: bool) -> Outcome {
    if path.trim().is_empty() {
        return Outcome::Skipped("empty path".into());
    }
    let Some(home) = home.map(Path::to_path_buf).or_else(current_home) else {
        return Outcome::Skipped("no HOME".into());
    };
    if skip_unisolated_home(&home) {
        log_debug!("[folder-trust] skip: test HOME is not isolated ({})", home.display());
        return Outcome::Skipped(SKIP_TEST_HOME.into());
    }
    let home = canonical_abs(&home).unwrap_or(home);
    let Some(eval) = evaluate_in(path, &home) else {
        return Outcome::Skipped(format!("refused path {path}"));
    };
    // A 0.46.3 database's rows wait for the upgrade marks (A4): a door that
    // comes before the boot pass finishes this folder's rows inline with
    // the scan it just made (no lock held across a scan, round 2 M4).
    upgrade_rows_for(&eval, &home);
    let db = crate::db::shared();
    let row = {
        let conn = db.lock();
        match load_row(&conn, &eval.scope) {
            Ok(r) => r,
            Err(e) => {
                // Can't read the decisions: never grant a risky folder.
                log_debug!("[folder-trust] {e}");
                if eval.is_clean() {
                    if write {
                        write_grants(&home, &eval.keys);
                    }
                    return Outcome::Granted { scope: eval.scope, clean: true };
                }
                return Outcome::Held(refusal(PENDING, &eval, None, Vec::new()));
            }
        }
    };
    let decision = row.as_ref().and_then(|r| r.decision);

    // "Don't trust" is about the folder: it holds even if the risky files
    // are gone (the person was told K2 won't start AI tools here). Reset
    // undoes it.
    if decision == Some(Decision::Deny) {
        if write {
            let conn = db.lock();
            revoke_k2(&conn, &eval.scope, &home);
            drop(conn);
            revoke_slots(&eval, &home);
        }
        let leftover = leftover_grants(&home, &eval.keys, eval.in_git);
        return Outcome::Denied(refusal(DENIED, &eval, None, leftover));
    }

    let covered = eval.is_clean()
        || decision == Some(Decision::Always)
        || (decision == Some(Decision::Once)
            && row.as_ref().and_then(|r| r.fingerprint.as_deref()) == eval.fingerprint.as_deref());
    if covered {
        if write {
            let changed = write_grants(&home, &eval.keys);
            if is_process_home(&home) {
                let conn = db.lock();
                if let Err(e) = record_granted(&conn, &eval.scope, &changed) {
                    log_debug!("[folder-trust] {e}");
                }
            }
        }
        if eval.is_clean() && row.as_ref().is_some_and(|r| r.pending.is_some()) && door != Door::Check {
            let conn = db.lock();
            let _ = set_pending(&conn, &eval.scope, None);
        }
        let clean = eval.is_clean();
        return Outcome::Granted { scope: eval.scope, clean };
    }

    // Held: risky and no decision covers this fingerprint. Grants K2 wrote
    // earlier (a `once` folder that changed, a clean folder that became
    // risky) come off, so the CLIs' own prompt applies outside K2 too.
    if write {
        let conn = db.lock();
        revoke_k2(&conn, &eval.scope, &home);
        drop(conn);
        revoke_slots(&eval, &home);
    }
    let fp = eval.fingerprint.clone().unwrap_or_default();
    let existing = row.as_ref().and_then(|r| r.pending.clone());
    let pending = match door {
        Door::Add | Door::Spawn | Door::Request => Some(ask(&eval, actor, existing.as_ref(), &fp)),
        Door::Sweep | Door::Check => existing.filter(|p| p.fingerprint == fp),
    };
    // What still trusts the folder outside K2's own entries (the person's,
    // a parent's, 0.46.3 entries that can't be told from the person's):
    // shown, with how to remove it (round 2 M2).
    let leftover = leftover_grants(&home, &eval.keys, eval.in_git);
    Outcome::Held(refusal(PENDING, &eval, pending.as_ref(), leftover))
}

/// Pinned-login and wallet slot homes keep their own trust entries; on
/// hold and deny they lose this folder's too (review item 14). Only the
/// process user's slots.
fn revoke_slots(eval: &Evaluation, home: &Path) {
    if !is_process_home(home) {
        return;
    }
    for e in revoke_slot_homes(&eval.keys) {
        log_debug!("[folder-trust] slot home: {e}");
    }
}

/// Present the card unless one for this fingerprint is still open. Two
/// doors at once may both present: the daemon's card is deduped per scope
/// (`replace`), so one card stays pending. (No process lock here: callers
/// may hold the shared DB lock, and the presenter takes it too.)
fn ask(eval: &Evaluation, actor: &Actor, existing: Option<&Pending>, fp: &str) -> Pending {
    let db = crate::db::shared();
    let current = {
        let conn = db.lock();
        load_row(&conn, &eval.scope).ok().flatten().and_then(|r| r.pending)
    }
    .or_else(|| existing.cloned());
    let t = now();
    if let Some(p) = current.as_ref() {
        // An open card stands. An open owner ticket stands only until a
        // Thread exists to carry the one-tap card (a workspace added with no
        // pinned Chat gets its Chat at the first spawn): the presenter is
        // asked again and reuses the open ticket when there is still no
        // Thread.
        if p.fingerprint == fp && p.card.is_some() && pending_open(p) {
            return p.clone();
        }
    }
    let replaces = current.as_ref().filter(|p| p.fingerprint != fp);
    let asked = match PRESENTER.get() {
        Some(pr) => (pr.present)(&Ask { eval, actor, replaces }),
        None => Asked::default(),
    };
    let p = Pending {
        card: asked.card,
        conversation: asked.conversation,
        project: asked.project,
        fingerprint: fp.to_string(),
        ticket: asked.ticket,
        at: t,
    };
    let conn = db.lock();
    if let Err(e) = set_pending(&conn, &eval.scope, Some(&p)) {
        log_debug!("[folder-trust] {e}");
    }
    p
}

// ── Decide, reset, status ─────────────────────────────────────────────

/// The result of a person's decision.
#[derive(Debug, Clone, Serialize)]
pub struct DecideResult {
    pub path: String,
    pub scope: String,
    pub decision: Decision,
    /// Trust stands for the folder as it is now.
    pub granted: bool,
    /// Grants K2 didn't write that still trust the folder (after deny).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub leftover: Vec<String>,
    /// Grants K2 couldn't remove (after deny).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The card's result line.
    pub message: String,
}

/// Record the owner's decision for `path` (the caller checked it is the
/// owner: `require_action_card_person`). `card_fingerprint`: the
/// fingerprint the card showed; "Trust it" covers exactly that version,
/// never something the person didn't see. Grants are written (or K2's
/// removed) in the process HOME.
pub fn decide(path: &str, decision: Decision, by: &str, card_fingerprint: Option<&str>) -> Result<DecideResult, String> {
    let home = current_home().ok_or("no HOME")?;
    if skip_unisolated_home(&home) {
        return Err("test HOME is not isolated".into());
    }
    let home = canonical_abs(&home).unwrap_or(home);
    let eval = evaluate_in(path, &home).ok_or_else(|| format!("K2 never trusts {path} ($HOME, / or a parent of $HOME), so there is nothing to decide"))?;
    let fingerprint = match decision {
        Decision::Once => card_fingerprint.map(str::to_string).or_else(|| eval.fingerprint.clone()),
        Decision::Always | Decision::Deny => eval.fingerprint.clone(),
    };
    let db = crate::db::shared();
    {
        let conn = db.lock();
        set_decision(&conn, &eval.scope, Some(decision), fingerprint.as_deref(), Some(by))?;
    }
    log_debug!("[folder-trust] {} decided {} for {}", by, decision.as_str(), eval.scope);
    match decision {
        Decision::Deny => {
            let mut errors = {
                let conn = db.lock();
                revoke_k2(&conn, &eval.scope, &home)
            };
            if is_process_home(&home) {
                errors.extend(revoke_slot_homes(&eval.keys));
            }
            let leftover = leftover_grants(&home, &eval.keys, eval.in_git);
            let mut message = "Not trusted. K2 won't start programs in this folder (a plain shell tab still opens).".to_string();
            if !leftover.is_empty() {
                message.push_str(&format!(" Still trusted elsewhere: {}.", leftover.join("; ")));
            }
            if !errors.is_empty() {
                message.push_str(&format!(" K2 couldn't remove: {}.", errors.join("; ")));
            }
            Ok(DecideResult { path: eval.path, scope: eval.scope, decision, granted: false, leftover, errors, message })
        }
        Decision::Once | Decision::Always => {
            let covers = decision == Decision::Always || eval.is_clean() || fingerprint == eval.fingerprint;
            if covers {
                let changed = write_grants(&home, &eval.keys);
                let conn = db.lock();
                record_granted(&conn, &eval.scope, &changed)?;
            }
            let message = match (decision, covers) {
                (Decision::Always, _) => "Always trusted. Undo in Settings → Workspaces.".to_string(),
                (_, true) => "Trusted. K2 asks again if these settings change.".to_string(),
                (_, false) => "Trusted the version on the card, but these settings changed since; K2 asks again.".to_string(),
            };
            Ok(DecideResult { path: eval.path, scope: eval.scope, decision, granted: covers, leftover: Vec::new(), errors: Vec::new(), message })
        }
    }
}

/// Settings → Reset: back to undecided (K2 asks again at the next start).
/// K2's own grants come off when the folder is risky right now.
pub fn reset(path: &str, by: &str) -> Result<StatusView, String> {
    let home = current_home().ok_or("no HOME")?;
    if skip_unisolated_home(&home) {
        return Err("test HOME is not isolated".into());
    }
    let home = canonical_abs(&home).unwrap_or(home);
    let eval = evaluate_in(path, &home).ok_or_else(|| format!("K2 never trusts {path}, so there is nothing to reset"))?;
    let db = crate::db::shared();
    {
        let conn = db.lock();
        set_decision(&conn, &eval.scope, None, None, None)?;
        if !eval.is_clean() {
            revoke_k2(&conn, &eval.scope, &home);
        }
    }
    if !eval.is_clean() {
        revoke_slots(&eval, &home);
    }
    log_debug!("[folder-trust] {by} reset {}", eval.scope);
    status(path)
}

/// What [`forget`] removed.
#[derive(Debug, Clone, Serialize)]
pub struct Forgotten {
    /// One line per entry removed (tool, folder, file).
    pub removed: Vec<String>,
    /// Stores K2 couldn't change (never silent).
    pub errors: Vec<String>,
    pub status: StatusView,
}

/// Remove every trust entry for `path`, in every CLI's store, whoever
/// wrote it (review round 2 M2): K2's, 0.46.3's, the person's own, and
/// parent folders' entries a CLI would inherit. The owner's command for
/// the leftovers a hold or "Don't trust" reports. The decision is kept:
/// a folder K2 trusts gets K2's own entries back at its next start.
pub fn forget(path: &str, by: &str) -> Result<Forgotten, String> {
    let home = current_home().ok_or("no HOME")?;
    if skip_unisolated_home(&home) {
        return Err(SKIP_TEST_HOME.into());
    }
    let home = canonical_abs(&home).unwrap_or(home);
    let eval = evaluate_in(path, &home).ok_or_else(|| format!("K2 never trusts {path} ($HOME, / or a parent of $HOME)"))?;
    let mut errors = {
        let db = crate::db::shared();
        let conn = db.lock();
        revoke_k2(&conn, &eval.scope, &home)
    };
    let (removed, more) = super::forget_grants(&home, &eval.keys);
    errors.extend(more);
    log_debug!("[folder-trust] {by} forgot {} ({} entries, {} errors)", eval.scope, removed.len(), errors.len());
    Ok(Forgotten { removed, errors, status: status(path)? })
}

/// What the person saw when they chose: the card's (or the CLI's status)
/// folder, scope and fingerprint.
#[derive(Debug, Clone, Copy, Default)]
pub struct Seen<'a> {
    pub path: Option<&'a str>,
    pub scope: Option<&'a str>,
    pub fingerprint: Option<&'a str>,
}

/// Why [`decide_checked`] didn't record a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecideError {
    /// The folder isn't what the person saw any more (path, scope or
    /// settings): nothing was recorded; ask again.
    Changed(String),
    Failed(String),
}

/// [`decide`] only when the folder still is what the person saw (reviews
/// B#2, B#3): it resolves to the same path and scope, and its fingerprint
/// is the one shown. This holds for Trust it, Always and Don't trust
/// alike; on a mismatch nothing is recorded.
pub fn decide_checked(path: &str, decision: Decision, by: &str, seen: Seen<'_>) -> Result<DecideResult, DecideError> {
    let home = current_home().ok_or_else(|| DecideError::Failed("no HOME".into()))?;
    let home = canonical_abs(&home).unwrap_or(home);
    let eval = evaluate_in(path, &home)
        .ok_or_else(|| DecideError::Failed(format!("K2 never trusts {path} ($HOME, / or a parent of $HOME), so there is nothing to decide")))?;
    if let Some(p) = seen.path {
        if p != eval.path {
            return Err(DecideError::Changed(format!("{p} now resolves to {}", eval.path)));
        }
    }
    if let Some(s) = seen.scope {
        if s != eval.scope {
            return Err(DecideError::Changed(format!("{} now belongs to {} (was {s})", eval.path, eval.scope)));
        }
    }
    if seen.fingerprint != eval.fingerprint.as_deref() {
        let now = if eval.is_clean() { "it has no risky settings now".to_string() } else { format!("{} finding(s) now", eval.findings.len()) };
        return Err(DecideError::Changed(format!("the settings in {} changed since you saw them ({now})", eval.path)));
    }
    decide(&eval.path, decision, by, seen.fingerprint).map_err(DecideError::Failed)
}

/// One folder's trust as Settings and `k2 workspace trust status` show it.
#[derive(Debug, Clone, Serialize)]
pub struct StatusView {
    pub path: String,
    pub scope: String,
    /// `clean` | `trusted` (once, this version) | `always` | `denied` |
    /// `pending` (a card is waiting) | `undecided` (risky, nobody asked
    /// yet) | `changed` (trusted once, then the settings changed).
    pub state: String,
    pub decision: Option<Decision>,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub findings: Vec<Finding>,
    pub more: usize,
    pub fingerprint: Option<String>,
    pub card: Option<String>,
    pub ticket: Option<String>,
    /// The entries K2 wrote (removed by "Don't trust").
    pub k2_granted: GrantedKeys,
    /// Every grant that trusts this folder now (K2's and the rest).
    pub trusted_by: Vec<String>,
    pub decide: String,
    /// How the owner removes `trusted_by` (when there is any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forget: Option<String>,
}

/// Status of one folder (no grant, no card).
pub fn status(path: &str) -> Result<StatusView, String> {
    let home = current_home().ok_or("no HOME")?;
    let home = canonical_abs(&home).unwrap_or(home);
    let eval = evaluate_in(path, &home).ok_or_else(|| format!("K2 never trusts {path} ($HOME, / or a parent of $HOME)"))?;
    let row = {
        let db = crate::db::shared();
        let conn = db.lock();
        load_row(&conn, &eval.scope)?
    };
    let decision = row.as_ref().and_then(|r| r.decision);
    let pending = row.as_ref().and_then(|r| r.pending.clone()).filter(|p| Some(&p.fingerprint) == eval.fingerprint.as_ref() && pending_open(p));
    let state = match decision {
        Some(Decision::Deny) => "denied",
        Some(Decision::Always) => "always",
        _ if eval.is_clean() => "clean",
        Some(Decision::Once) if row.as_ref().and_then(|r| r.fingerprint.as_deref()) == eval.fingerprint.as_deref() => "trusted",
        Some(Decision::Once) => "changed",
        None if pending.is_some() => "pending",
        None => "undecided",
    };
    let state = if state == "changed" && pending.is_some() { "pending" } else { state };
    let trusted_by = leftover_grants(&home, &eval.keys, eval.in_git);
    Ok(StatusView {
        path: eval.path.clone(),
        scope: eval.scope.clone(),
        state: state.to_string(),
        decision,
        decided_by: row.as_ref().and_then(|r| r.decided_by.clone()),
        decided_at: row.as_ref().and_then(|r| r.decided_at),
        more: eval.findings.len().saturating_sub(MAX_LISTED),
        findings: eval.findings.iter().take(MAX_LISTED).cloned().collect(),
        fingerprint: eval.fingerprint.clone(),
        card: pending.as_ref().and_then(|p| p.card.clone()),
        ticket: pending.as_ref().and_then(|p| p.ticket.clone()),
        k2_granted: row.as_ref().map(|r| r.granted.clone()).unwrap_or_default(),
        trusted_by: trusted_by.clone(),
        decide: decide_hint(&eval.path),
        forget: if trusted_by.is_empty() { None } else { Some(forget_hint(&eval.path)) },
    })
}

/// The decision scope for `path` (`None` for a refused path).
pub fn scope_for(path: &str) -> Option<String> {
    let home = current_home()?;
    let cwd = canonical_abs(Path::new(path.trim()))?;
    let home = canonical_abs(&home).unwrap_or(home);
    if is_refused_path(&cwd, &home) {
        return None;
    }
    Some(main_checkout(&cwd, &home).unwrap_or(cwd).to_string_lossy().to_string())
}

/// The fingerprint a pending card for `scope` was presented for (the
/// card's answer applies that one).
pub fn pending_of(scope: &str) -> Option<Pending> {
    let db = crate::db::shared();
    let conn = db.lock();
    load_row(&conn, scope).ok().flatten().and_then(|r| r.pending)
}

// ── Upgrade from 0.46.3 (A4, Q6) ──────────────────────────────────────

/// A folder trusted before K2 checked, now risky (the upgrade summary).
#[derive(Debug, Clone, Serialize)]
pub struct UpgradedFolder {
    pub path: String,
    pub scope: String,
    pub findings: Vec<Finding>,
    pub more: usize,
}

/// Finish what migration 0223 started: for each `upgrade = 1` row (a
/// 0.46.3 registered path), re-key it by scope, record every grant that is
/// in place now as K2's (0.46.3 wrote all of them, A4), and mark a folder
/// that is risky right now `once` with its current fingerprint
/// (`decided_by = 'upgrade'`), so nothing breaks today. Returns the risky
/// ones for the owner's one summary. Idempotent: rows end `upgrade = 0`.
pub fn upgrade_pass() -> Vec<UpgradedFolder> {
    upgrade_rows();
    let mut g = UPGRADED.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *g)
}

/// Folders the upgrade marked, waiting for the boot pass's summary (a door
/// may mark its own folder first).
static UPGRADED: Mutex<Vec<UpgradedFolder>> = Mutex::new(Vec::new());

fn note_upgraded(found: Option<UpgradedFolder>) {
    if let Some(f) = found {
        let mut g = UPGRADED.lock().unwrap_or_else(|e| e.into_inner());
        if !g.iter().any(|u| u.scope == f.scope) {
            g.push(f);
        }
    }
}

/// One row's marks inside a savepoint (review B#9): a row ends
/// `upgrade = 0` only with all of its marks, so an error leaves it to be
/// redone, and redoing is idempotent. The caller holds the DB lock, never
/// across a scan (`eval` is already computed; round 2 M4).
fn upgrade_row_locked(conn: &Connection, home: &Path, old: &Row, eval: Option<&Evaluation>) {
    let still: bool = conn
        .query_row("SELECT EXISTS (SELECT 1 FROM folder_trust WHERE scope = ?1 AND upgrade = 1)", params![old.scope], |r| r.get(0))
        .unwrap_or(false);
    if !still {
        return; // another door or pass did it
    }
    if let Err(e) = conn.execute_batch("SAVEPOINT folder_trust_upgrade") {
        log_debug!("[folder-trust] upgrade {}: savepoint: {e}", old.scope);
        return;
    }
    match upgrade_one(conn, home, old, eval) {
        Ok(found) => match conn.execute_batch("RELEASE folder_trust_upgrade") {
            Ok(()) => note_upgraded(found),
            Err(e) => log_debug!("[folder-trust] upgrade {}: release: {e}", old.scope),
        },
        Err(e) => {
            log_debug!("[folder-trust] upgrade {}: {e} (left for the next pass)", old.scope);
            let _ = conn.execute_batch("ROLLBACK TO folder_trust_upgrade; RELEASE folder_trust_upgrade");
        }
    }
}

/// A door's own folder: finish its 0.46.3 upgrade rows inline, with the
/// scan the door already made, so a folder 0.46.3 trusted is never held
/// because the boot pass hasn't reached it yet.
fn upgrade_rows_for(eval: &Evaluation, home: &Path) {
    let db = crate::db::shared();
    let conn = db.lock();
    let rows: Vec<Row> = {
        let Ok(mut stmt) = conn.prepare(&format!("SELECT {ROW_COLS} FROM folder_trust WHERE upgrade = 1 AND (scope = ?1 OR scope = ?2)")) else {
            return;
        };
        let rows = stmt.query_map(params![eval.scope, eval.path], row_from).map(|it| it.filter_map(|r| r.ok()).collect::<Vec<_>>());
        rows.unwrap_or_default()
    };
    for old in rows {
        upgrade_row_locked(&conn, home, &old, Some(eval));
    }
}

/// The boot pass: list the pending rows, then for each scan with no lock
/// held and write under a short lock.
fn upgrade_rows() {
    let Some(home) = current_home() else { return };
    if skip_unisolated_home(&home) {
        return;
    }
    let home = canonical_abs(&home).unwrap_or(home);
    let db = crate::db::shared();
    let raw: Vec<Row> = {
        let conn = db.lock();
        let mut stmt = match conn.prepare(&format!("SELECT {ROW_COLS} FROM folder_trust WHERE upgrade = 1")) {
            Ok(s) => s,
            Err(e) => {
                log_debug!("[folder-trust] upgrade read: {e}");
                return;
            }
        };
        let rows = stmt.query_map([], row_from).map(|it| it.filter_map(|r| r.ok()).collect::<Vec<_>>());
        rows.unwrap_or_default()
    };
    for old in raw {
        let eval = evaluate_in(&old.scope, &home);
        let conn = db.lock();
        upgrade_row_locked(&conn, &home, &old, eval.as_ref());
    }
}

/// One upgrade row's marks (inside the caller's savepoint).
fn upgrade_one(conn: &Connection, home: &Path, old: &Row, eval: Option<&Evaluation>) -> Result<Option<UpgradedFolder>, String> {
    let db_err = |e: rusqlite::Error| format!("folder_trust write: {e}");
    let Some(eval) = eval else {
        conn.execute("DELETE FROM folder_trust WHERE scope = ?1 AND upgrade = 1", params![old.scope]).map_err(db_err)?;
        return Ok(None);
    };
    if eval.scope != old.scope {
        conn.execute("DELETE FROM folder_trust WHERE scope = ?1 AND upgrade = 1", params![old.scope]).map_err(db_err)?;
    }
    ensure_row(conn, &eval.scope)?;
    // Only entries that look like K2 wrote them (never the person's own).
    record_granted(conn, &eval.scope, &upgrade_k2_grants(home, &eval.keys))?;
    let current = load_row(conn, &eval.scope)?;
    let mut found = None;
    if !eval.is_clean() && current.as_ref().and_then(|r| r.decision).is_none() {
        set_decision(conn, &eval.scope, Some(Decision::Once), eval.fingerprint.as_deref(), Some("upgrade"))?;
        found = Some(UpgradedFolder {
            path: eval.path.clone(),
            scope: eval.scope.clone(),
            more: eval.findings.len().saturating_sub(MAX_LISTED),
            findings: eval.findings.iter().take(MAX_LISTED).cloned().collect(),
        });
    }
    conn.execute("UPDATE folder_trust SET upgrade = 0 WHERE scope = ?1", params![eval.scope]).map_err(db_err)?;
    Ok(found)
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
