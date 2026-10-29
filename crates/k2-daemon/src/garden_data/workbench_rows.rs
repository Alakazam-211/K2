//! The Workbench's Garden rows (0.46.7, catalog v40, `workbench-v1`;
//! research-t3code-ui-as-garden-v1 + vs-live B4/B5).
//!
//! Garden-local rows a widget calls through `POST /cli/zen/call`, answered
//! here as the person on this computer (owner token only, `call::handle`):
//!
//! | row | cap | what |
//! |---|---|---|
//! | `work.list` | `agents:read` | every sidecar here, with its worktree, branch, ahead / dirty |
//! | `git.snapshot` | `git:read` | an agent's branch and changed files (vs its base) |
//! | `git.diffFile` | `git:read` | one file's two sides, for the `k2-diff` kit |
//! | `git.log` | `git:read` | commits on its branch the base doesn't have |
//! | `compute.jobs` | `compute:read` | compute jobs for its branch |
//! | `git.note` | `git:notes` | review notes, sent to that agent's own Thread |
//! | `agents.start` | `agents:start` | a new worktree agent (K2's sheet first) |
//! | `agents.stop` | `agents:stop` | stop its turn |
//! | `git.land` | `agents:start` + `git:read` | land its branch (K2's sheet first) |
//! | `agents.settle` | `agents:start` + `git:read` | retire / discard (K2's sheet first) |
//!
//! **Addresses.** A row's address is `ws::local` (a workspace's main
//! agent) or `ws/name::local` (a sidecar). Only this computer's agents:
//! anything else is `not_bound`, the same answer as an agent that isn't
//! there.
//!
//! **No host paths.** Files are relative to the agent's worktree; a
//! worktree is relative to its workspace (`.worktrees/k2-fix`).
//!
//! **K2's sheet.** `agents.start`, `git.land` and `agents.settle` first
//! answer `confirm_required` with the change (what the sheet shows) and a
//! one-time `secretId` bound to this placement, verb and arguments; K2's
//! renderer (never widget code: the frame never sees the id) shows the
//! sheet and repeats the call with `confirmToken` only after the person's
//! press there. The action itself runs through `workbench_w1` (Lane W1's
//! routes; stubs until they land).
//!
//! Every row needs its cap, the signer gate, the placement's budget; writes
//! need an unpaused placement and leave an audit line. The renderer adds
//! the press-within-5-s rule (`zen-custom-work.ts`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value as J};

use k2_core::contract::{catalog, Effect, VerbRow};

use super::budget::{self, BudgetKey, Spend};
use super::call::{resolve, CallBody, Placed};
use super::person_rows::{args_of, no, passthrough, project};
use super::workbench_w1 as w1;
use crate::cli_response::CliResponse;

/// The rows this file answers, in catalog order.
pub const VERBS: [&str; 10] = [
    "work.list",
    "git.snapshot",
    "git.diffFile",
    "git.log",
    "compute.jobs",
    "agents.start",
    "agents.stop",
    "git.land",
    "agents.settle",
    "git.note",
];

pub fn is_verb(verb: &str) -> bool {
    VERBS.contains(&verb)
}

/// The only host a Workbench row reaches (`LOCAL_HOME_HOST`).
const LOCAL_HOST: &str = "local";
/// How long K2's sheet may stay open before its answer goes stale.
const CONFIRM_TTL: Duration = Duration::from_secs(300);
/// At most this many notes per `git.note`, each at most this long.
const NOTES_MAX: usize = 50;
const NOTE_CHARS: usize = 4_000;
const BRIEF_CHARS: usize = 20_000;

pub fn handle(b: &CallBody) -> CliResponse {
    match run(b) {
        Ok(v) => crate::garden_data::ok(json!({ "ok": true, "result": v })),
        Err(r) => r,
    }
}

