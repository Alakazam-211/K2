//! `k2 ci release` and `k2 ci evidence` (0.46.11, prd-ci-on-compute-for-
//! clients-v1 §4.6, §4.7; vs-live V31, V32; slices C5 + C6b). The rules
//! and the store are `k2_core::ci_release`; this file gathers the facts,
//! submits the set and assembles the evidence bundle. It rides the `k2 ci`
//! router (`ci_routes`): the same callers, the same `ciPreview` gate.
//!
//! **`POST /cli/ci/release`** `{workspace?, version, rev?, dir?, builds?,
//! tier?, plan?}`: read `.k2/ci.toml` **at the commit** (`[release]`), find
//! this server's seat on `[release] owner` / `lane`, read the owner's
//! newest node roster held here, ask the normal job path (`--plan`) which
//! seat nodes can take each build and how soon, place the set as one unit
//! (`ci_release::place`), then submit every job: detached, `--tier ci`,
//! receipt v2, `--only <its node>`, `BUILD_COMMIT` / `BUILD_TREE` set, the
//! build's lane secrets. A submit that fails cancels the set's jobs that
//! were queued and ends the run `error`. `plan: true` answers the placement
//! and submits nothing. The jobs carry the **workspace's** compute grants,
//! never the caller's reach, exactly as `k2 ci run`'s.
//!
//! **`POST /cli/ci/evidence`** `{workspace?, run}`: once every job of a
//! release run ended done/exit 0 with a receipt v2 this server verified, the
//! evidence bundle v2 (`docs/ci-formats-v1.md` §8): receipts and plans by
//! label, the self-statements they name, the owner's newest roster and this
//! client's seat statement, the pairing pins, the commit object. Nothing in
//! it is trusted because it is in the bundle: the signer re-checks
//! everything with its own pins. The answer also lists each job's files
//! (to fetch with `k2 compute fetch`) and the dual-build comparison this
//! server saw, a convenience only.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use k2_core::ci::{self as ci, check_state, run_state};
use k2_core::ci_release::{self as rel, codes as rcodes, Ask, Fit, Placed, SeatNode};
use k2_core::compute::proto::ci::Signed;
use k2_core::compute::proto::ci_config::{CiConfig, ReleaseBuild, ReleaseConfig, CI_CONFIG_PATH};
use k2_core::compute::proto::crypto;
use k2_core::compute::proto::evidence::{EvidenceBundle, EvidenceReceipt, EvidenceSource, Role, EVIDENCE_KIND, EVIDENCE_V};
use k2_core::compute::proto::frames::{Receipt, SignedReceipt, RECEIPT_V2};
use k2_core::compute::{lane_secrets, store, trust};
use serde_json::{json, Value};

use crate::ci_engine;
use crate::compute_routes::{audit, b, json_body, need_credential, no, passport_gate, s, target_ws, Who, Ws, R};

fn now() -> i64 {
    k2_core::compute::now()
}

