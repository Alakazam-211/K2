//! `/cli/ci/*` — `k2 ci` (0.46.10, prd-ci-on-compute-for-clients-v1 §4.7,
//! vs-live V32 slice C6a). The CLI is thin; the engine is
//! [`crate::ci_engine`], the store `k2_core::ci`.
//!
//! These ride the compute router (`compute_routes::handle_from`): the same
//! callers ([`Who`]), the same preview flag (404 while compute is off), the
//! TCP listener and the per-cell socket alike.
//!
//! Who may do what (the compute model):
//! - **status, run** (agent verbs): an agent passport needs its
//!   workspace's "Allow compute nodes" switch and acts only for its own
//!   workspace; people name the workspace. A run's jobs always carry the
//!   **workspace's** compute grants (never the caller's or the owner's
//!   reach), so `--tier ci` needs a grant with `--ci`.
//! - **watch, freeze, init** (change what the daemon does for a
//!   workspace): the owner or an Admin; an agent for its own workspace
//!   (switch on); a local agent under "Allow managing compute nodes" for
//!   any workspace (watch and freeze, like grants). A Connect login below
//!   Admin is `owner_only`. Each change is audited (`compute_events`).
//!
//! Nothing here ever holds or answers a token: statuses name the vault
//! secret, the engine reads it when it posts.

use std::collections::HashMap;
use std::path::Path;

use k2_core::ci::{self as ci, codes};
use k2_core::compute::proto::ci_config::{CiConfig, CI_CONFIG_PATH};

use crate::ci_engine::{self as engine, Start};
use crate::compute_manage::Manager;
use crate::compute_routes::{audit, b, grant_target_ws, json_body, n, need_admin, need_credential, no, passport_gate, s, target_ws, Who, Ws, R};

pub const EVIDENCE: &str = "/cli/ci/evidence";
pub const FREEZE: &str = "/cli/ci/freeze";
pub const INIT: &str = "/cli/ci/init";
pub const RELEASE: &str = "/cli/ci/release";
pub const RUN: &str = "/cli/ci/run";
pub const STATUS: &str = "/cli/ci/status";
pub const WATCH: &str = "/cli/ci/watch";

pub const ROUTES: &[&str] = &[EVIDENCE, FREEZE, INIT, RELEASE, RUN, STATUS, WATCH];
pub const POST_ROUTES: &[&str] = &[EVIDENCE, FREEZE, INIT, RELEASE, RUN, WATCH];

/// Longest freeze (`k2 ci freeze --until`).
pub const MAX_FREEZE_SECS: i64 = 30 * 86_400;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub(crate) fn handle(p: &str, who: &Who, body: &[u8], params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    match p {
        STATUS => status(who, params),
        RUN => run(who, body),
        // 0.46.11 (C5/C6b, `ci_release`): a release set and its evidence.
        RELEASE => crate::ci_release::release(who, body),
        EVIDENCE => crate::ci_release::evidence(who, body),
        WATCH => watch(who, body, mgr),
        INIT => init(who, body),
        FREEZE => freeze(who, body, mgr),
        _ => Err(no("not_found", 404, "no such ci route")),
    }
}

// ── who ──────────────────────────────────────────────────────────────

/// Read or run: anyone who may use compute for the workspace.
fn reader(who: &Who, requested: Option<&str>) -> R<Ws> {
    need_credential(who)?;
    passport_gate(who)?;
    target_ws(who, requested)
}

/// Change what the daemon does for a workspace: people need Admin; an
/// agent acts for its own workspace; a manager (`mgr`) for any.
fn changer(who: &Who, requested: Option<&str>, mgr: Option<&Manager>) -> R<Ws> {
    need_credential(who)?;
    if who.passport().is_some() {
        if mgr.is_some() {
            return grant_target_ws(who, requested, mgr);
        }
        passport_gate(who)?;
        return target_ws(who, requested);
    }
    need_admin(who)?;
    target_ws(who, requested)
}

fn now() -> i64 {
    k2_core::compute::now()
}

fn tier_label(t: &str) -> &'static str {
    if t == "ci" {
        "ci"
    } else {
        "standard"
    }
}

/// `ci` | `standard` (`normal`) → the stored tier.
fn tier_from(v: &serde_json::Value) -> R<Option<&'static str>> {
    match s(v, "tier") {
        None => Ok(None),
        Some("ci") => Ok(Some("ci")),
        Some("standard" | "normal") => Ok(Some("normal")),
        Some(_) => Err(no(codes::BAD_WATCH, 400, "--tier is ci or standard")),
    }
}

