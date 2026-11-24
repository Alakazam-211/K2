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
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
use k2_core::cli_name;

/// prd-compute-blacksmith-class-v1 B2 (§4.1.5): `k2 ci cache trust`
/// (GET) and its change (POST).
pub const CACHE_TRUST: &str = "/cli/ci/cache/trust";
pub const CACHE_TRUST_SET: &str = "/cli/ci/cache/trust/set";
pub const EVIDENCE: &str = "/cli/ci/evidence";
pub const FREEZE: &str = "/cli/ci/freeze";
pub const INIT: &str = "/cli/ci/init";
/// prd-compute-blacksmith-class-v1 O1: `k2 ci monitor add|mute|rm` (POST)
/// and `k2 ci monitors` (GET).
pub const MONITOR: &str = "/cli/ci/monitor";
pub const MONITORS: &str = "/cli/ci/monitors";
/// prd-compute-build-attestations-v1 S3: `k2 ci provenance` (a reader).
pub const PROVENANCE: &str = "/cli/ci/provenance";
pub const RELEASE: &str = "/cli/ci/release";
pub const RUN: &str = "/cli/ci/run";
/// prd-compute-blacksmith-class-v1 O1: `k2 ci stats`.
pub const STATS: &str = "/cli/ci/stats";
pub const SETUP: &str = "/cli/ci/setup";
pub const STATUS: &str = "/cli/ci/status";
/// prd-compute-blacksmith-class-v1 T1: `k2 ci tests` (test history).
pub const TESTS: &str = "/cli/ci/tests";
pub const WATCH: &str = "/cli/ci/watch";

pub const ROUTES: &[&str] = &[CACHE_TRUST, CACHE_TRUST_SET, EVIDENCE, FREEZE, INIT, MONITOR, MONITORS, PROVENANCE, RELEASE, RUN, SETUP, STATS, STATUS, TESTS, WATCH];
pub const POST_ROUTES: &[&str] = &[EVIDENCE, FREEZE, INIT, MONITOR, PROVENANCE, RELEASE, RUN, SETUP, WATCH];

