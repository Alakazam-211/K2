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
//! git repos up to [`NESTED_MAX_DEPTH`] levels below the cwd: Gemini and
//! Cursor let a nested clone inherit the parent's trust, so K2 trusting
//! the parent trusts them too. Nested findings are kept for the CLIs that
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
    canonical_abs, current_home, git_common_workdir, grant_keys, is_refused_path, leftover_grants,
    present_grants, revoke_grants, skip_unisolated_home, write_grants, GrantedKeys,
};

/// The 409 code while a card is waiting (or nobody decided yet).
pub const PENDING: &str = "folder_trust_pending";
/// The 409 code after "Don't trust".
pub const DENIED: &str = "folder_trust_denied";
/// Findings listed in a refusal or a card; the rest are counted.
pub const MAX_LISTED: usize = 20;
/// Nested repos: how deep below a scanned root, and how many folders.
pub const NESTED_MAX_DEPTH: usize = 3;
const NESTED_MAX_DIRS: usize = 400;
const NESTED_SKIP: &[&str] = &["node_modules", "target", "vendor", "dist", "build", "venv", "__pycache__"];
/// Folders between the cwd and its work tree top that are scanned.
const MAX_BETWEEN: usize = 8;
/// Tools whose settings a nested clone inherits through the parent's
/// trust (U0 Q4: Gemini and Cursor; Cursor also runs `.claude` hooks).
const NESTED_TOOLS: &[&str] = &["gemini", "cursor", "claude"];
/// The marker a refusal leaves in an error string (see [`Refusal::to_error_string`]).
const TOKEN_OPEN: &str = "[k2ft:";

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
    /// Denied: grants K2 didn't write that still trust this folder.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub leftover: Vec<String>,
}

impl Refusal {
    /// The id the error-string token carries (state + scope).
    fn token_id(&self) -> String {
        let h = format!("{:x}", Sha256::digest(self.scope.as_bytes()));
        let state = if self.error == DENIED { "d" } else { "p" };
        format!("{state}{}", &h[..20])
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

/// The work tree that holds `cwd` (its own top for a worktree).
fn own_worktree_top(cwd: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let w = repo.workdir()?;
    canonical_abs(w).or_else(|| Some(w.to_path_buf()))
}

/// Git repos below `root` (a `.git` file or folder), up to
/// [`NESTED_MAX_DEPTH`] levels, skipping hidden and build folders.
fn nested_repos(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || NESTED_SKIP.contains(&name.as_str()) {
                continue;
            }
            visited += 1;
            if visited > NESTED_MAX_DIRS {
                return out;
            }
            let p = e.path();
            if std::fs::symlink_metadata(p.join(".git")).is_ok() {
                out.push(p.clone());
            }
            if depth + 1 < NESTED_MAX_DEPTH {
                stack.push((p, depth + 1));
            }
        }
    }
    out.sort();
    out
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
    let top = own_worktree_top(&cwd);
    let in_git = top.is_some();
    let common = git_common_workdir(&cwd);
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
        let tracked = |rel: &Path| git_tracked(root, rel);
        let r = scan::scan_folder(root, &tracked);
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
        for repo in nested_repos(&base) {
            if uniq.contains(&repo) || seen_nested.contains(&repo) {
                continue;
            }
            seen_nested.push(repo.clone());
            let tracked = |rel: &Path| git_tracked(&repo, rel);
            let r = scan::scan_folder(&repo, &tracked);
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

/// One finding as a card / message line: "`file`: detail".
pub fn finding_line(f: &Finding) -> String {
    format!("{}: {}", f.file, f.detail)
}

fn refusal(code: &str, eval: &Evaluation, pending: Option<&Pending>, leftover: Vec<String>) -> Refusal {
    let more = eval.findings.len().saturating_sub(MAX_LISTED);
    let first = eval.findings.first().map(finding_line).unwrap_or_default();
    let message = if code == DENIED {
        format!(
            "K2 won't start AI tools in {}: the owner chose \"Don't trust\" for its settings ({first}). To change it: Settings → Workspaces → Folder trust → Reset, or {}.",
            eval.path,
            decide_hint(&eval.path)
        )
    } else {
        format!(
            "Waiting for the owner to trust {}: it has settings that run commands when AI tools start ({first}{}). Answer the card on Thread, or: {}.",
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
    }
}

// ── The gate ──────────────────────────────────────────────────────────

/// Run the gate for `path`: scan, look up the decision, then grant
/// (writing into `home`, default the process HOME), hold or deny. See the
/// module doc for the rules.
pub fn ensure(path: &str, door: Door, actor: &Actor, home: Option<&Path>) -> Outcome {
    ensure_inner(path, door, actor, home, door != Door::Check)
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
        return Outcome::Skipped("test HOME is not isolated".into());
    }
    let home = canonical_abs(&home).unwrap_or(home);
    // A 0.46.3 database's rows wait for the upgrade marks (A4): a spawn
    // that comes before the boot pass runs them first, so a folder 0.46.3
    // already trusted is never held by the race.
    upgrade_if_pending();
    let Some(eval) = evaluate_in(path, &home) else {
        return Outcome::Skipped(format!("refused path {path}"));
    };
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
    }
    let fp = eval.fingerprint.clone().unwrap_or_default();
    let existing = row.as_ref().and_then(|r| r.pending.clone());
    let pending = match door {
        Door::Add | Door::Spawn => Some(ask(&eval, actor, existing.as_ref(), &fp)),
        Door::Sweep | Door::Check => existing.filter(|p| p.fingerprint == fp),
    };
    Outcome::Held(refusal(PENDING, &eval, pending.as_ref(), Vec::new()))
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
            let errors = {
                let conn = db.lock();
                revoke_k2(&conn, &eval.scope, &home)
            };
            let leftover = leftover_grants(&home, &eval.keys, eval.in_git);
            let mut message = "Not trusted. K2 won't start AI tools in this folder.".to_string();
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
    log_debug!("[folder-trust] {by} reset {}", eval.scope);
    status(path)
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
        trusted_by: leftover_grants(&home, &eval.keys, eval.in_git),
        decide: decide_hint(&eval.path),
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
    Some(
        git_common_workdir(&cwd)
            .or_else(|| own_worktree_top(&cwd))
            .unwrap_or(cwd)
            .to_string_lossy()
            .to_string(),
    )
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
    upgrade_if_pending();
    let mut g = UPGRADED.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *g)
}