fn reader(who: &Who, requested: Option<&str>) -> R<Ws> {
    need_credential(who)?;
    passport_gate(who)?;
    target_ws(who, requested)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `git rev-parse <sha>^{tree}` and `git cat-file commit <sha>` in the
/// workspace's repo (hardened git, `k2_core::git_safe`).
fn git_out(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut c = k2_core::git_safe::git(root);
    c.args(args);
    let o = c.output().map_err(|e| format!("git: {e}"))?;
    if !o.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(o.stdout)
}

fn tree_of(root: &Path, sha: &str) -> Result<String, String> {
    let out = git_out(root, &["rev-parse", "--verify", &format!("{sha}^{{tree}}")])?;
    let t = String::from_utf8_lossy(&out).trim().to_string();
    if ci::is_sha(&t) {
        Ok(t)
    } else {
        Err(format!("git rev-parse {sha}^{{tree}}: not a tree id"))
    }
}

// ── the set ──────────────────────────────────────────────────────────

/// One job of the set before it is placed: its build, label, role and the
/// `/cli/compute/run` body (without `only` and `clientJobId`).
struct JobSpec {
    build: String,
    label: String,
    body: Value,
}

/// The `/cli/compute/run` body of a build: the normal job path, at the
/// commit, detached, receipt v2, the build's needs, files, env names and
/// lane secrets. Env: `BUILD_COMMIT` / `BUILD_TREE` carry the commit and
/// tree; every other name is set empty (signer M1: the policy pins names,
/// not values).
fn build_body(name: &str, label: &str, b: &ReleaseBuild, sha: &str, tree: &str, tier: &str) -> Value {
    let env: serde_json::Map<String, Value> = b
        .env
        .iter()
        .map(|e| {
            let v = match e.as_str() {
                "BUILD_COMMIT" => sha.to_string(),
                "BUILD_TREE" => tree.to_string(),
                _ => String::new(),
            };
            (e.clone(), Value::String(v))
        })
        .collect();
    let secrets: Vec<Value> = b.secrets.iter().map(|s| json!({"name": s, "env": s})).collect();
    let mut v = json!({
        "argv": b.argv,
        "sha": sha,
        "tier": tier,
        "detach": true,
        "submitter": "release",
        "receiptV": RECEIPT_V2,
        "tag": format!("release:{name}:{label}").chars().take(64).collect::<String>(),
    });
    if !b.needs.is_empty() {
        v["needs"] = json!(b.needs);
    }
    if !b.artifacts.is_empty() {
        v["artifacts"] = json!(b.artifacts);
    }
    if !env.is_empty() {
        v["env"] = Value::Object(env);
    }
    if !secrets.is_empty() {
        v["secrets"] = json!(secrets);
    }
    if let Some(t) = b.timeout_secs {
        v["timeoutSecs"] = json!(t);
    }
    if let Some(c) = b.cpus {
        v["cpus"] = json!(c);
    }
    if let Some(gb) = b.mem_gb {
        v["memBytes"] = json!(u64::from(gb) << 30);
    }
    v
}

/// The seat's nodes as this server knows them, with why each is out.
fn seat_nodes(conn: &rusqlite::Connection, seat: &lane_secrets::SeatView, seat_names: &[String]) -> Vec<(SeatNode, Option<store::NodeRow>)> {
    let roster = trust::latest_roster(conn, &seat.owner_key_fp);
    seat.node_ids
        .iter()
        .map(|id| {
            let row = store::node_by_id(conn, id);
            let (name, entry, out) = match &row {
                None => (id.clone(), None, Some("removed from this server".to_string())),
                Some(n) => {
                    let entry = roster.as_ref().and_then(|r| r.roster.entry(&n.fingerprint)).cloned();
                    let revoked = trust::key_revoked(conn, &n.fingerprint).is_some();
                    let mut out = rel::entry_problem(entry.as_ref(), seat_names, revoked);
                    if out.is_none() && n.state != "active" {
                        out = Some(format!("it is {} here", n.state));
                    }
                    (entry.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| n.name.clone()), entry, out)
                }
            };
            (SeatNode { node_id: id.clone(), name, entry, out }, row)
        })
        .collect()
}

/// What `/cli/ci/release` places, before anything is stored.
struct Planned {
    cfg: ReleaseConfig,
    sha: String,
    tree: String,
    seat: lane_secrets::SeatView,
    placed: Vec<Placed>,
    specs: Vec<JobSpec>,
    /// node id → this server's node name (what `--only` names).
    local_names: HashMap<String, String>,
}