/// Longest freeze (`k2 ci freeze --until`).
pub const MAX_FREEZE_SECS: i64 = 30 * 86_400;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub(crate) fn handle(p: &str, who: &Who, body: &[u8], params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    match p {
        STATUS => status(who, params),
        TESTS => tests(who, params),
        // O1 (prd-compute-blacksmith-class-v1 §4.5.2, §4.5.4).
        STATS => stats(who, params),
        MONITORS => monitors(who, params),
        MONITOR => monitor(who, body, mgr),
        RUN => run(who, body),
        // B2: the trusted branches of the workspace's CI cache.
        CACHE_TRUST => cache_trust(who, params),
        CACHE_TRUST_SET => cache_trust_set(who, body),
        // 0.46.11 (C5/C6b, `ci_release`): a release set and its evidence.
        RELEASE => crate::ci_release::release(who, body),
        EVIDENCE => crate::ci_release::evidence(who, body),
        PROVENANCE => crate::ci_release::provenance(who, body),
        // BA-46: the signer setup file (read-only; a person applies it at the signer).
        SETUP => crate::ci_release::setup(who, body),
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
            .ok_or_else(|| no(codes::NO_RUN, 404, format!("no CI run '{t}' in workspace {} ({CLI_NAME} ci status lists them)", ws.handle)))?;
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

// ── /cli/ci/cache/trust (B2, §4.1.5) ─────────────────────────────────

/// The URL git fetches the workspace's CI remote from now (the watch's
/// remote, else origin), normalised.
fn trusted_remote_now(ws: &Ws) -> Option<String> {
    let o = engine::ops();
    let root = o.repo_root(Path::new(&ws.path)).ok()?;
    let remote = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::workspace(&conn, &ws.id).filter(|w| w.watch_at > 0).map(|w| w.remote).unwrap_or_else(|| ci::DEFAULT_REMOTE.to_string())
    };
    o.remote_url(&root, &remote)
}

fn trust_view(ws: &Ws, t: &ci::Trusted) -> serde_json::Value {
    serde_json::json!({
        "workspace": ws.handle,
        "remote": t.remote,
        "branches": t.branches,
        "default": t.default,
        "by": t.by,
        "at": t.at,
        "note": "only CI checks of these branches, run by the CI engine from a pushed commit, save the shared (trusted) cache; every other job saves only its own scratch copy",
    })
}

/// `k2 ci cache trust`: the branches whose CI checks may save the shared
/// cache. A reader verb.
fn cache_trust(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let ws = reader(who, params.get("workspace").map(String::as_str))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(trust_view(&ws, &ci::trusted(&conn, &ws.id)))
}

/// `k2 ci cache trust --add B | --set a,b | --remove B`. Narrowing is
/// anyone's who may change the workspace's CI (an agent for its own);
/// adding a branch is a person's (the owner or an Admin, from a terminal
/// outside K2): `cache_trust_needs_person`, exit 3. Never read from
/// `.k2/ci.toml` (a PR could name its own branch there).
fn cache_trust_set(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let ws = changer(who, s(&v, "workspace"), None)?;
    let (set, add, remove) = (v.get("set").map(|_| list(&v, "set")).transpose()?, list(&v, "add")?, list(&v, "remove")?);
    let asks = [set.is_some(), !add.is_empty(), !remove.is_empty()].iter().filter(|x| **x).count();
    if asks != 1 {
        return Err(no("bad_request", 400, "say one of: --add BRANCH, --set a,b (the whole list), --remove BRANCH"));
    }
    let now_list = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::trusted(&conn, &ws.id).branches
    };
    let next: Vec<String> = match (&set, add.is_empty()) {
        (Some(s), _) => s.clone(),
        (None, false) => now_list.iter().cloned().chain(add.iter().cloned()).collect(),
        (None, true) => now_list.iter().filter(|b| !remove.contains(b)).cloned().collect(),
    };
    if ci::trust_widens(&now_list, &next) && !who.is_admin() {
        let added: Vec<String> = next.iter().filter(|b| !now_list.contains(b)).cloned().collect();
        let mut r = no(
            ci::CACHE_TRUST_NEEDS_PERSON,
            403,
            format!(
                "adding {} to workspace {}'s trusted branches lets CI checks of that branch save the build cache every job here starts from, \
                 so only a person does it: the owner or an Admin, from a terminal outside {PRODUCT_NAME} ({CLI_NAME} ci cache trust --add {} --workspace {}). \
                 An agent may only remove branches",
                added.join(", "),
                ws.handle,
                added.join(","),
                ws.handle
            ),
        );
        r = r.with("workspace", serde_json::json!(ws.handle)).with("branches", serde_json::json!(added.join(",")));
        return Err(r);
    }
    let by = who.label();
    // A person's change records the remote URL git fetches from now (the
    // trusted cache is pinned to it); an agent's narrowing never touches it.
    let remote = if who.is_admin() { trusted_remote_now(&ws) } else { None };
    if who.is_admin() && remote.is_none() {
        return Err(no(
            codes::REMOTE_UNREACHABLE,
            409,
            format!("can't read the git remote's URL in workspace {}'s repo (the trusted cache is pinned to it): check its remote ({CLI_NAME} ci watch shows which)", ws.handle),
        ));
    }
    let stored = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::set_trusted(&conn, &ws.id, &next, remote.as_deref(), &by, now()).map_err(|e| no("bad_request", 400, e))?
    };
    audit(who, "ci.cache.trust", None, Some(&ws.id), None, serde_json::json!({ "from": now_list, "to": stored }));
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(trust_view(&ws, &ci::trusted(&conn, &ws.id)))
}

// ── GET /cli/ci/tests ────────────────────────────────────────────────

/// `k2 ci tests [--check C] [--flaky|--slow|--failing|--quarantined]
/// [--days 30] [--limit N]` (prd-compute-blacksmith-class-v1 §4.3.5): the
/// workspace's test history. A reader verb, like status. Test names are
/// program output: the CLI shows them as data.
fn tests(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let ws = reader(who, params.get("workspace").map(String::as_str))?;
    let check = params.get("check").map(|c| c.trim()).filter(|c| !c.is_empty());
    if let Some(c) = check {
        if !k2_core::compute::proto::ci::is_name(c) {
            return Err(no("bad_request", 400, format!("--check {c:?}: a check name ([a-z0-9._-])")));
        }
    }
    let filter_text = params.get("filter").map(String::as_str).unwrap_or("");
    let filter = k2_core::ci_tests::Filter::parse(filter_text)
        .ok_or_else(|| no("bad_request", 400, "filter: flaky, slow, failing or quarantined (or none for all three lists)"))?;
    let days = match params.get("days") {
        None => 30,
        Some(d) => d.parse::<i64>().ok().filter(|d| (1..=365).contains(d)).ok_or_else(|| no("bad_request", 400, "--days: 1-365"))?,
    };
    let limit = match params.get("limit") {
        None => 50,
        Some(l) => l
            .parse::<usize>()
            .ok()
            .filter(|l| (1..=k2_core::ci_tests::MAX_LIST).contains(l))
            .ok_or_else(|| no("bad_request", 400, "--limit: 1-500"))?,
    };
    let since = now() - days * 86_400;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = k2_core::ci_tests::list(&conn, &ws.id, check, filter, since, limit).map_err(|e| no("internal", 500, e))?;
    Ok(serde_json::json!({
        "workspace": ws.handle,
        "check": check,
        "filter": if filter_text.is_empty() { "all" } else { filter_text },
        "days": days,
        "tests": rows,
    }))
}