fn run(b: &CallBody) -> Result<J, CliResponse> {
    let row = catalog().verb(&b.verb).ok_or_else(|| no(400, "unknown_verb", format!("no verb {}", b.verb)))?;
    let w = resolve(b).map_err(|e| crate::garden_data::response(&e))?;
    for cap in row.cap.iter().chain(row.also_caps.iter()) {
        crate::garden_data::call::check_cap(&w, &b.verb, cap).map_err(|e| crate::garden_data::response(&e))?;
    }
    // Garden access: a Workbench row acts as the person, so only a version
    // that passed the signer gate makes one (k2:* built-ins always do).
    if !w.signed {
        return Err(crate::garden_data::response(&crate::garden_data::call::needs_editor(&w)));
    }
    let write = row.effect == Effect::Write;
    if write && crate::zen_routes::placement_pause(&w.garden, &w.placement).is_some() {
        return Err(no(423, "paused", "this widget is paused: too many writes. The person can Resume it on the widget"));
    }
    let key = BudgetKey::Placement { garden: w.garden.clone(), placement: w.placement.clone() };
    budget::spend(&key, if write { Spend::Write } else { Spend::Read }).map_err(|e| crate::garden_data::response(&e))?;
    let args = args_of(row, &b.args)?;
    let v = answer(row, &w, &args, b)?;
    if write {
        crate::garden_data::audit::line(
            "zen.workbench.write",
            &format!("garden:{}", w.widget),
            &format!("verb={} placement={}/{} hash={} by=owner address={}", b.verb, w.garden, w.placement, w.hash, str_arg(&args, "address")),
        );
    }
    Ok(project(&row.response, &v, &BTreeMap::new()))
}

fn str_arg<'a>(args: &'a Map<String, J>, k: &str) -> &'a str {
    args.get(k).and_then(J::as_str).map(str::trim).unwrap_or("")
}

fn bad_args(hint: impl Into<String>) -> CliResponse {
    no(400, "bad_args", hint)
}

// ── Targets ───────────────────────────────────────────────────────────

/// One agent on this computer a Workbench row acts on.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub project_id: String,
    pub ws_handle: String,
    pub ws_label: String,
    pub ws_path: String,
    /// The sidecar (its handle and tab row), or `None` for the main agent.
    pub sidecar: Option<k2_core::sidecar::SidecarRow>,
    /// The folder git reads: the sidecar's worktree, else the workspace.
    pub dir: String,
}

impl Target {
    pub fn handle(&self) -> Option<&str> {
        self.sidecar.as_ref().and_then(|s| s.handle.as_deref())
    }

    /// `ws` or `ws/name` (the Thread address).
    pub fn thread(&self) -> String {
        k2_core::workspace_session_handles::format_address(&self.ws_handle, self.handle())
    }

    /// `ws::local` or `ws/name::local` (the Garden row address).
    pub fn address(&self) -> String {
        format!("{}::{LOCAL_HOST}", self.thread())
    }

    /// The worktree relative to the workspace, when the agent works in one.
    pub fn worktree(&self) -> Option<String> {
        relative_inside(&self.dir, &self.ws_path).filter(|r| !r.is_empty())
    }
}

fn canon(p: &str) -> PathBuf {
    dunce::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p))
}

/// `child` relative to `parent` ("" for the same folder), or `None` when it
/// isn't inside.
fn relative_inside(child: &str, parent: &str) -> Option<String> {
    let c = canon(child);
    let p = canon(parent);
    c.strip_prefix(&p).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
}

fn not_bound(address: &str) -> CliResponse {
    no(403, "not_bound", format!("'{address}' isn't an agent on this computer this widget reaches"))
}

/// `ws[/name][::local]` → the agent, or `not_bound`.
pub(crate) fn target(address: &str) -> Result<Target, CliResponse> {
    let raw = address.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Err(bad_args("needs an agent's address (from agents.list() or work.list())"));
    }
    let token = match raw.split_once("::") {
        Some((t, host)) if host == LOCAL_HOST => t.to_string(),
        Some(_) => return Err(not_bound(&raw)),
        None => raw.clone(),
    };
    let (ws_token, name) = match token.split_once('/') {
        Some((w, n)) if !w.is_empty() && !n.is_empty() && !n.contains('/') => (w.to_string(), Some(n.to_string())),
        Some(_) => return Err(not_bound(&raw)),
        None => (token.clone(), None),
    };
    let ws = crate::fs_routes::resolve_owner_workspace(&ws_token).map_err(|_| not_bound(&raw))?;
    let label = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, &ws.project_id).map(|p| p.name).unwrap_or_else(|_| ws.handle.clone())
    };
    let mut t = Target {
        project_id: ws.project_id.clone(),
        ws_handle: ws.handle.clone(),
        ws_label: label,
        ws_path: ws.path.clone(),
        sidecar: None,
        dir: ws.path.clone(),
    };
    if let Some(name) = name {
        let rows = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::sidecar::list_rows(&conn, &ws.project_id).map_err(|e| no(500, "failed", e))?
        };
        let row = rows.into_iter().find(|r| r.handle.as_deref() == Some(name.as_str())).ok_or_else(|| not_bound(&raw))?;
        t.dir = sidecar_dir(&row, &ws.path);
        t.sidecar = Some(row);
    }
    Ok(t)
}