/// A list field: a JSON array of strings or one comma string.
fn list(v: &serde_json::Value, k: &str) -> R<Vec<String>> {
    match v.get(k) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::String(x)) => Ok(x.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| no("bad_request", 400, format!("{k} is a list of strings"))))
            .collect(),
        Some(_) => Err(no("bad_request", 400, format!("{k} is a list of strings"))),
    }
}

// ── views ────────────────────────────────────────────────────────────

fn watch_view(w: Option<&ci::WorkspaceRow>) -> serde_json::Value {
    match w {
        Some(w) if w.watch_on || w.watch_at > 0 => serde_json::json!({
            "on": w.watch_on,
            "remote": w.remote,
            "branches": w.branches,
            "pollSecs": w.poll_secs,
            "tier": tier_label(&w.tier),
            "maxInflight": w.max_inflight,
            "by": w.watch_by,
            "at": w.watch_at,
            "lastPollAt": w.last_poll_at,
            "lastError": w.last_error,
        }),
        _ => serde_json::Value::Null,
    }
}

fn freeze_view(w: Option<&ci::WorkspaceRow>, now: i64) -> serde_json::Value {
    match w.and_then(|w| ci::frozen(w, now).map(|(until, why)| (w, until, why))) {
        Some((w, until, why)) => serde_json::json!({ "until": until, "why": why, "by": w.freeze_by, "at": w.freeze_at }),
        None => serde_json::Value::Null,
    }
}

pub(crate) fn run_view(conn: &rusqlite::Connection, r: &ci::RunRow) -> serde_json::Value {
    let jobs = if r.kind == k2_core::ci_release::kind::RELEASE { k2_core::ci_release::job_facts(conn, &r.id) } else { Default::default() };
    let checks: Vec<serde_json::Value> = ci::checks_of(conn, &r.id)
        .into_iter()
        .map(|c| {
            let mut v = serde_json::json!({
                "name": c.name,
                "job": c.job_id,
                "state": c.state,
                "description": c.description,
                "status": c.posted,
                "statusError": c.post_error,
            });
            if let Some(j) = jobs.get(&c.name) {
                v["label"] = j.label.clone().into();
                v["build"] = j.build.clone().into();
                v["role"] = j.role.clone().into();
                v["nodeId"] = j.node_id.clone().into();
            }
            v
        })
        .collect();
    let release = (r.kind == k2_core::ci_release::kind::RELEASE)
        .then(|| k2_core::ci_release::release_facts(conn, &r.id).map(|(_, f)| f))
        .flatten();
    serde_json::json!({
        "kind": r.kind,
        "release": release,
        "id": r.id,
        "short": r.short(),
        "sha": r.sha,
        "ref": r.git_ref,
        "trigger": r.trigger,
        "state": r.state,
        "detail": r.detail,
        "by": r.created_by,
        "tier": tier_label(&r.tier),
        "createdAt": r.created_at,
        "endedAt": r.ended_at,
        "statuses": r.statuses,
        "checks": checks,
    })
}

// ── GET /cli/ci/status ───────────────────────────────────────────────

/// The workspace's watch, freeze and newest runs; or one run (`run=` an
/// id, a commit or a branch).
fn status(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let ws = reader(who, params.get("workspace").map(String::as_str))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(t) = params.get("run").map(|x| x.trim()).filter(|x| !x.is_empty()) {
        let r = ci::find_run(&conn, &ws.id, t)
            .ok_or_else(|| no(codes::NO_RUN, 404, format!("no CI run '{t}' in workspace {} (k2 ci status lists them)", ws.handle)))?;
        return Ok(serde_json::json!({ "workspace": ws.handle, "run": run_view(&conn, &r) }));
    }
    let limit = params.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(10).clamp(1, 50);
    let row = ci::workspace(&conn, &ws.id);
    let runs: Vec<serde_json::Value> = ci::runs_of(&conn, &ws.id, limit).iter().map(|r| run_view(&conn, r)).collect();
    Ok(serde_json::json!({
        "workspace": ws.handle,
        "watch": watch_view(row.as_ref()),
        "freeze": freeze_view(row.as_ref(), now()),
        "inflight": ci::inflight_count(&conn, &ws.id),
        "runs": runs,
    }))
}

// ── POST /cli/ci/run ─────────────────────────────────────────────────