// ── GET /cli/ci/stats ────────────────────────────────────────────────

/// `k2 ci stats [--check C] [--branch B] [--days 14]` (prd-compute-
/// blacksmith-class-v1 §4.5.2): per check, run time and queue wait p50/p95,
/// failure and flaky rates, CPU-hours, cache hit rate. A reader verb.
fn stats(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    use k2_core::ci_stats as st;
    let ws = reader(who, params.get("workspace").map(String::as_str))?;
    let get = |k: &str| params.get(k).map(|c| c.trim()).filter(|c| !c.is_empty());
    let check = get("check");
    if let Some(c) = check {
        if !k2_core::compute::proto::ci::is_name(c) {
            return Err(no("bad_request", 400, format!("--check {c:?}: a check name ([a-z0-9._-])")));
        }
    }
    let branch = get("branch");
    if branch.is_some_and(|b| b.len() > 200 || b.chars().any(|c| c.is_whitespace() || c.is_control())) {
        return Err(no("bad_request", 400, "--branch: a branch name"));
    }
    let days = match get("days") {
        None => st::DEFAULT_DAYS,
        Some(d) => d.trim_end_matches('d').parse::<i64>().ok().filter(|d| (1..=st::MAX_DAYS).contains(d)).ok_or_else(|| no("bad_request", 400, format!("--days: 1-{}", st::MAX_DAYS)))?,
    };
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        st::stats(&conn, &ws.id, check, branch, now() - days * 86_400)
    };
    Ok(serde_json::json!({"workspace": ws.handle, "check": check, "branch": branch, "days": days, "checks": rows}))
}

// ── GET /cli/ci/monitors, POST /cli/ci/monitor ───────────────────────

/// `90s`, `10m`, `6h`, `1d`, or seconds.
fn dur_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        'd' => (&s[..s.len() - 1], 86_400),
        'h' => (&s[..s.len() - 1], 3600),
        'm' => (&s[..s.len() - 1], 60),
        's' => (&s[..s.len() - 1], 1),
        c if c.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    num.parse::<i64>().ok().filter(|n| *n > 0).and_then(|n| n.checked_mul(mul))
}

fn monitor_json(m: &k2_core::ci_monitors::Monitor) -> serde_json::Value {
    let mut v = serde_json::to_value(m).unwrap_or_default();
    v["watches"] = m.describe().into();
    // A workspace target by its handle, never a bare id.
    if let k2_core::ci_monitors::To::Workspace(id) = &m.to {
        v["to"] = serde_json::json!({"kind": "workspace", "target": crate::compute_routes::ws_handle(id)});
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("workspaceId");
    }
    v
}

/// `k2 ci monitors`: this workspace's monitors.
fn monitors(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let ws = reader(who, params.get("workspace").map(String::as_str))?;
    let list = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::ci_monitors::list(&conn, &ws.id)
    };
    Ok(serde_json::json!({"workspace": ws.handle, "monitors": list.iter().map(monitor_json).collect::<Vec<_>>()}))
}