/// A sidecar's folder: its tab row's cwd when that is a folder inside its
/// workspace (a worktree), else the workspace itself.
fn sidecar_dir(row: &k2_core::sidecar::SidecarRow, ws_path: &str) -> String {
    match row.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(cwd) if Path::new(cwd).is_dir() && relative_inside(cwd, ws_path).is_some() => cwd.to_string(),
        _ => ws_path.to_string(),
    }
}

// ── git helpers ───────────────────────────────────────────────────────

fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    let out = k2_core::git_safe::git(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The branch a workspace integrates on: its main checkout's branch (W1's
/// recorded base wins when it has one).
fn default_base(t: &Target) -> String {
    if let Some(b) = t.sidecar.as_ref().and_then(|s| w1::task_facts(&t.project_id, &s.pane_group_id).base) {
        return b;
    }
    git_out(&t.ws_path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).filter(|b| !b.is_empty()).unwrap_or_else(|| "main".to_string())
}

/// A ref a widget names: a plain branch or commit name, never an option.
fn safe_ref(r: &str) -> Result<String, CliResponse> {
    let r = r.trim();
    let ok = !r.is_empty()
        && r.len() <= 200
        && !r.starts_with('-')
        && !r.contains("..")
        && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'));
    if ok {
        Ok(r.to_string())
    } else {
        Err(bad_args(format!("'{r}' isn't a branch name")))
    }
}

fn base_of(t: &Target, args: &Map<String, J>) -> Result<String, CliResponse> {
    match args.get("base").and_then(J::as_str) {
        Some(b) => safe_ref(b),
        None => Ok(default_base(t)),
    }
}

fn changes_refusal(e: k2_core::git_changes::ChangesError) -> CliResponse {
    use k2_core::git_changes::codes;
    match e.code {
        c if c == codes::NOT_A_REPO => no(400, "not_a_repo", e.hint),
        c if c == codes::NOT_FOUND => no(404, "not_found", e.hint),
        c if c == codes::BAD_PATH || c == codes::BAD_REF || c == codes::BAD_REQUEST => no(400, "bad_args", e.hint),
        _ => no(500, "failed", e.hint),
    }
}

fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

/// The snapshot as a widget sees it (GitStatus): relative paths only.
fn status_json(t: &Target, base: &str) -> Result<J, CliResponse> {
    let snap = k2_core::git_changes::snapshot(&t.dir, Some(base)).map_err(changes_refusal)?;
    let Some(s) = snap.repo() else {
        return Err(no(400, "not_a_repo", format!("{} isn't in a git repository", t.thread())));
    };
    let files: Vec<J> = s
        .files
        .iter()
        .map(|f| {
            json!({
                "file": f.path, "oldFile": f.old_path, "group": f.group, "status": f.status,
                "additions": f.additions, "deletions": f.deletions, "binary": f.binary,
            })
        })
        .collect();
    Ok(json!({
        "branch": s.branch,
        "head": s.head_oid.as_deref().map(short),
        "state": s.state,
        "base": s.base.as_ref().map(|b| json!({ "ref": b.ref_name, "ahead": b.ahead, "behind": b.behind })),
        "counts": s.counts,
        "truncated": s.truncated,
        "rev": s.rev,
        "files": files,
    }))
}

/// Commits on HEAD that `base` doesn't have, newest first.
fn log_commits(dir: &str, base: &str, limit: usize) -> Result<Vec<J>, CliResponse> {
    let range = format!("{base}..HEAD");
    let n = format!("-n{limit}");
    let out = k2_core::git_safe::git(dir)
        .args(["log", "--no-color", n.as_str(), "--format=%H%x1f%h%x1f%s%x1f%an%x1f%at", range.as_str(), "--"])
        .output()
        .map_err(|e| no(500, "failed", format!("git log: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("not a git repository") {
            return Err(no(400, "not_a_repo", "this agent's folder isn't in a git repository"));
        }
        return Err(bad_args(format!("git log {range}: {err}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 5).then(|| {
                json!({ "sha": p[0], "short": p[1], "subject": p[2], "author": p[3], "at": p[4].parse::<i64>().unwrap_or(0) })
            })
        })
        .collect())
}

fn ahead_count(dir: &str, base: &str) -> Option<i64> {
    git_out(dir, &["rev-list", "--count", format!("{base}..HEAD").as_str(), "--"]).and_then(|s| s.parse().ok())
}

// ── reads ─────────────────────────────────────────────────────────────

/// One sidecar as a Workbench task row (WorkRow).
fn work_row(t: &Target, with_git: bool) -> J {
    let s = t.sidecar.as_ref().expect("a sidecar");
    let facts = w1::task_facts(&t.project_id, &s.pane_group_id);
    let live = crate::v2_session_map::lookup_by_agent_name(&s.agent_name).is_some_and(|x| x.is_child_alive());
    let worktree = t.worktree();
    // Git facts only for an agent in its own worktree: a sidecar at the
    // workspace root shares the main checkout, so its "changes" aren't its own.
    let (branch, base, head, ahead, behind, dirty) = match worktree.as_ref().filter(|_| with_git) {
        Some(_) => {
            let base = default_base(t);
            match k2_core::git_changes::snapshot(&t.dir, Some(&base)).ok().and_then(|r| r.repo()) {
                Some(snap) => {
                    let (ahead, behind) = snap.base.as_ref().map(|b| (b.ahead as i64, b.behind as i64)).unzip();
                    let dirty = (snap.counts.staged + snap.counts.unstaged + snap.counts.untracked + snap.counts.conflicted) as i64;
                    (snap.branch.clone(), Some(base), snap.head_oid.as_deref().map(short), ahead, behind, Some(dirty))
                }
                None => (None, Some(base), None, None, None, None),
            }
        }
        None => (None, None, None, None, None, None),
    };
    json!({
        "address": t.address(),
        "thread": t.thread(),
        "workspace": t.ws_handle,
        "workspaceLabel": t.ws_label,
        // The reach route's rename-repair hint (a widget never sees it: the
        // catalog's WorkRow is closed).
        "workspaceId": t.project_id,
        "name": s.name.clone().or_else(|| s.handle.clone()).unwrap_or_default(),
        "harness": s.provider,
        "live": live,
        "worktree": worktree,
        "branch": branch,
        "base": base,
        "head": head,
        "ahead": ahead,
        "behind": behind,
        "dirty": dirty,
        "taskState": facts.task_state,
        "landedAt": facts.landed_at,
        "retiredAt": facts.retired_at,
    })
}

fn work_list(with_git: bool) -> Result<J, CliResponse> {
    let projects = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::list(&conn).map_err(|e| no(500, "failed", e.to_string()))?
    };
    let mut out = Vec::new();
    for p in projects {
        let rows = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::sidecar::list_rows(&conn, &p.id).unwrap_or_default()
        };
        if rows.is_empty() {
            continue;
        }
        let handle = match k2_core::workspace_session_handles::workspace_address_name_shared(&p.id) {
            Ok(h) if !h.is_empty() => h,
            _ => continue,
        };
        for r in rows.into_iter().filter(|r| r.handle.is_some()) {
            let t = Target {
                project_id: p.id.clone(),
                ws_handle: handle.clone(),
                ws_label: p.name.clone(),
                ws_path: p.path.clone(),
                dir: sidecar_dir(&r, &p.path),
                sidecar: Some(r),
            };
            out.push(work_row(&t, with_git));
        }
    }
    Ok(J::Array(out))
}