/// `k2 ci run [<rev>] [--check a,b] [--ref B] [--tier ci|standard]`: one
/// run now, the same engine as the watcher. `dir` (the caller's repo top
/// level) resolves `HEAD` in a git worktree of the workspace.
fn run(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let ws = reader(who, s(&v, "workspace"))?;
    let rev = s(&v, "rev").unwrap_or("HEAD");
    if rev.starts_with('-') || rev.len() > 256 {
        return Err(no("bad_request", 400, "a revision: a commit, a branch or HEAD"));
    }
    let only = list(&v, "checks")?;
    if let Some(bad) = only.iter().find(|c| !k2_core::compute::proto::ci::is_name(c)) {
        return Err(no("bad_request", 400, format!("--check {bad:?}: a check name ([a-z0-9._-])")));
    }
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::workspace(&conn, &ws.id)
    };
    let tier = match tier_from(&v)? {
        Some(t) => t.to_string(),
        None => row.as_ref().filter(|w| w.watch_on).map(|w| w.tier.clone()).unwrap_or_else(|| "ci".to_string()),
    };
    let remote = row.as_ref().filter(|w| w.watch_at > 0).map(|w| w.remote.clone()).unwrap_or_else(|| ci::DEFAULT_REMOTE.to_string());
    let o = engine::ops();
    let root = o.repo_root(Path::new(&ws.path)).map_err(|e| not_a_repo(&ws, &e))?;
    let checkout = match s(&v, "dir") {
        Some(d) => match k2_core::compute::sync::caller_checkout(Path::new(&ws.path), &root, Path::new(d)) {
            Ok(Some(wt)) => wt,
            _ => root.clone(),
        },
        None => root.clone(),
    };
    let sha = o
        .resolve(&checkout, rev)
        .map_err(|e| no(codes::NO_COMMIT, 404, format!("'{rev}' isn't a commit in workspace {}'s repo ({e}); commit first, CI checks commits", ws.handle)))?;
    let git_ref = match s(&v, "ref") {
        Some(r) if r.starts_with('-') || r.contains(char::is_whitespace) || r.len() > 200 => return Err(no("bad_request", 400, "--ref is a branch name")),
        Some(r) => Some(r.to_string()),
        None if rev != "HEAD" && o.is_branch(&root, rev) => Some(rev.to_string()),
        None => None,
    };
    let by = who.label();
    let start = Start { sha: &sha, git_ref: git_ref.as_deref(), trigger: "manual", by: &by, tier: &tier, remote: &remote, only: &only };
    let p = engine::prepare(o.as_ref(), &ws, &root, &start).map_err(|p| no(p.code, p.status, p.hint))?;
    let r = engine::launch(o.as_ref(), &ws, &start, p, now());
    audit(who, "ci.run", None, Some(&ws.id), None, serde_json::json!({ "run": r.id, "sha": r.sha, "ref": r.git_ref, "state": r.state }));
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(serde_json::json!({ "workspace": ws.handle, "run": run_view(&conn, &r) }))
}

fn not_a_repo(ws: &Ws, e: &str) -> crate::compute_routes::Refusal {
    no(codes::NOT_A_REPO, 409, format!("workspace {}'s folder isn't a git repo ({e}); CI checks commits of a repo", ws.handle))
}

/// `[watch]` in the working tree's `.k2/ci.toml`, when the file is there.
fn file_watch(root: &Path) -> R<Option<k2_core::compute::proto::ci_config::WatchConfig>> {
    let path = root.join(CI_CONFIG_PATH);
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
    let cfg = CiConfig::parse(&text).map_err(|e| no(codes::BAD_CONFIG, 400, format!("{e} (fix the file, then try again)")))?;
    Ok(cfg.watch)
}

// ── POST /cli/ci/watch ───────────────────────────────────────────────