/// `k2 ci monitor add|mute|rm <name> …` (§4.5.4). An agent for its own
/// workspace (its messages go to itself or its own Thread); people (Admin)
/// for any, to any workspace. Audited.
fn monitor(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    use k2_core::ci_monitors::{self as mon, codes as mc, Kind, Monitor, To};
    let v = json_body(body)?;
    let ws = changer(who, s(&v, "workspace"), mgr)?;
    let bad = |m: String| no(mc::INVALID, 400, m);
    let name = s(&v, "name").ok_or_else(|| bad(concat!("name the monitor: ", cli_name!(), " ci monitor add <name> --kind …").into()))?.to_string();
    if !mon::valid_name(&name) {
        return Err(bad(format!("monitor name {name:?}: 1-40 of a-z 0-9 . _ -, starting with a letter or digit")));
    }
    let action = s(&v, "action").unwrap_or("add");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let missing = || no(mc::NOT_FOUND, 404, format!("no monitor '{name}' in this workspace ({CLI_NAME} ci monitors)"));
    match action {
        "rm" | "remove" => {
            if !mon::remove(&conn, &ws.id, &name) {
                return Err(missing());
            }
            drop(conn);
            audit(who, "ci.monitor.rm", None, Some(&ws.id), None, serde_json::json!({"name": name}));
            Ok(serde_json::json!({"workspace": ws.handle, "removed": name}))
        }
        "mute" => {
            let f = s(&v, "for").ok_or_else(|| bad("mute --for 1d (at most 30d), or --for off".into()))?;
            let until = match f {
                "off" | "0" => None,
                d => Some(now() + dur_secs(d).filter(|x| *x <= mon::MAX_MUTE_SECS).ok_or_else(|| bad("--for: like 2h or 1d (at most 30d), or off".into()))?),
            };
            if !mon::mute(&conn, &ws.id, &name, until) {
                return Err(missing());
            }
            let m = mon::get(&conn, &ws.id, &name).ok_or_else(missing)?;
            drop(conn);
            audit(who, "ci.monitor.mute", None, Some(&ws.id), None, serde_json::json!({"name": name, "until": until}));
            Ok(serde_json::json!({"workspace": ws.handle, "monitor": monitor_json(&m)}))
        }
        "add" => {
            let kind_s = s(&v, "kind").ok_or_else(|| bad("--kind fail-streak|slow|queue-wait|new-flaky".into()))?;
            let kind = Kind::parse(kind_s).ok_or_else(|| bad(format!("--kind {kind_s:?}: fail-streak, slow, queue-wait or new-flaky")))?;
            let check = s(&v, "check").map(str::to_string);
            if let Some(c) = &check {
                if !k2_core::compute::proto::ci::is_name(c) {
                    return Err(bad(format!("--check {c:?}: a check name ([a-z0-9._-])")));
                }
            }
            let branch = s(&v, "branch").map(str::to_string);
            if branch.as_deref().is_some_and(|b| b.len() > 200 || b.contains('*') || b.chars().any(|c| c.is_whitespace() || c.is_control())) {
                return Err(bad("--branch: one branch name (no glob)".into()));
            }
            let over = s(&v, "over");
            let (mut count, mut over_milli, mut over_secs) = (None, None, None);
            match kind {
                Kind::FailStreak => {
                    if branch.is_none() {
                        return Err(bad("fail-streak needs --branch (e.g. --branch main)".into()));
                    }
                    let c = n(&v, "count").unwrap_or(mon::DEFAULT_STREAK);
                    if !(1..=mon::MAX_STREAK).contains(&c) {
                        return Err(bad(format!("--count: 1-{}", mon::MAX_STREAK)));
                    }
                    count = Some(c);
                }
                Kind::Slow => {
                    if check.is_none() {
                        return Err(bad("slow needs --check (a [checks.<name>] of .k2/ci.toml)".into()));
                    }
                    let m = match over {
                        None => mon::DEFAULT_OVER_MILLI,
                        Some(o) => mon::parse_ratio(o).filter(|m| (mon::MIN_OVER_MILLI..=mon::MAX_OVER_MILLI).contains(m)).ok_or_else(|| bad("--over: a ratio like 1.5x (1.1x-10x)".into()))?,
                    };
                    over_milli = Some(m);
                }
                Kind::QueueWait => {
                    let w = match over {
                        None => mon::DEFAULT_WAIT_SECS,
                        Some(o) => dur_secs(o).filter(|w| (mon::MIN_WAIT_SECS..=mon::MAX_WAIT_SECS).contains(w)).ok_or_else(|| bad("--over: a wait like 10m (1m-1d)".into()))?,
                    };
                    over_secs = Some(w);
                }
                Kind::NewFlaky => {}
            }
            if !matches!(kind, Kind::FailStreak) && branch.is_some() {
                return Err(bad("--branch goes with --kind fail-streak".into()));
            }
            if matches!(kind, Kind::FailStreak | Kind::QueueWait) && check.is_some() {
                return Err(bad(format!("--check doesn't go with --kind {}", kind.as_str())));
            }
            if matches!(kind, Kind::FailStreak | Kind::NewFlaky) && over.is_some() {
                return Err(bad(format!("--over doesn't go with --kind {}", kind.as_str())));
            }
            let cooldown = match s(&v, "cooldown") {
                None => mon::DEFAULT_COOLDOWN_SECS,
                Some(c) => dur_secs(c).filter(|c| (mon::MIN_COOLDOWN_SECS..=mon::MAX_COOLDOWN_SECS).contains(c)).ok_or_else(|| bad("--cooldown: like 6h (5m-7d)".into()))?,
            };
            // Where: this workspace (default), its Thread, or (people) another workspace.
            let to = match s(&v, "to") {
                None => To::Workspace(ws.id.clone()),
                Some(t) => match t.strip_prefix("thread:").or(if t == "thread" { Some("") } else { None }) {
                    Some(addr) => {
                        let addr = if addr.is_empty() { ws.handle.as_str() } else { addr };
                        drop(conn);
                        let owner = crate::overlay_routes::thread_project_of(addr).ok_or_else(|| bad(format!("--to thread:{addr}: no Thread at that address ({CLI_NAME} thread's address)")))?;
                        if who.passport().is_some() && owner != ws.id {
                            return Err(no("forbidden", 403, "an agent's monitor may post only to its own workspace's Thread"));
                        }
                        return add_monitor(who, &ws, Monitor {
                            workspace_id: ws.id.clone(),
                            name,
                            kind,
                            check,
                            branch,
                            count,
                            over_milli,
                            over_secs,
                            cooldown_secs: cooldown,
                            to: To::Thread(addr.to_string()),
                            muted_until: None,
                            last_fired_at: None,
                            fired: 0,
                            created_by: who.label(),
                            created_at: now(),
                        });
                    }
                    None => {
                        let other = crate::compute_routes::ws_by_token(t).ok_or_else(|| bad(format!("--to {t:?}: no such workspace (or thread:<addr>)")))?;
                        if other.id != ws.id && who.passport().is_some() {
                            return Err(no("forbidden", 403, "an agent's monitor may message only its own workspace (or its own Thread)"));
                        }
                        To::Workspace(other.id)
                    }
                },
            };
            drop(conn);
            add_monitor(who, &ws, Monitor {
                workspace_id: ws.id.clone(),
                name,
                kind,
                check,
                branch,
                count,
                over_milli,
                over_secs,
                cooldown_secs: cooldown,
                to,
                muted_until: None,
                last_fired_at: None,
                fired: 0,
                created_by: who.label(),
                created_at: now(),
            })
        }
        other => Err(bad(format!("action {other:?}: add, mute or rm"))),
    }
}