fn diff_file(t: &Target, args: &Map<String, J>) -> Result<J, CliResponse> {
    use k2_core::git_changes::DiffMode;
    let file = str_arg(args, "file");
    if file.is_empty() {
        return Err(bad_args("git.diffFile needs file"));
    }
    let old = args.get("oldFile").and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty());
    let base = match args.get("base").and_then(J::as_str) {
        Some(b) => Some(safe_ref(b)?),
        None => None,
    };
    let mode = match &base {
        Some(b) => DiffMode::Base(b.clone()),
        None => DiffMode::Worktree,
    };
    let d = k2_core::git_changes::diff_sides(&t.dir, file, old, mode).map_err(changes_refusal)?;
    let side = |s: &k2_core::git_changes::DiffSide| {
        json!({ "exists": s.exists, "text": s.text, "size": s.size, "binary": s.binary, "tooLarge": s.too_large })
    };
    Ok(json!({
        "file": d.path, "oldFile": d.old_path, "base": base, "binary": d.binary, "tooLarge": d.too_large,
        "additions": d.additions, "deletions": d.deletions, "old": side(&d.old), "new": side(&d.new),
    }))
}

fn git_log(t: &Target, args: &Map<String, J>) -> Result<J, CliResponse> {
    let base = base_of(t, args)?;
    let limit = args.get("limit").and_then(J::as_u64).unwrap_or(50).clamp(1, 200) as usize;
    let commits = log_commits(&t.dir, &base, limit)?;
    let ahead = ahead_count(&t.dir, &base).unwrap_or(commits.len() as i64);
    Ok(json!({
        "branch": git_out(&t.dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]),
        "base": base,
        "ahead": ahead,
        "commits": commits,
    }))
}