fn plan_set(ws: &Ws, v: &Value, run_id: &str) -> R<Planned> {
    let o = ci_engine::ops();
    let root = o
        .repo_root(Path::new(&ws.path))
        .map_err(|e| no(ci::codes::NOT_A_REPO, 409, format!("workspace {}'s folder isn't a git repo ({e}); a release builds a commit of a repo", ws.handle)))?;
    let rev = s(v, "rev").unwrap_or("HEAD");
    if rev.starts_with('-') || rev.len() > 256 {
        return Err(no("bad_request", 400, "a revision: a commit, a branch or HEAD"));
    }
    let checkout = match s(v, "dir") {
        Some(d) => match k2_core::compute::sync::caller_checkout(Path::new(&ws.path), &root, Path::new(d)) {
            Ok(Some(wt)) => wt,
            _ => root.clone(),
        },
        None => root.clone(),
    };
    let sha = o
        .resolve(&checkout, rev)
        .map_err(|e| no(ci::codes::NO_COMMIT, 404, format!("'{rev}' isn't a commit in workspace {}'s repo ({e}); commit first, a release builds commits", ws.handle)))?;
    let text = o
        .read_config(&root, &sha)
        .map_err(|e| no(ci::codes::BAD_CONFIG, 400, format!("{CI_CONFIG_PATH} at {}: {}", &sha[..12], one_line(&e))))?
        .ok_or_else(|| no(ci::codes::NO_CONFIG, 404, format!("commit {} has no {CI_CONFIG_PATH}: add a [release] section, commit it, then release (k2 study ci-release)", &sha[..12])))?;
    let cfg = CiConfig::parse(&text).map_err(|e| no(ci::codes::BAD_CONFIG, 400, format!("at {}: {e}", &sha[..12])))?;
    let Some(rcfg) = cfg.release else {
        return Err(no(rcodes::NO_RELEASE, 404, format!("{CI_CONFIG_PATH} at {} has no [release]: add one (k2 study ci-release; k2 app init --template tauri writes one)", &sha[..12])));
    };
    let tree = tree_of(&root, &sha).map_err(|e| no(ci::codes::NO_COMMIT, 404, e))?;
    let tier = match s(v, "tier") {
        None | Some("ci") => "ci",
        Some("standard" | "normal") => "normal",
        Some(_) => return Err(no("bad_request", 400, "--tier is ci or standard")),
    };
    // `--build a,b`: only those builds (every one otherwise).
    let only: Vec<String> = match v.get("builds") {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(x)) => x.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    if let Some(bad) = only.iter().find(|n| !rcfg.builds.contains_key(*n)) {
        let have: Vec<&str> = rcfg.builds.keys().map(String::as_str).collect();
        return Err(no(rcodes::NO_BUILD, 404, format!("no build '{bad}' in [release] (it has: {})", have.join(", "))));
    }
    // The seat on the owner's lane.
    let (seat, nodes) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let seat = lane_secrets::seat_named_in(&conn, &rcfg.owner, &rcfg.lane).ok_or_else(|| {
            no(
                rcodes::NO_SEAT,
                404,
                format!(
                    "this server holds no seat on {}'s lane {}: a person asks for one (k2 compute seats request {} --lane {}), the owner approves it, then release again",
                    rcfg.owner, rcfg.lane, rcfg.owner, rcfg.lane
                ),
            )
        })?;
        let row = trust::seat(&conn, "client", &seat.seat_id);
        let names = row.as_ref().map(|r| r.nodes.clone()).unwrap_or_default();
        if !seat.active {
            let state = row.map(|r| r.state).unwrap_or_else(|| "gone".into());
            return Err(no(rcodes::SEAT_DEGRADED, 409, format!("seat {} on {} is {state}, not active: no release builds there until the owner makes it active", seat.seat_id, rcfg.owner)));
        }
        let nodes = seat_nodes(&conn, &seat, &names);
        (seat, nodes)
    };
    let mut local_names: HashMap<String, String> = HashMap::new();
    for (sn, row) in &nodes {
        if let Some(r) = row {
            local_names.insert(sn.node_id.clone(), r.name.clone());
        }
    }
    // The builds and their jobs.
    let mut asks = Vec::new();
    let mut specs = Vec::new();
    for (name, bcfg) in rcfg.builds.iter().filter(|(n, _)| only.is_empty() || only.contains(n)) {
        asks.push(Ask { build: name.clone(), label: bcfg.label.clone(), verify_label: bcfg.verify_label.clone(), independence: bcfg.independence() });
        specs.push(JobSpec { build: name.clone(), label: bcfg.label.clone(), body: build_body(name, &bcfg.label, bcfg, &sha, &tree, tier) });
        if let Some(vl) = &bcfg.verify_label {
            specs.push(JobSpec { build: name.clone(), label: vl.clone(), body: build_body(name, vl, bcfg, &sha, &tree, tier) });
        }
    }
    // Per build and node: would the normal job path place it there, and
    // how soon? (`--plan`: nothing stored.)
    let mut fits: HashMap<(String, String), Result<Fit, String>> = HashMap::new();
    for spec in specs.iter().filter(|sp| asks.iter().any(|a| a.label == sp.label)) {
        for (sn, _) in nodes.iter().filter(|(sn, _)| sn.out.is_none()) {
            let Some(local) = local_names.get(&sn.node_id) else { continue };
            let mut probe = spec.body.clone();
            probe["only"] = json!([local]);
            let r = crate::compute_routes::plan_for_ci(&ws.id, run_id, &probe).map(|p| Fit {
                starts_now: p["placement"]["startsNow"].as_bool().unwrap_or(false),
                eta_secs: p["placement"]["etaSecs"].as_u64().unwrap_or(0),
            });
            fits.insert((spec.build.clone(), sn.node_id.clone()), r.map_err(|e| format!("{}: {}", e.code, one_line(&e.hint))));
        }
    }
    let seat_only: Vec<SeatNode> = nodes.iter().map(|(sn, _)| sn.clone()).collect();
    let placed = rel::place(&seat_only, &asks, |build, node| fits.get(&(build.to_string(), node.to_string())).cloned().unwrap_or_else(|| Err("not asked".into())))
        .map_err(|e| no(e.code, 409, e.message))?;
    Ok(Planned { cfg: rcfg, sha, tree, seat, placed, specs, local_names })
}