fn add_monitor(who: &Who, ws: &Ws, m: k2_core::ci_monitors::Monitor) -> R<serde_json::Value> {
    use k2_core::ci_monitors::codes as mc;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::ci_monitors::add(&conn, &m).map_err(|(code, hint)| {
            let (code, status) = match code {
                c if c == mc::EXISTS => (mc::EXISTS, 409),
                c if c == mc::LIMIT => (mc::LIMIT, 409),
                _ => ("internal", 500),
            };
            no(code, status, hint)
        })?;
    }
    audit(who, "ci.monitor.add", None, Some(&ws.id), None, serde_json::json!({"name": m.name, "kind": m.kind.as_str()}));
    Ok(serde_json::json!({"workspace": ws.handle, "monitor": monitor_json(&m)}))
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
    let on = b(&v, "on").ok_or_else(|| no("bad_request", 400, concat!("on: true (", cli_name!(), " ci watch on) or false (", cli_name!(), " ci watch off)")))?;
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
        // A person's watch records the remote for the trusted cache when
        // none is (never an agent's; never replacing one).
        if who.is_admin() {
            if let Some(url) = engine::ops().repo_root(Path::new(&ws.path)).ok().and_then(|root| engine::ops().remote_url(&root, &w.remote)) {
                ci::record_remote_if_unset(&conn, &ws.id, &url).map_err(|e| no("internal", 500, e))?;
            }
        }
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
        return Err(no(codes::CONFIG_EXISTS, 409, format!("{} already exists: edit it, or {CLI_NAME} ci init --force to replace it", path.display())));
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