fn compute_jobs(t: &Target, args: &Map<String, J>) -> Result<J, CliResponse> {
    let limit = args.get("limit").and_then(J::as_u64).unwrap_or(10).clamp(1, 50) as usize;
    // A sidecar in a worktree: jobs that ran one of its branch's commits
    // (jobs are filed under the parent workspace). The main agent: all of
    // the workspace's jobs.
    let commits: Option<Vec<String>> = match (t.sidecar.is_some(), t.worktree()) {
        (true, Some(_)) => {
            let base = default_base(t);
            let mut c: Vec<String> = log_commits(&t.dir, &base, 200)
                .unwrap_or_default()
                .iter()
                .filter_map(|x| x["sha"].as_str().map(str::to_string))
                .collect();
            if let Some(h) = git_out(&t.dir, &["rev-parse", "HEAD"]) {
                c.push(h);
            }
            Some(c)
        }
        (true, None) => Some(Vec::new()),
        _ => None,
    };
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::store::list_jobs(
            &conn,
            &k2_core::compute::store::JobFilter {
                node_id: None,
                workspace_id: Some(&t.project_id),
                state: None,
                session_id: None,
                limit: 200,
                tier: None,
                tag: None,
            },
        )
    };
    let mut out = Vec::new();
    for r in &rows {
        let v = crate::compute_views::job_summary(r);
        let commit = v["submitCommit"].as_str().or(r.src_commit.as_deref()).map(str::to_string);
        if let Some(set) = &commits {
            let hit = commit.as_deref().is_some_and(|c| !c.is_empty() && set.iter().any(|s| s.starts_with(c) || c.starts_with(s.as_str())));
            if !hit {
                continue;
            }
        }
        let mut command = r.argv.join(" ");
        if command.chars().count() > 200 {
            command = command.chars().take(199).collect::<String>() + "…";
        }
        out.push(json!({
            "id": v["short"],
            "node": v["node"],
            "state": v["state"],
            "exitCode": r.exit_code,
            "commit": commit.as_deref().map(short),
            "command": command,
            "tag": v["tag"],
            "createdAt": r.created_at,
            "startedAt": r.started_at,
            "endedAt": r.ended_at,
            "secs": r.started_at.zip(r.ended_at).map(|(s, e)| (e - s).max(0)),
        }));
        if out.len() >= limit {
            break;
        }
    }
    Ok(J::Array(out))
}