/// `k2 ci watch on [--remote R] [--branches a,b/*] [--poll S] [--tier T]
/// [--max-inflight N] [--skip-current]` / `k2 ci watch off`. Unset values
/// come from the working tree's `.k2/ci.toml` `[watch]`, then the
/// workspace's last watch, then `origin` / `main` / 60 s / ci / 2. The
/// remote must answer `git ls-remote` now. Without `--skip-current` the
/// branches' current tips are checked on the next tick too.
fn watch(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let ws = changer(who, s(&v, "workspace"), mgr)?;
    let on = b(&v, "on").ok_or_else(|| no("bad_request", 400, "on: true (k2 ci watch on) or false (k2 ci watch off)"))?;
    let by = who.label();
    let at = now();
    if !on {
        let changed = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ci::watch_off(&conn, &ws.id, &by, at).map_err(|e| no("internal", 500, e))?
        };
        audit(who, "ci.watch.off", None, Some(&ws.id), None, serde_json::json!({ "changed": changed }));
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = ci::workspace(&conn, &ws.id);
        return Ok(serde_json::json!({ "workspace": ws.handle, "watch": watch_view(row.as_ref()), "changed": changed }));
    }
    let o = engine::ops();
    let root = o.repo_root(Path::new(&ws.path)).map_err(|e| not_a_repo(&ws, &e))?;
    let from_file = file_watch(&root)?;
    let last = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::workspace(&conn, &ws.id).filter(|w| w.watch_at > 0)
    };
    let branches = {
        let asked = list(&v, "branches")?;
        if !asked.is_empty() {
            asked
        } else if let Some(f) = &from_file {
            f.branches.clone()
        } else if let Some(l) = &last {
            l.branches.clone()
        } else {
            vec![ci::DEFAULT_BRANCH.to_string()]
        }
    };
    let remote = s(&v, "remote")
        .map(str::to_string)
        .or_else(|| from_file.as_ref().map(|f| f.remote.clone()))
        .or_else(|| last.as_ref().map(|l| l.remote.clone()))
        .unwrap_or_else(|| ci::DEFAULT_REMOTE.to_string());
    let poll_secs = match n(&v, "pollSecs") {
        Some(p) => u32::try_from(p).map_err(|_| no(codes::BAD_WATCH, 400, "--poll is 30-3600 seconds"))?,
        None => from_file.as_ref().map(|f| f.poll_secs).or(last.as_ref().map(|l| l.poll_secs)).unwrap_or(60),
    };
    let tier = tier_from(&v)?.map(str::to_string).or_else(|| last.as_ref().map(|l| l.tier.clone())).unwrap_or_else(|| "ci".to_string());
    let max_inflight = match n(&v, "maxInflight") {
        Some(m) => u32::try_from(m).map_err(|_| no(codes::BAD_WATCH, 400, format!("--max-inflight is 1-{}", ci::MAX_INFLIGHT_CAP)))?,
        None => last.as_ref().map(|l| l.max_inflight).unwrap_or(ci::DEFAULT_MAX_INFLIGHT),
    };
    let w = ci::Watch { remote, branches, poll_secs, tier, max_inflight };
    w.check().map_err(|e| no(codes::BAD_WATCH, 400, e))?;
    // Loud at set-up, not silently in the loop: the remote must answer.
    let tips = engine::current_tips(&ws, &w.remote, &w.branches).map_err(|e| {
        no(
            codes::REMOTE_UNREACHABLE,
            409,
            format!("git ls-remote {} failed in workspace {}'s repo: {e}. The daemon reads the remote with this machine's own git credentials", w.remote, ws.handle),
        )
    })?;
    let skip = b(&v, "skipCurrent") == Some(true);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::set_watch(&conn, &ws.id, &w, &by, at).map_err(|e| no(codes::BAD_WATCH, 400, e))?;
        if skip {
            for (sha, branch) in &tips {
                ci::mark_seen(&conn, &ws.id, sha, branch, at);
            }
        }
    }
    audit(
        who,
        "ci.watch.on",
        None,
        Some(&ws.id),
        None,
        serde_json::json!({ "remote": w.remote, "branches": w.branches, "pollSecs": w.poll_secs, "tier": w.tier, "maxInflight": w.max_inflight, "skipCurrent": skip }),
    );
    let tips_json: Vec<serde_json::Value> = tips.iter().map(|(sha, branch)| serde_json::json!({ "branch": branch, "sha": sha })).collect();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = ci::workspace(&conn, &ws.id);
    Ok(serde_json::json!({ "workspace": ws.handle, "watch": watch_view(row.as_ref()), "tips": tips_json, "skippedCurrent": skip }))
}

// ── POST /cli/ci/init ────────────────────────────────────────────────