/// Folders the upgrade marked, waiting for the boot pass's summary (the
/// marks may run first from a spawn).
static UPGRADED: Mutex<Vec<UpgradedFolder>> = Mutex::new(Vec::new());

/// Run the upgrade marks once there are `upgrade = 1` rows. Serialized by
/// the shared DB's own (reentrant) lock, held for the whole pass: a caller
/// that already holds it (a register door) runs it inline, and no second
/// lock can deadlock against it.
fn upgrade_if_pending() {
    let db = crate::db::shared();
    let conn = db.lock();
    let pending: bool = conn
        .query_row("SELECT EXISTS (SELECT 1 FROM folder_trust WHERE upgrade = 1)", [], |r| r.get(0))
        .unwrap_or(false);
    if !pending {
        return;
    }
    let found = upgrade_rows();
    drop(conn);
    if !found.is_empty() {
        UPGRADED.lock().unwrap_or_else(|e| e.into_inner()).extend(found);
    }
}

fn upgrade_rows() -> Vec<UpgradedFolder> {
    let Some(home) = current_home() else { return Vec::new() };
    if skip_unisolated_home(&home) {
        return Vec::new();
    }
    let home = canonical_abs(&home).unwrap_or(home);
    let db = crate::db::shared();
    let raw: Vec<Row> = {
        let conn = db.lock();
        let mut stmt = match conn.prepare(&format!("SELECT {ROW_COLS} FROM folder_trust WHERE upgrade = 1")) {
            Ok(s) => s,
            Err(e) => {
                log_debug!("[folder-trust] upgrade read: {e}");
                return Vec::new();
            }
        };
        let rows = stmt.query_map([], row_from).map(|it| it.filter_map(|r| r.ok()).collect::<Vec<_>>());
        rows.unwrap_or_default()
    };
    let mut out = Vec::new();
    for old in raw {
        let eval = evaluate_in(&old.scope, &home);
        let conn = db.lock();
        let Some(eval) = eval else {
            let _ = conn.execute("DELETE FROM folder_trust WHERE scope = ?1 AND upgrade = 1", params![old.scope]);
            continue;
        };
        let present = present_grants(&home, &eval.keys);
        if eval.scope != old.scope {
            let _ = conn.execute("DELETE FROM folder_trust WHERE scope = ?1 AND upgrade = 1", params![old.scope]);
        }
        if ensure_row(&conn, &eval.scope).is_err() {
            continue;
        }
        let _ = conn.execute("UPDATE folder_trust SET upgrade = 0 WHERE scope = ?1", params![eval.scope]);
        if let Err(e) = record_granted(&conn, &eval.scope, &present) {
            log_debug!("[folder-trust] upgrade {}: {e}", eval.scope);
        }
        let current = load_row(&conn, &eval.scope).ok().flatten();
        if !eval.is_clean() && current.as_ref().and_then(|r| r.decision).is_none() {
            if let Err(e) = set_decision(&conn, &eval.scope, Some(Decision::Once), eval.fingerprint.as_deref(), Some("upgrade")) {
                log_debug!("[folder-trust] upgrade {}: {e}", eval.scope);
                continue;
            }
            if !out.iter().any(|u: &UpgradedFolder| u.scope == eval.scope) {
                out.push(UpgradedFolder {
                    path: eval.path.clone(),
                    scope: eval.scope.clone(),
                    more: eval.findings.len().saturating_sub(MAX_LISTED),
                    findings: eval.findings.iter().take(MAX_LISTED).cloned().collect(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