/// `GET /cli/workbench/agents` (Member): this computer's sidecars as Garden
/// rows, without git facts (cheap: the Garden's reach reads it to let a
/// widget and the Conversation widget reach `ws/name::local`, 0.46.7).
/// Agents: `k2 sidecar list` per workspace.
pub fn agents_route() -> CliResponse {
    match work_list(false) {
        Ok(v) => CliResponse::ok_json(json!({ "ok": true, "agents": v }).to_string()),
        Err(r) => r,
    }
}

// ── K2's sheet: one-time confirms bound to the call ───────────────────

struct Pending {
    garden: String,
    placement: String,
    verb: String,
    args: String,
    /// What the action is bound to (Land: the tips the person saw).
    bound: J,
    at: Instant,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn args_key(args: &Map<String, J>) -> String {
    // serde_json's Map is ordered by key: one string per argument set.
    J::Object(args.clone()).to_string()
}

/// `confirm_required`: the change K2's sheet shows, and the id the renderer
/// sends back after the person's press there.
fn ask(w: &Placed, verb: &str, args: &Map<String, J>, change: J, bound: J) -> CliResponse {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    p.retain(|_, x| x.at.elapsed() < CONFIRM_TTL);
    p.insert(
        id.clone(),
        Pending {
            garden: w.garden.clone(),
            placement: w.placement.clone(),
            verb: verb.to_string(),
            args: args_key(args),
            bound,
            at: Instant::now(),
        },
    );
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: json!({ "error": {
            "code": "confirm_required",
            "hint": "K2 asks the person on its own sheet first",
            "change": change,
            "secretId": id,
        }})
        .to_string(),
    }
}

/// The person's answer from K2's sheet: the bound facts, once.
fn confirmed(w: &Placed, verb: &str, args: &Map<String, J>, token: &str) -> Result<J, CliResponse> {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(x) = p.remove(token) else {
        return Err(no(403, "declined", "that answer from K2's sheet is unknown, used or too old: press the button again"));
    };
    if x.garden != w.garden || x.placement != w.placement || x.verb != verb || x.args != args_key(args) {
        return Err(no(403, "declined", "that answer from K2's sheet was for another change"));
    }
    if x.at.elapsed() >= CONFIRM_TTL {
        return Err(no(403, "declined", "K2's sheet was open too long: press the button again"));
    }
    Ok(x.bound)
}

fn sidecar_of<'a>(t: &'a Target, verb: &str) -> Result<&'a str, CliResponse> {
    t.handle().ok_or_else(|| bad_args(format!("{verb} is for a sidecar (ws/name), not a workspace's main agent")))
}