fn placed_json(p: &[Placed]) -> Value {
    json!(p)
}

// ── POST /cli/ci/release ─────────────────────────────────────────────

pub(crate) fn release(who: &Who, body: &[u8]) -> R<Value> {
    let v = json_body(body)?;
    let ws = reader(who, s(&v, "workspace"))?;
    let version = s(&v, "version").ok_or_else(|| no("bad_request", 400, "the version to release: k2 ci release <x.y.z>"))?;
    if !rel::valid_version(version) {
        return Err(no(rcodes::BAD_VERSION, 400, format!("'{version}' is not a release version (x.y.z, optionally -pre or +build)")));
    }
    let run_id = ci_engine::new_run_id();
    let p = plan_set(&ws, &v, &run_id)?;
    if b(&v, "plan") == Some(true) {
        return Ok(json!({
            "workspace": ws.handle, "plan": true, "version": version, "project": p.cfg.project, "sha": p.sha, "tree": p.tree,
            "seat": p.seat.seat_id, "owner": p.cfg.owner, "lane": p.cfg.lane, "placed": placed_json(&p.placed),
        }));
    }
    let by = who.label();
    let at = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::insert_run(
            &conn,
            &ci::NewRun {
                id: &run_id,
                workspace_id: &ws.id,
                sha: &p.sha,
                git_ref: None,
                trigger: "manual",
                state: run_state::RUNNING,
                detail: Some(&format!("release {version} of {} on {}'s seat {}", p.cfg.project, p.cfg.owner, p.seat.seat_id)),
                created_by: &by,
                tier: "ci",
                statuses: None,
                at,
            },
        )
        .map_err(|e| no("internal", 500, e))?;
        rel::mark_release(&conn, &run_id, version, &p.cfg.project, &p.tree, &p.seat.seat_id).map_err(|e| no("internal", 500, e))?;
        for pl in &p.placed {
            rel::insert_job(&conn, &run_id, pl, check_state::PENDING, "submitting").map_err(|e| no("internal", 500, e))?;
        }
    }
    // Submit the set; a refusal cancels what was queued and ends the run.
    let mut submitted: Vec<String> = Vec::new();
    let mut failed: Option<String> = None;
    for (i, pl) in p.placed.iter().enumerate() {
        let spec = p.specs.iter().find(|sp| sp.label == pl.label).expect("a spec per placed label");
        let mut body = spec.body.clone();
        body["clientJobId"] = json!(format!("rel-{run_id}-{i:02}"));
        body["only"] = json!([p.local_names.get(&pl.node_id).cloned().unwrap_or_else(|| pl.node_id.clone())]);
        let (job, state, desc) = if failed.is_some() {
            (None, check_state::ERROR, "not submitted: another job of the set was refused".to_string())
        } else {
            match crate::compute_routes::submit_for_ci(&ws.id, &run_id, &body) {
                Ok(r) => match r["job"]["id"].as_str() {
                    Some(id) => {
                        submitted.push(id.to_string());
                        (Some(id.to_string()), check_state::PENDING, format!("job {} queued on {} ({})", &id[..id.len().min(8)], pl.node, pl.why))
                    }
                    None => {
                        failed = Some(format!("{}: compute answered without a job id", pl.label));
                        (None, check_state::ERROR, "not submitted: compute answered without a job id".into())
                    }
                },
                Err(e) => {
                    failed = Some(format!("{} on {}: {}: {}", pl.label, pl.node, e.code, one_line(&e.hint)));
                    (None, check_state::ERROR, format!("not submitted: {}: {}", e.code, one_line(&e.hint)))
                }
            }
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        ci::set_check(&conn, &run_id, &pl.label, job.as_deref(), state, &desc);
    }
    if let Some(why) = &failed {
        let agent = Who::Agent { project_id: ws.id.clone(), session_id: format!("ci-{}", &run_id[..8]) };
        for job in &submitted {
            let _ = crate::compute_views::job_cancel(&agent, json!({"job": job, "ifState": "queued"}).to_string().as_bytes());
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        for c in ci::checks_of(&conn, &run_id).iter().filter(|c| !c.is_final()) {
            ci::set_check(&conn, &run_id, &c.name, None, check_state::ERROR, "cancelled: the set is placed as one unit and another job was refused");
        }
        ci::set_run_state(&conn, &run_id, run_state::ERROR, Some(&format!("the set was not submitted whole: {why}")), Some(now()));
    }
    audit(
        who,
        "ci.release",
        None,
        Some(&ws.id),
        None,
        json!({"run": run_id, "version": version, "project": p.cfg.project, "sha": p.sha, "seat": p.seat.seat_id, "placed": placed_json(&p.placed), "error": failed}),
    );
    let db = k2_core::db::shared();
    let conn = db.lock();
    let r = ci::run_by_id(&conn, &run_id).ok_or_else(|| no("internal", 500, "the run was not stored"))?;
    Ok(json!({ "workspace": ws.handle, "run": crate::ci_routes::run_view(&conn, &r), "placed": placed_json(&p.placed) }))
}

// ── POST /cli/ci/evidence ────────────────────────────────────────────

/// The base64 DER SPKI of a pinned PEM key.
fn spki_b64(pem: &str) -> Option<String> {
    crypto::pem_decode("PUBLIC KEY", pem).ok().map(|d| crypto::b64(&d))
}

pub(crate) fn evidence(who: &Who, body: &[u8]) -> R<Value> {
    let v = json_body(body)?;
    let ws = reader(who, s(&v, "workspace"))?;
    let token = s(&v, "run").ok_or_else(|| no("bad_request", 400, "which release run: k2 ci evidence <run> --out <dir>"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let run = ci::find_run(&conn, &ws.id, token).ok_or_else(|| no(ci::codes::NO_RUN, 404, format!("no CI run '{token}' in workspace {} (k2 ci status lists them)", ws.handle)))?;
    let (kind, facts) = rel::release_facts(&conn, &run.id).ok_or_else(|| no(ci::codes::NO_RUN, 404, "the run is gone"))?;
    if kind != rel::kind::RELEASE {
        return Err(no(rcodes::NOT_A_RELEASE, 409, format!("run {} checks a commit; evidence is for release runs (k2 ci release)", run.short())));
    }
    let checks = ci::checks_of(&conn, &run.id);
    let jobs = rel::job_facts(&conn, &run.id);
    let not_done: Vec<String> = checks.iter().filter(|c| c.state != check_state::SUCCESS).map(|c| format!("{} {}", c.name, c.state)).collect();
    if !not_done.is_empty() || run.state != run_state::PASSED {
        return Err(no(
            rcodes::NOT_DONE,
            409,
            format!("release run {} is {} ({}): evidence needs every job done with exit 0 (k2 ci status {})", run.short(), run.state, not_done.join(", "), run.short()),
        ));
    }
    let seat_id = facts.seat_id.clone().unwrap_or_default();
    let seat_row = trust::seat(&conn, "client", &seat_id).ok_or_else(|| no(rcodes::NO_SEAT, 404, format!("seat {seat_id} is no longer on this server")))?;
    let owner_key = seat_row.owner_key_fp.clone().unwrap_or_default();
    let roster = trust::latest_roster(&conn, &owner_key).ok_or_else(|| no(rcodes::NO_SEAT, 404, format!("no node roster from {} is held here", seat_row.owner_id)))?;
    let seat_stmt = seat_row.statement.clone().ok_or_else(|| no(rcodes::NO_SEAT, 404, format!("seat {seat_id} has no signed seat statement here")))?;
    let mut receipts: BTreeMap<String, EvidenceReceipt> = BTreeMap::new();
    let mut statements: BTreeMap<String, Signed> = BTreeMap::new();
    let mut pins: BTreeMap<String, String> = BTreeMap::new();
    let mut files: BTreeMap<String, Value> = BTreeMap::new();
    let mut shas: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for c in &checks {
        let jf = jobs.get(&c.name).ok_or_else(|| no("internal", 500, format!("job {} has no release facts", c.name)))?;
        let unusable = |why: String| no(rcodes::RECEIPT, 409, format!("{}: {why}", c.name));
        let job_id = c.job_id.as_deref().ok_or_else(|| unusable("never submitted".into()))?;
        let j = store::job_by_id(&conn, job_id).ok_or_else(|| unusable(format!("job {job_id} has no record here")))?;
        let raw = j.receipt_json.as_deref().ok_or_else(|| unusable("no receipt yet (the node sends it when the job ends)".into()))?;
        if j.receipt_ok != Some(true) {
            return Err(unusable("this server's check of the receipt failed (wrong key, plan or version): don't sign it".into()));
        }
        let signed: SignedReceipt = serde_json::from_str(raw).map_err(|e| unusable(format!("stored receipt: {e}")))?;
        let stored: Value = serde_json::from_str(raw).unwrap_or_default();
        let rc: Receipt = serde_json::from_str(&signed.body).map_err(|e| unusable(format!("receipt body: {e}")))?;
        if rc.v != RECEIPT_V2 {
            return Err(unusable(format!("receipt v{}, a release needs v2", rc.v)));
        }
        let st: Signed = serde_json::from_value(stored.get("statement").cloned().unwrap_or_default()).map_err(|_| unusable("the receipt came without its node statement".into()))?;
        statements.insert(st.digest(), st);
        let plan = j.plan().and_then(|p| serde_json::to_value(p).ok()).ok_or_else(|| unusable("the job's plan is gone".into()))?;
        let node = store::node_by_id(&conn, &j.node_id).ok_or_else(|| unusable("its node is gone from this server".into()))?;
        if let Some(spki) = spki_b64(&node.public_key_pem) {
            pins.insert(node.fingerprint.clone(), spki);
        }
        let role = if jf.role == "verify" { Role::Verify } else { Role::Build };
        receipts.insert(c.name.clone(), EvidenceReceipt { build: jf.build.clone(), role, body: signed.body.clone(), sig: signed.sig.clone(), plan });
        let arts: Vec<Value> = rc.artifacts.iter().map(|a| json!({"name": a.name, "sha256": a.sha256, "bytes": a.bytes})).collect();
        shas.insert(c.name.clone(), rc.artifacts.iter().map(|a| (a.name.clone(), a.sha256.clone())).collect());
        files.insert(c.name.clone(), json!({"job": j.id, "short": &j.id[..j.id.len().min(8)], "build": jf.build, "role": jf.role, "node": node.name, "artifacts": arts}));
    }
    // The dual builds as this server saw them (the signer decides).
    let mut dual: BTreeMap<String, Value> = BTreeMap::new();
    for (label, jf) in jobs.iter().filter(|(_, j)| j.role == "verify") {
        if let Some((bl, _)) = jobs.iter().find(|(_, b)| b.role == "build" && b.build == jf.build) {
            let d = rel::dual_compare(shas.get(bl).unwrap_or(&BTreeMap::new()), shas.get(label).unwrap_or(&BTreeMap::new()));
            dual.insert(jf.build.clone(), json!({"build": bl, "verify": label, "compare": d}));
        }
    }
    drop(conn);
    let o = ci_engine::ops();
    let root = o.repo_root(Path::new(&ws.path)).map_err(|e| no(ci::codes::NOT_A_REPO, 409, e))?;
    let commit_object = git_out(&root, &["cat-file", "commit", &run.sha]).map_err(|e| no(ci::codes::NO_COMMIT, 404, e))?;
    let tree = facts.tree.clone().ok_or_else(|| no("internal", 500, "the run has no tree"))?;
    let controller_fp = crate::compute_admin::trust_controller_key()?.fingerprint();
    let bundle = EvidenceBundle {
        v: EVIDENCE_V,
        kind: EVIDENCE_KIND.to_string(),
        project: facts.project.clone().unwrap_or_default(),
        version: facts.version.clone(),
        run_id: run.id.clone(),
        controller_fp,
        created_at: now(),
        source: EvidenceSource { commit: run.sha.clone(), tree, commit_object: Some(crypto::b64(&commit_object)) },
        receipts,
        statements: statements.into_values().collect(),
        rotations: Vec::new(),
        attestations: Vec::new(),
        rosters: vec![roster.signed.clone()],
        seats: vec![seat_stmt],
        pairing_pins: pins,
    };
    bundle.check().map_err(|e| no(rcodes::RECEIPT, 409, format!("the evidence bundle doesn't check: {e}")))?;
    audit(who, "ci.evidence", None, Some(&ws.id), None, json!({"run": run.id, "version": facts.version, "jobs": files.keys().collect::<Vec<_>>()}));
    let attested: Vec<String> = roster.roster.entries.iter().filter(|e| e.attestation_digest.is_some()).map(|e| e.name.clone()).collect();
    let mut notes: Vec<String> = Vec::new();
    if !attested.is_empty() {
        notes.push(format!(
            "the owner vouches for {} by attestation; this server holds no attestation documents (owners don't send them yet), so a signer that checks owner_attested keys refuses this bundle",
            attested.join(", ")
        ));
    }
    Ok(json!({
        "workspace": ws.handle,
        "run": run.id,
        "short": run.short(),
        "version": facts.version,
        "project": facts.project,
        "file": format!("evidence-{}-{}.json", facts.project.clone().unwrap_or_default(), facts.version.clone().unwrap_or_default()),
        "bundle": bundle,
        "jobs": files,
        "dual": dual,
        "notes": notes,
    }))
}

// ── `k2 app updates push --evidence <run>` (C8) ──────────────────────

/// The release run a push names, with every file its jobs' signed receipts
/// list (`sha256 → (label, name)`).
pub(crate) struct PushEvidence {
    pub run_id: String,
    pub short: String,
    pub project: Option<String>,
    pub sha: String,
    files: BTreeMap<String, (String, String)>,
}

impl PushEvidence {
    /// Each pushed file (`(platform, sha256)`) must be one the run's
    /// receipts list: what the summary records (`platform → label/name`).
    pub fn match_files(&self, pushed: &[(String, String)]) -> Result<Value, (&'static str, String)> {
        let mut matched = serde_json::Map::new();
        for (platform, sha) in pushed {
            let (label, name) = self.files.get(sha).ok_or_else(|| {
                (
                    EVIDENCE_MISMATCH,
                    format!("the {platform} file is not one release run {} built (its sha256 {} is in none of the run's receipts): push the file k2 ci evidence fetched", self.short, &sha[..sha.len().min(12)]),
                )
            })?;
            matched.insert(platform.clone(), json!({"label": label, "name": name, "sha256": sha}));
        }
        Ok(json!({"run": self.run_id, "files": matched}))
    }
}

/// `--evidence`'s refusal when a pushed file isn't the run's.
pub const EVIDENCE_MISMATCH: &str = "evidence_mismatch";

/// Find the run (`ws_id`'s, by id, commit or branch) for a push of
/// `version`: a release run that passed, for that version.
pub(crate) fn evidence_for_push(ws_id: &str, token: &str, version: &str) -> Result<PushEvidence, (&'static str, u16, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let run = ci::find_run(&conn, ws_id, token).ok_or((ci::codes::NO_RUN, 404, format!("--evidence {token}: no CI run of the App's workspace by that name (k2 ci status lists them)")))?;
    let (kind, facts) = rel::release_facts(&conn, &run.id).ok_or((ci::codes::NO_RUN, 404, "the run is gone".to_string()))?;
    if kind != rel::kind::RELEASE {
        return Err((rcodes::NOT_A_RELEASE, 409, format!("--evidence {}: a check run, not a release (k2 ci release)", run.short())));
    }
    if run.state != run_state::PASSED {
        return Err((rcodes::NOT_DONE, 409, format!("--evidence {}: the release run is {}, not passed", run.short(), run.state)));
    }
    if facts.version.as_deref() != Some(version) {
        return Err((rcodes::BAD_VERSION, 409, format!("--evidence {} built version {}, not {version}", run.short(), facts.version.as_deref().unwrap_or("?"))));
    }
    let mut files = BTreeMap::new();
    for c in ci::checks_of(&conn, &run.id) {
        let Some(j) = c.job_id.as_deref().and_then(|id| store::job_by_id(&conn, id)) else { continue };
        if j.receipt_ok != Some(true) {
            continue;
        }
        let Some(rc) = j
            .receipt_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<SignedReceipt>(raw).ok())
            .and_then(|s| serde_json::from_str::<Receipt>(&s.body).ok())
        else {
            continue;
        };
        for a in rc.artifacts {
            files.entry(a.sha256.clone()).or_insert((c.name.clone(), a.name.clone()));
        }
    }
    Ok(PushEvidence { run_id: run.id.clone(), short: run.short().to_string(), project: facts.project, sha: run.sha.clone(), files })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_matches_files_by_sha_or_refuses() {
        let ev = PushEvidence {
            run_id: "r".repeat(32),
            short: "rrrrrrrr".into(),
            project: Some("acme".into()),
            sha: "a".repeat(40),
            files: BTreeMap::from([("1".repeat(64), ("L1".to_string(), "k2/out/acme.AppImage".to_string()))]),
        };
        let ok = ev.match_files(&[("linux-x86_64".into(), "1".repeat(64))]).unwrap();
        assert_eq!(ok["files"]["linux-x86_64"]["label"], "L1");
        assert_eq!(ok["files"]["linux-x86_64"]["name"], "k2/out/acme.AppImage");
        let (code, why) = ev.match_files(&[("linux-x86_64".into(), "1".repeat(64)), ("darwin-aarch64".into(), "2".repeat(64))]).unwrap_err();
        assert_eq!(code, EVIDENCE_MISMATCH);
        assert!(why.contains("darwin-aarch64") && why.contains("rrrrrrrr"), "{why}");
    }

    #[test]
    fn a_build_body_sets_only_the_build_facts() {
        let cfg = CiConfig::parse(
            "version = 1\n[release]\nproject = \"acme\"\nowner = \"o.example\"\nlane = \"ci-standard\"\n[release.builds.linux-x86_64]\nargv = [\"bash\", \"b.sh\"]\nneeds = [\"rust@1.99.1\"]\nartifacts = [\"k2/out/*\"]\nenv = [\"BUILD_COMMIT\", \"BUILD_TREE\", \"EXTRA\"]\nsecrets = [\"API\"]\nlabel = \"L1\"\nmem_gb = 16\ntimeout_secs = 600\n",
        )
        .unwrap();
        let b = &cfg.release.as_ref().unwrap().builds["linux-x86_64"];
        let v = build_body("linux-x86_64", "L1", b, &"c".repeat(40), &"d".repeat(40), "ci");
        assert_eq!(v["env"]["BUILD_COMMIT"], "c".repeat(40));
        assert_eq!(v["env"]["BUILD_TREE"], "d".repeat(40));
        assert_eq!(v["env"]["EXTRA"], "", "signer M1: other names carry no value");
        assert_eq!(v["receiptV"], 2);
        assert_eq!(v["secrets"], json!([{"name": "API", "env": "API"}]));
        assert_eq!((v["memBytes"].as_u64(), v["timeoutSecs"].as_u64()), (Some(16 << 30), Some(600)));
        assert_eq!(v["tag"], "release:linux-x86_64:L1");
        assert_eq!((v["tier"].as_str(), v["detach"].as_bool()), (Some("ci"), Some(true)));
        assert!(v.get("supersede").is_none(), "a release never replaces another's jobs");
    }
}