/// `k2 ci init [--check NAME] [--needs a,b] [--os linux] [--mem GB]
/// [--cpus N] [--timeout S] [--remote R] [--branches a,b] [--statuses-token
/// NAME] [--force] [-- <cmd…>]`: write a starter `.k2/ci.toml` at the
/// repo's top level. Never overwrites without `--force`. Without a command
/// the check is a placeholder that fails and says to edit the file.
fn init(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let ws = changer(who, s(&v, "workspace"), None)?;
    let o = engine::ops();
    let root = o.repo_root(Path::new(&ws.path)).map_err(|e| not_a_repo(&ws, &e))?;
    let path = root.join(CI_CONFIG_PATH);
    if path.exists() && b(&v, "force") != Some(true) {
        return Err(no(codes::CONFIG_EXISTS, 409, format!("{} already exists: edit it, or k2 ci init --force to replace it", path.display())));
    }
    let num = |k: &str, max: i64| -> R<Option<i64>> {
        match n(&v, k) {
            None => Ok(None),
            Some(x) if (1..=max).contains(&x) => Ok(Some(x)),
            Some(_) => Err(no(codes::BAD_CONFIG, 400, format!("{k} is 1-{max}"))),
        }
    };
    // argv is only ever a list (never a comma string: words may hold commas).
    let argv = match v.get("argv") {
        Some(serde_json::Value::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_string).ok_or_else(|| no("bad_request", 400, "argv is a list of strings"))).collect::<R<Vec<_>>>()?,
        Some(serde_json::Value::Null) | None => Vec::new(),
        Some(_) => return Err(no("bad_request", 400, "argv is a list of strings")),
    };
    let starter = ci::Starter {
        remote: s(&v, "remote").map(str::to_string),
        branches: list(&v, "branches")?,
        check: s(&v, "check").map(str::to_string),
        argv,
        needs: list(&v, "needs")?,
        os: s(&v, "os").map(str::to_string),
        mem_gb: num("memGb", 1024)?.map(|x| x as u32),
        cpus: num("cpus", 256)?.map(|x| x as u32),
        timeout_secs: num("timeoutSecs", 7 * 86_400)?.map(|x| x as u64),
        statuses_token: s(&v, "statusesToken").map(str::to_string),
    };
    let text = starter.render().map_err(|e| no(codes::BAD_CONFIG, 400, e))?;
    let parent = path.parent().unwrap_or(root.as_path());
    std::fs::create_dir_all(parent).map_err(|e| no("internal", 500, format!("create {}: {e}", parent.display())))?;
    k2_core::fs_atomic::atomic_write_str(&path, &text).map_err(|e| no("internal", 500, format!("write {}: {e}", path.display())))?;
    // A .gitignore that hides the file would make every commit `skipped`.
    let ignored = {
        let mut c = k2_core::git_safe::git(&root);
        c.args(["check-ignore", "-q", "--", CI_CONFIG_PATH]);
        c.output().map(|o| o.status.success()).unwrap_or(false)
    };
    audit(who, "ci.init", None, Some(&ws.id), None, serde_json::json!({ "path": path.to_string_lossy(), "ignored": ignored }));
    Ok(serde_json::json!({
        "workspace": ws.handle,
        "path": path.to_string_lossy(),
        "config": text,
        "placeholder": starter.argv.is_empty(),
        "ignored": ignored,
    }))
}

// ── POST /cli/ci/freeze ──────────────────────────────────────────────

/// `k2 ci freeze --until <time> [--why "…"]` / `k2 ci freeze off`: flag a
/// release freeze (0.46.11's rebalancer moves no seat inside one). At most
/// 30 days ahead.
fn freeze(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let ws = changer(who, s(&v, "workspace"), mgr)?;
    let at = now();
    let by = who.label();
    let (until, why) = if b(&v, "off") == Some(true) {
        (None, None)
    } else {
        let until = n(&v, "until").ok_or_else(|| no("bad_request", 400, "--until <time> (2026-11-20T00:00:00Z, or 3d / 12h from now), or off"))?;
        if until <= at || until > at + MAX_FREEZE_SECS {
            return Err(no("bad_request", 400, "--until is in the future and at most 30 days ahead"));
        }
        let why = s(&v, "why").map(str::to_string);
        if why.as_deref().is_some_and(|w| w.len() > 200 || w.contains(char::is_control)) {
            return Err(no("bad_request", 400, "--why is one line of at most 200 characters"));
        }
        (Some(until), why)
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::set_freeze(&conn, &ws.id, until, why.as_deref(), &by, at).map_err(|e| no("internal", 500, e))?;
    }
    audit(who, if until.is_some() { "ci.freeze.on" } else { "ci.freeze.off" }, None, Some(&ws.id), None, serde_json::json!({ "until": until, "why": why }));
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = ci::workspace(&conn, &ws.id);
    Ok(serde_json::json!({ "workspace": ws.handle, "freeze": freeze_view(row.as_ref(), at) }))
}