fn land(w: &Placed, args: &Map<String, J>, b: &CallBody) -> Result<J, CliResponse> {
    let t = target(str_arg(args, "address"))?;
    let name = sidecar_of(&t, "git.land")?.to_string();
    if t.worktree().is_none() {
        return Err(bad_args(format!("{} works in its workspace's own folder, not a worktree: nothing to land", t.thread())));
    }
    match b.confirm_token.as_deref().filter(|s| !s.is_empty()) {
        None => {
            let base = default_base(&t);
            let commits = log_commits(&t.dir, &base, 50)?;
            if commits.is_empty() {
                return Err(no(409, "conflict", format!("{} has no commits that {base} doesn't have: nothing to land", t.thread())));
            }
            let status = status_json(&t, &base)?;
            let files: Vec<J> = status["files"].as_array().into_iter().flatten().filter(|f| f["group"] == "base").cloned().collect();
            let onto = git_out(&t.ws_path, &["rev-parse", base.as_str()]).unwrap_or_default();
            let tip = git_out(&t.dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            let dirty = status["counts"]["total"].as_i64().unwrap_or(0) - status["counts"]["base"].as_i64().unwrap_or(0);
            let change = json!({
                "kind": "land", "address": t.address(), "thread": t.thread(), "workspace": t.ws_label, "name": name,
                "branch": status["branch"], "base": base, "ahead": commits.len(), "commits": commits, "files": files,
                "uncommitted": dirty, "onto": short(&onto), "tip": short(&tip),
            });
            Err(ask(w, &b.verb, args, change, json!({ "onto": onto, "tip": tip })))
        }
        Some(token) => {
            let bound = confirmed(w, &b.verb, args, token)?;
            w1::land(&t.ws_handle, &name, bound["onto"].as_str().unwrap_or(""), bound["tip"].as_str().unwrap_or(""))
        }
    }
}

fn settle(w: &Placed, args: &Map<String, J>, b: &CallBody) -> Result<J, CliResponse> {
    let t = target(str_arg(args, "address"))?;
    let name = sidecar_of(&t, "agents.settle")?.to_string();
    let discard = args.get("discard").and_then(J::as_bool).unwrap_or(false);
    match b.confirm_token.as_deref().filter(|s| !s.is_empty()) {
        None => {
            let worktree = t.worktree();
            let (branch, base, ahead, dirty) = if worktree.is_some() {
                let base = default_base(&t);
                let s = status_json(&t, &base).ok();
                let dirty = s.as_ref().map(|s| s["counts"]["total"].as_i64().unwrap_or(0) - s["counts"]["base"].as_i64().unwrap_or(0));
                (s.as_ref().map(|s| s["branch"].clone()).unwrap_or(J::Null), Some(base.clone()), ahead_count(&t.dir, &base), dirty)
            } else {
                (J::Null, None, None, None)
            };
            if !discard && ahead.unwrap_or(0) > 0 {
                return Err(no(
                    409,
                    "conflict",
                    format!("{} has {} commit(s) that aren't landed: Land them first, or Discard", t.thread(), ahead.unwrap_or(0)),
                ));
            }
            let change = json!({
                "kind": "settle", "discard": discard, "address": t.address(), "thread": t.thread(), "workspace": t.ws_label,
                "name": name, "worktree": worktree, "branch": branch, "base": base, "unlanded": ahead, "uncommitted": dirty,
            });
            Err(ask(w, &b.verb, args, change, json!({})))
        }
        Some(token) => {
            confirmed(w, &b.verb, args, token)?;
            let mut v = w1::settle(&t.ws_handle, &name, discard)?;
            if v.get("discarded").is_none() {
                v["discarded"] = json!(discard);
            }
            Ok(v)
        }
    }
}

fn start(w: &Placed, args: &Map<String, J>, b: &CallBody) -> Result<J, CliResponse> {
    let ws_token = str_arg(args, "workspace");
    let t = target(ws_token)?;
    if t.sidecar.is_some() {
        return Err(bad_args("agents.start: workspace is a workspace's address (ws::local), not a sidecar's"));
    }
    let name = k2_core::sidecar::validate_name(str_arg(args, "name"), &t.ws_handle).map_err(|e| bad_args(e.hint))?;
    let brief = args.get("brief").and_then(J::as_str).unwrap_or("").to_string();
    if brief.trim().is_empty() {
        return Err(bad_args("agents.start needs a brief: what the agent should do"));
    }
    if brief.chars().count() > BRIEF_CHARS {
        return Err(no(413, "too_large", "the brief is over 20,000 characters"));
    }
    k2_core::sidecar::check_brief(&brief).map_err(|e| bad_args(e.hint))?;
    let harness = args.get("harness").and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let base = match args.get("base").and_then(J::as_str) {
        Some(b) => safe_ref(b)?,
        None => default_base(&t),
    };
    match b.confirm_token.as_deref().filter(|s| !s.is_empty()) {
        None => {
            let excerpt: String = brief.chars().take(600).collect();
            let change = json!({
                "kind": "start", "workspace": t.ws_label, "workspaceAddress": t.address(), "name": name,
                "harness": harness, "base": base, "branch": format!("k2/{name}"), "worktree": format!(".worktrees/k2-{name}"),
                "brief": excerpt, "briefChars": brief.chars().count(),
            });
            Err(ask(w, &b.verb, args, change, json!({})))
        }
        Some(token) => {
            confirmed(w, &b.verb, args, token)?;
            w1::start(&w1::StartReq {
                workspace_id: t.project_id.clone(),
                workspace_handle: t.ws_handle.clone(),
                name,
                brief,
                harness,
                base: Some(base),
            })
        }
    }
}

/// Review notes on the agent's files, then (by default) sent to that
/// agent's own Thread address (a sidecar's, not its workspace's main agent).
fn note(t: &Target, args: &Map<String, J>) -> Result<J, CliResponse> {
    let notes = args.get("notes").and_then(J::as_array).cloned().unwrap_or_default();
    if notes.is_empty() {
        return Err(bad_args("git.note needs at least one note"));
    }
    if notes.len() > NOTES_MAX {
        return Err(bad_args(format!("git.note takes at most {NOTES_MAX} notes")));
    }
    for n in &notes {
        let body = n.get("body").and_then(J::as_str).unwrap_or("");
        let file = n.get("file").and_then(J::as_str).unwrap_or("");
        if body.trim().is_empty() || file.trim().is_empty() {
            return Err(bad_args("each note needs file and body"));
        }
        if body.chars().count() > NOTE_CHARS {
            return Err(no(413, "too_large", "a note is over 4,000 characters"));
        }
        if n.get("line").is_some_and(|l| l.as_i64().is_none_or(|l| l < 1)) {
            return Err(bad_args("a note's line is a whole number ≥ 1"));
        }
    }
    let caller = crate::git_notes_routes::human_caller(None, None);
    let deliver = crate::git_notes_routes::deliver_to_agent;
    let mut ids = Vec::new();
    for n in &notes {
        let body = json!({
            "path": t.dir,
            "filePath": n["file"],
            "startLine": n.get("line").and_then(J::as_i64).unwrap_or(1),
            "body": n["body"],
        });
        let r = crate::git_notes_routes::dispatch_post("/cli/git/notes/add", body.to_string().as_bytes(), &caller, &deliver);
        if r.status != "200 OK" {
            return Err(passthrough(r));
        }
        let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
        if let Some(id) = v.get("note").and_then(|x| x.get("id")).or_else(|| v.get("id")).and_then(J::as_str) {
            ids.push(id.to_string());
        }
    }
    let mut sent = 0;
    if args.get("send").and_then(J::as_bool).unwrap_or(true) {
        let body = json!({ "path": t.dir, "ids": ids, "to": t.thread() });
        let r = crate::git_notes_routes::dispatch_post("/cli/git/notes/send", body.to_string().as_bytes(), &caller, &deliver);
        if r.status != "200 OK" {
            return Err(passthrough(r));
        }
        let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
        sent = v["sent"].as_i64().unwrap_or(0);
    }
    Ok(json!({ "added": notes.len(), "sent": sent }))
}

fn answer(row: &VerbRow, w: &Placed, args: &Map<String, J>, b: &CallBody) -> Result<J, CliResponse> {
    match row.verb.as_str() {
        "work.list" => work_list(true),
        "git.snapshot" => {
            let t = target(str_arg(args, "address"))?;
            let base = base_of(&t, args)?;
            status_json(&t, &base)
        }
        "git.diffFile" => diff_file(&target(str_arg(args, "address"))?, args),
        "git.log" => git_log(&target(str_arg(args, "address"))?, args),
        "compute.jobs" => compute_jobs(&target(str_arg(args, "address"))?, args),
        "git.note" => note(&target(str_arg(args, "address"))?, args),
        "agents.stop" => {
            let t = target(str_arg(args, "address"))?;
            let name = sidecar_of(&t, "agents.stop").map(str::to_string).unwrap_or_default();
            w1::stop(&t.ws_handle, &name)
        }
        // GTC-5: a closed answer; Cancel on K2's sheet is the renderer's
        // `{cancelled: true}` (the person's press never reaches this file).
        "git.land" => land(w, args, b).map(|mut v| {
            for (k, d) in [("landed", json!(false)), ("commits", json!(0)), ("head", J::Null), ("conflict", json!([])), ("cancelled", json!(false))] {
                if v.get(k).is_none() {
                    v[k] = d;
                }
            }
            v
        }),
        "agents.settle" => settle(w, args, b).map(|mut v| {
            if v.get("cancelled").is_none() {
                v["cancelled"] = json!(false);
            }
            v
        }),
        "agents.start" => start(w, args, b),
        other => Err(no(400, "unknown_verb", format!("{other} isn't a Workbench row"))),
    }
}

#[cfg(test)]
#[path = "workbench_rows_tests.rs"]
mod tests;
