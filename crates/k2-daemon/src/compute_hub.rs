//! The compute hub, hub side (`prd-compute-shared-queue-v1` S1/S2;
//! 0.46.3, Rosson 2026-10-20). One K2 server owns the nodes and the queue;
//! other K2 servers (tenants) submit to it.
//!
//! **Tenant routes** (`/cli/compute/hub/*`, POST, route floor `Public`):
//! no token. Every request carries the signed `X-K2-Hub` header
//! ([`k2_core::compute::hub`]); [`authenticate`] verifies it fail-closed
//! (skew, pinned federation key, `require_peer(fp, "compute")`, pairing
//! v2, tenant state, epoch, nonce) and the verb then runs as
//! [`Who::Tenant`] through the same code as a local caller. Request
//! parameters ride in the signed JSON body (reads included), so the
//! signature covers them; the blob upload streams its body and is signed
//! over its sha256 (the `sha256` parameter).
//!
//! | route | what |
//! |---|---|
//! | `hello` | the tenant's id, label, epoch and limits (any epoch: how a caller learns it) |
//! | `nodes` | the nodes this tenant is granted (redacted: no fingerprints) |
//! | `run` | submit, as `/cli/compute/run` (source from the tenant's store) |
//! | `jobs`, `jobs/get`, `jobs/logs`, `jobs/artifact`, `jobs/receipt` | reads, tenant-scoped |
//! | `jobs/cancel`, `jobs/retry` | the tenant's own jobs only |
//! | `changes` | long-poll: state of the listed jobs (the submitter's poller) |
//! | `prepare` | which of the listed commits the tenant's hub mirror has |
//! | `blob` | streaming upload of a bundle or a `--dirty` blob |
//!
//! **Owner routes** (local, `/cli/compute/tenants*`): list (admin), add /
//! set / suspend / resume / revoke / grant / ungrant (owner only: adding or
//! widening a tenant needs the owner, never an agent, even with "Allow
//! managing compute nodes"). Until the runtime wall (S6) only `own`
//! tenants are accepted (`wall_required`).
//!
//! For S3 (fair queue v2), S4 (reservations) and S7 (UI): tenant rows and
//! grants are in `k2_core::compute::hub`; a job's tenant/class are on
//! `JobRow`; [`TENANT_STATE_WAIT`] is the waiting reason a suspended
//! tenant's queued job shows (`compute_ws::decision_for`).

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use k2_core::compute::hub::{self as hub, codes as hcodes, TenantRow};
use k2_core::compute::proto::crypto;
use k2_core::compute::proto::frames::{Blob, JobState, Src};
use k2_core::compute::{self as compute, codes, store};
use k2_core::federation::PeerStore;

use crate::cli_response::CliResponse;
use crate::compute_routes::{self as cr, json_body, n, no, refusal_response, s, TenantCaller, Who, R};
use crate::compute_ws;

pub const HELLO_PATH: &str = "/cli/compute/hub/hello";
pub const BLOB_PATH: &str = "/cli/compute/hub/blob";

/// Every tenant-facing hub route (all POST).
pub const ROUTES: &[&str] = &[
    "/cli/compute/hub/blob",
    "/cli/compute/hub/changes",
    "/cli/compute/hub/hello",
    "/cli/compute/hub/jobs",
    "/cli/compute/hub/jobs/artifact",
    "/cli/compute/hub/jobs/cancel",
    "/cli/compute/hub/jobs/get",
    "/cli/compute/hub/jobs/logs",
    "/cli/compute/hub/jobs/receipt",
    "/cli/compute/hub/jobs/retry",
    "/cli/compute/hub/nodes",
    "/cli/compute/hub/prepare",
    "/cli/compute/hub/run",
];

/// The hub owner's tenant routes (local callers, through `compute_routes`).
pub const ADMIN_ROUTES: &[&str] = &[
    "/cli/compute/tenants",
    "/cli/compute/tenants/add",
    "/cli/compute/tenants/grant",
    "/cli/compute/tenants/resume",
    "/cli/compute/tenants/revoke",
    "/cli/compute/tenants/set",
    "/cli/compute/tenants/suspend",
    "/cli/compute/tenants/ungrant",
];

/// The waiting reason of a queued job whose tenant is suspended.
pub const TENANT_STATE_WAIT: &str = "tenant_suspended";

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_admin_route(p: &str) -> bool {
    ADMIN_ROUTES.contains(&p)
}

// ── authentication ───────────────────────────────────────────────────

/// What the federation store says about `fp`, for [`hub::verify_request`].
fn peer_facts(fp: &str) -> R<Option<hub::PeerFacts>> {
    let store = PeerStore::load().map_err(|e| no("internal", 500, format!("federation peers: {e}")))?;
    let Some(p) = store.get(fp) else { return Ok(None) };
    let spki = crypto::pem_decode("PUBLIC KEY", &p.public_key_pem).map_err(|e| no("internal", 500, format!("pinned key: {e}")))?;
    Ok(Some(hub::PeerFacts {
        spki_der: spki,
        trusted: p.trust == k2_core::federation::PeerTrust::Trusted,
        has_compute: p.capabilities.contains(hub::CAP_COMPUTE),
        pairing_version: p.pairing_version,
    }))
}

/// Verify a tenant request (fail-closed). `body_sha` = sha256 hex of the
/// body (for the blob route, the declared blob hash, re-checked after
/// streaming).
pub(crate) fn authenticate(path: &str, method: &str, raw_query: &str, header: Option<&str>, body_sha: &str) -> R<TenantRow> {
    let a = header
        .and_then(hub::HubAuth::parse)
        .ok_or_else(|| no(hcodes::AUTH_MISSING, 401, format!("a hub request needs a signed {} header (this route has no token)", hub::HEADER)))?;
    let peer = peer_facts(&a.fp)?;
    let tenant = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        hub::tenant_by_fp(&conn, &a.fp)
    };
    hub::verify_request(&a, peer.as_ref(), tenant.as_ref(), method, path, raw_query, body_sha, compute::now(), path == HELLO_PATH)
        .map_err(|(code, status, hint)| no(code, status, hint))
}

fn opt_str(v: &serde_json::Value, k: &str, max: usize) -> R<Option<String>> {
    match v.get(k) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(x)) if x.chars().count() <= max && !x.chars().any(char::is_control) => {
            Ok(Some(x.trim().to_string()).filter(|x| !x.is_empty()))
        }
        _ => Err(no(codes::BAD_REQUEST, 400, format!("{k} is a short string"))),
    }
}

/// The tenant caller a verified request acts as.
fn caller(t: &TenantRow, v: &serde_json::Value) -> R<Who> {
    let remote_ws = opt_str(v, "remoteWs", 128)?;
    if remote_ws.as_deref().is_some_and(|w| !hub::valid_remote_id(w)) {
        return Err(no(codes::BAD_REQUEST, 400, "remoteWs is 1–128 of A-Z a-z 0-9 - _ ."));
    }
    Ok(Who::Tenant(Box::new(TenantCaller {
        tenant_id: t.id.clone(),
        label: t.label.clone(),
        peer_fp: t.peer_fp.clone().unwrap_or_default(),
        remote_ws,
        remote_ws_label: opt_str(v, "remoteWsLabel", 128)?,
        remote_session: opt_str(v, "remoteSession", 128)?,
    })))
}

/// A JSON body as the `params` map the read verbs take.
fn params_of(v: &serde_json::Value) -> HashMap<String, String> {
    v.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, x)| match x {
                    serde_json::Value::String(s) => Some((k.clone(), s.clone())),
                    serde_json::Value::Number(n) => Some((k.clone(), n.to_string())),
                    serde_json::Value::Bool(b) => Some((k.clone(), b.to_string())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Serve one tenant request (not the blob upload). Blocking.
pub fn handle_request(
    path: &str,
    method: &str,
    raw_query: &str,
    header: Option<&str>,
    body: &[u8],
    ingress: crate::routes::dispatcher::Ingress,
) -> CliResponse {
    if !compute::enabled() {
        return cr::dark();
    }
    compute_ws::ensure_ticker_started();
    let r = (|| -> R<serde_json::Value> {
        let t = authenticate(path, method, raw_query, header, &crypto::sha256_hex(body))?;
        let v = json_body(body)?;
        let who = caller(&t, &v)?;
        match path {
            HELLO_PATH => Ok(hello(&t)),
            "/cli/compute/hub/nodes" => tenant_nodes(&t),
            "/cli/compute/hub/run" => cr::run(&who, body),
            "/cli/compute/hub/jobs" => cr::jobs_list(&who, &params_of(&v)),
            "/cli/compute/hub/jobs/get" => cr::job_get(&who, &params_of(&v)),
            "/cli/compute/hub/jobs/logs" => cr::job_logs(&who, &params_of(&v)),
            "/cli/compute/hub/jobs/artifact" => cr::job_artifact(&who, &params_of(&v)),
            "/cli/compute/hub/jobs/receipt" => cr::job_receipt(&who, &params_of(&v)),
            "/cli/compute/hub/jobs/cancel" => cr::job_cancel(&who, body),
            "/cli/compute/hub/jobs/retry" => cr::job_retry(&who, body),
            "/cli/compute/hub/changes" => changes(&who, &v),
            "/cli/compute/hub/prepare" => prepare(&who, &v, &t, ingress),
            _ => Err(no("not_found", 404, "no such hub route")),
        }
    })();
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

fn hello(t: &TenantRow) -> serde_json::Value {
    serde_json::json!({
        "tenant": t.id,
        "label": t.label,
        "epoch": t.epoch,
        "trust": t.trust,
        "weight": t.weight,
        "topClass": t.top_class,
        "maxQueued": t.max_queued,
        "hub": compute_ws::controller_label(),
        "hubFingerprint": k2_core::federation::local_fingerprint().ok(),
    })
}

/// The nodes this tenant may use, without anything only the hub owner
/// should see (fingerprints, SAS, other tenants' grants).
fn tenant_nodes(t: &TenantRow) -> R<serde_json::Value> {
    let rows: Vec<(store::NodeRow, hub::TenantGrant)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        hub::tenant_grants(&conn, Some(&t.id), None)
            .into_iter()
            .filter_map(|g| store::node_by_id(&conn, &g.node_id).filter(|n| n.state == "active").map(|n| (n, g)))
            .collect()
    };
    let nodes: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(n, g)| {
            let (online, route, live) = compute_ws::live_view(&n.id);
            let offer = live.or_else(|| n.last_offer.clone());
            let (queued, running) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                (store::jobs_in_states(&conn, &n.id, &["queued"]).len(), store::jobs_in_states(&conn, &n.id, store::ACTIVE_STATES).len())
            };
            serde_json::json!({
                "name": n.name,
                "state": n.state,
                "os": n.os,
                "arch": n.arch,
                "labels": n.labels,
                "online": online,
                "liveRoute": route,
                "paused": n.controller_pause,
                "queued": queued,
                "running": running,
                "grant": {"maxParallel": g.max_parallel, "maxJobSecs": g.max_job_secs, "maxDiskGb": g.max_disk_gb, "maxQueued": g.max_queued},
                "offer": offer.map(|o| serde_json::json!({
                    "control": o.control.as_str(),
                    "available": o.availability.ok,
                    "caps": o.caps,
                    "free": o.free,
                    "parallelMax": o.parallel_max,
                    "tools": o.tools,
                    "features": o.features,
                })),
            })
        })
        .collect();
    Ok(serde_json::json!({ "nodes": nodes, "controller": compute_ws::controller_label(), "tenant": t.label }))
}

/// One job's state as the submitter's poller needs it.
fn change_row(j: &store::JobRow) -> serde_json::Value {
    let node = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id).map(|n| n.name)
    };
    let tests = compute::result::outputs(j).and_then(|o| o.tests);
    serde_json::json!({
        "id": j.id,
        "state": j.state,
        "reason": j.reason,
        "detail": j.detail,
        "exitCode": j.exit_code,
        "signal": j.signal,
        "node": node,
        "generation": j.generation,
        "attempt": j.attempt,
        "assignedAt": j.assigned_at,
        "startedAt": j.started_at,
        "endedAt": j.ended_at,
        "terminal": j.is_terminal(),
        "shards": j.ctl.shards.len(),
        "tests": tests,
    })
}

/// `changes {jobs: [{id, state}], wait}`: the listed jobs' current state,
/// returned as soon as any differs from what the caller last saw (or after
/// `wait` ≤ 25 s). Jobs that aren't this tenant's are `unknown`.
fn changes(who: &Who, v: &serde_json::Value) -> R<serde_json::Value> {
    let list = v.get("jobs").and_then(|x| x.as_array()).ok_or_else(|| no(codes::BAD_REQUEST, 400, "jobs is a list of {id, state}"))?;
    if list.len() > 500 {
        return Err(no(codes::BAD_REQUEST, 400, "at most 500 jobs per call"));
    }
    let seen: Vec<(String, String)> = list
        .iter()
        .filter_map(|x| Some((s(x, "id")?.to_string(), s(x, "state").unwrap_or("").to_string())))
        .collect();
    let wait = n(v, "wait").unwrap_or(0).clamp(0, 25) as u64;
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        let counter = compute_ws::change_counter();
        let rows: Vec<Option<store::JobRow>> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            seen.iter().map(|(id, _)| store::job_by_id(&conn, id).filter(|j| cr::visible_to(who, j))).collect()
        };
        let moved = rows.iter().zip(&seen).any(|(r, (_, st))| r.as_ref().is_some_and(|j| &j.state != st));
        let now = Instant::now();
        if moved || now >= deadline {
            let mut jobs = Vec::new();
            let mut unknown = Vec::new();
            for (r, (id, _)) in rows.iter().zip(&seen) {
                match r {
                    Some(j) => jobs.push(change_row(j)),
                    None => unknown.push(id.clone()),
                }
            }
            return Ok(serde_json::json!({ "jobs": jobs, "unknown": unknown }));
        }
        compute_ws::wait_change(counter, (deadline - now).min(Duration::from_secs(5)));
    }
}

// ── the tenant's code (S2) ───────────────────────────────────────────

fn is_hex40(c: &str) -> bool {
    c.len() == 40 && c.chars().all(|x| x.is_ascii_hexdigit() && !x.is_ascii_uppercase())
}

fn git_in(mirror: &Path) -> std::process::Command {
    let mut c = std::process::Command::new("git");
    c.arg("--git-dir").arg(mirror).env("GIT_TERMINAL_PROMPT", "0").env("GIT_CONFIG_NOSYSTEM", "1").stdin(std::process::Stdio::null());
    c
}

/// Does the tenant's hub mirror have `commit`?
pub fn mirror_has(tenant_id: &str, project_key: &str, commit: &str) -> bool {
    let m = hub::tenant_mirror(tenant_id, project_key);
    if !is_hex40(commit) || !m.join("objects").is_dir() {
        return false;
    }
    let mut c = git_in(&m);
    c.args(["cat-file", "-e", &format!("{commit}^{{commit}}")]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c.status().is_ok_and(|s| s.success())
}

/// The source of a tenant's job: its commit must be in the tenant's mirror
/// for this workspace and remote, its `--dirty` blob in the tenant's blob
/// store (V20). The mirror key is the hub's own (V28).
pub(crate) fn tenant_source(t: &TenantCaller, v: &serde_json::Value) -> R<(Src, cr::SourceInfo)> {
    let rws = t.remote_ws.as_deref().ok_or_else(|| no("workspace_not_found", 404, "a tenant request names its workspace (remoteWs)"))?;
    let src = v
        .get("source")
        .filter(|x| x.is_object())
        .ok_or_else(|| no(hcodes::SOURCE_MISSING, 409, "a tenant run carries source {commit, remote, dirty} (or noSource)"))?;
    let commit = s(src, "commit").filter(|c| is_hex40(c)).ok_or_else(|| no(codes::BAD_REQUEST, 400, "source.commit is a full 40-hex commit"))?;
    let remote = s(src, "remote").and_then(compute::sync::fetchable_remote);
    let pk = hub::hub_project_key(&t.tenant_id, rws, remote.as_deref());
    if !mirror_has(&t.tenant_id, &pk, commit) {
        return Err(no(
            hcodes::SOURCE_MISSING,
            409,
            format!("this hub doesn't have commit {} for that workspace yet: upload it first (prepare, then blob)", &commit[..8]),
        ));
    }
    let dirty = match src.get("dirty").filter(|d| !d.is_null()) {
        None => None,
        Some(d) => {
            let sha = s(d, "sha256").filter(|x| hub::is_sha256_hex(x)).ok_or_else(|| no(codes::BAD_REQUEST, 400, "source.dirty.sha256 is a sha256"))?;
            let known = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                hub::blob_known(&conn, &t.tenant_id, sha)
            };
            match known {
                Some((kind, bytes)) if kind == "dirty" && hub::blob_path(&t.tenant_id, sha).is_file() => Some(Blob { sha256: sha.to_string(), bytes }),
                _ => return Err(no(hcodes::SOURCE_MISSING, 409, "the --dirty blob isn't on this hub yet: upload it first (blob kind=dirty)")),
            }
        }
    };
    let info = cr::SourceInfo { checkout: "hub", path: String::new(), commit: commit.to_string(), dirty: dirty.as_ref().map(|d| (0, 0, d.bytes)) };
    Ok((Src { project_key: pk, remote_url: remote, commit: commit.to_string(), dirty, slots: 1 }, info))
}

/// `prepare {remoteWs, remote, commits: [...]}`: which of these commits the
/// tenant's mirror for this workspace already has (the submitter bundles
/// only `have..commit`, V20).
fn prepare(who: &Who, v: &serde_json::Value, row: &TenantRow, ingress: crate::routes::dispatcher::Ingress) -> R<serde_json::Value> {
    let t = who.tenant().ok_or_else(cr::owner_only)?;
    let rws = t.remote_ws.as_deref().ok_or_else(|| no("workspace_not_found", 404, "name the workspace (remoteWs)"))?;
    let remote = s(v, "remote").and_then(compute::sync::fetchable_remote);
    let pk = hub::hub_project_key(&t.tenant_id, rws, remote.as_deref());
    let commits: Vec<String> = v
        .get("commits")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|c| c.as_str()).filter(|c| is_hex40(c)).take(512).map(str::to_string).collect())
        .unwrap_or_default();
    let have: Vec<&String> = commits.iter().filter(|c| mirror_has(&t.tenant_id, &pk, c)).collect();
    // The caps an upload through this same door meets, so the submitter
    // refuses an oversized blob before it sends a byte (the blob route
    // closes on a refused body; a client mid-send sees a broken pipe).
    let used = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        hub::blob_bytes(&conn, &t.tenant_id)
    };
    Ok(serde_json::json!({
        "have": have,
        "mirror": hub::tenant_mirror(&t.tenant_id, &pk).join("objects").is_dir(),
        "maxBlobBytes": blob_cap_for(ingress),
        "blobFreeBytes": row.blob_cap_bytes().saturating_sub(used),
    }))
}

// ── owner: tenants ───────────────────────────────────────────────────

fn tenant_or_404(conn: &rusqlite::Connection, token: Option<&str>) -> R<TenantRow> {
    let t = token.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name a tenant"))?;
    hub::find_tenant(conn, t).ok_or_else(|| no("tenant_not_found", 404, format!("no tenant '{t}' (k2 compute tenants)")))
}

fn tenant_json(conn: &rusqlite::Connection, t: &TenantRow) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or_default();
    let count = |states: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM compute_jobs WHERE tenant_id = ?1 AND state IN ({states})"), [&t.id], |r| r.get(0)).unwrap_or(0)
    };
    v["queued"] = count("'queued'").into();
    v["running"] = count("'assigned','preparing','running','finishing'").into();
    v["blobBytes"] = hub::blob_bytes(conn, &t.id).into();
    v["grants"] = hub::tenant_grants(conn, Some(&t.id), None)
        .into_iter()
        .map(|g| {
            let mut x = serde_json::to_value(&g).unwrap_or_default();
            x["node"] = store::node_by_id(conn, &g.node_id).map(|n| n.name).into();
            x
        })
        .collect::<Vec<_>>()
        .into();
    v
}

/// Route a hub owner request (`/cli/compute/tenants*`). Local callers only.
pub(crate) fn admin(path: &str, who: &Who, body: &[u8], _params: &HashMap<String, String>) -> R<serde_json::Value> {
    if who.tenant().is_some() {
        return Err(cr::owner_only());
    }
    match path {
        "/cli/compute/tenants" => {
            cr::need_admin(who)?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            let list: Vec<serde_json::Value> = hub::list_tenants(&conn).iter().map(|t| tenant_json(&conn, t)).collect();
            Ok(serde_json::json!({ "tenants": list, "federation": k2_core::federation::enabled() }))
        }
        "/cli/compute/tenants/add" => tenants_add(who, body),
        "/cli/compute/tenants/set" => tenants_set(who, body),
        "/cli/compute/tenants/suspend" => tenants_state(who, body, "suspended"),
        "/cli/compute/tenants/resume" => tenants_state(who, body, "active"),
        "/cli/compute/tenants/revoke" => tenants_state(who, body, "revoked"),
        "/cli/compute/tenants/grant" => tenants_grant(who, body),
        "/cli/compute/tenants/ungrant" => tenants_ungrant(who, body),
        _ => Err(no("not_found", 404, "no such compute route")),
    }
}

fn audit_line(who: &Who, act: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(&format!("compute-{act}"), &who.label(), outcome, "loopback", "-", "cli"));
}

/// `tenants/add {peer, label?, trust?, weight?, topClass?, maxQueued?}`:
/// grant a paired server (Trusted, pairing v2) the peer capability
/// `compute` and make it a tenant. Owner only.
fn tenants_add(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    cr::need_owner(who)?;
    let v = json_body(body)?;
    if !k2_core::federation::enabled() {
        return Err(no("federation_off", 409, "the compute hub needs federation on (K2_FEDERATION=1 or Settings); tenants are paired servers"));
    }
    let trust = s(&v, "trust").unwrap_or("own");
    if trust == "team" {
        return Err(no(
            hcodes::WALL_REQUIRED,
            409,
            "another owner's server (team) needs the runtime wall on the nodes first; only your own servers (own) can be tenants for now",
        ));
    }
    if trust != "own" {
        return Err(no(codes::BAD_REQUEST, 400, "trust is own (team needs the runtime wall)"));
    }
    let sel = s(&v, "peer").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the paired server (peer: fingerprint, label or host)"))?;
    let store = PeerStore::load().map_err(|e| no("internal", 500, format!("federation peers: {e}")))?;
    let peer = store
        .list()
        .iter()
        .find(|p| crate::federation_routes::peer_selector_matches(p, sel))
        .cloned()
        .ok_or_else(|| no("peer_not_found", 404, format!("no paired server '{sel}' (k2 fed peers)")))?;
    if peer.trust != k2_core::federation::PeerTrust::Trusted {
        return Err(no(hcodes::PEER_NOT_GRANTED, 409, format!("'{sel}' isn't confirmed (Trusted) yet: finish pairing first")));
    }
    if peer.pairing_version < k2_core::federation::pairing_v2::PAIRING_VERSION {
        return Err(no(hcodes::PAIRING_V2_REQUIRED, 409, format!("'{sel}' was paired with the old code; pair the two servers again (pairing v2) before giving it compute")));
    }
    let label = s(&v, "label").map(str::to_string).unwrap_or_else(|| if peer.label.is_empty() { peer.subdomain.clone() } else { peer.label.clone() });
    let weight = n(&v, "weight").unwrap_or(1);
    let top_class = s(&v, "topClass").unwrap_or("interactive");
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        hub::insert_server_tenant(
            &conn,
            &hub::NewTenant { label: &label, peer_fp: &peer.fingerprint, trust, weight, top_class, max_queued: n(&v, "maxQueued"), by: &who.label() },
            compute::now(),
        )
        .map_err(|e| no(codes::BAD_REQUEST, 409, e))?
    };
    PeerStore::transact(|st| -> Result<((), bool), String> { Ok(((), st.grant(&peer.fingerprint, hub::CAP_COMPUTE))) })
        .map_err(|e| no("internal", 500, e))?
        .map_err(|e| no("internal", 500, e))?;
    cr::audit(who, "tenant_add", None, None, None, serde_json::json!({"tenant": row.id, "label": row.label, "peer": peer.fingerprint, "weight": weight, "topClass": top_class}));
    audit_line(who, "tenant-add", format!("ok tenant-add tenant={} label={} peer={} weight={weight} top_class={top_class}", row.id, row.label, &peer.fingerprint[..16]));
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(serde_json::json!({ "ok": true, "tenant": tenant_json(&conn, &row) }))
}

fn opt_patch(v: &serde_json::Value, k: &str) -> Option<Option<i64>> {
    match v.get(k) {
        None => None,
        Some(serde_json::Value::Null) => Some(None),
        Some(x) => x.as_i64().map(Some),
    }
}

fn tenants_set(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    cr::need_owner(who)?;
    let v = json_body(body)?;
    let patch = hub::TenantPatch {
        label: s(&v, "label").map(str::to_string),
        weight: n(&v, "weight"),
        top_class: s(&v, "topClass").map(str::to_string),
        max_running: opt_patch(&v, "maxRunning"),
        max_queued: opt_patch(&v, "maxQueued"),
        cpu_hours_day: opt_patch(&v, "cpuHoursDay"),
        log_gb: opt_patch(&v, "logGb"),
        artifact_days: opt_patch(&v, "artifactDays"),
        blob_mb: opt_patch(&v, "blobMb"),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let t = tenant_or_404(&conn, s(&v, "tenant"))?;
    let row = hub::update_tenant(&conn, &t.id, &patch).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    store::record_event(&conn, &who.label(), "tenant_set", None, None, None, serde_json::json!({"tenant": row.id, "set": v}));
    drop(conn);
    audit_line(who, "tenant-set", format!("ok tenant-set tenant={} weight={} top_class={}", row.id, row.weight, row.top_class));
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(serde_json::json!({ "ok": true, "tenant": tenant_json(&conn, &row) }))
}

/// Suspend (no new starts, signed calls refused), resume, or revoke (the
/// peer loses `compute`, its epoch bumps, queued jobs are cancelled and
/// running ones stopped: reason `revoked`, V25).
fn tenants_state(who: &Who, body: &[u8], state: &str) -> R<serde_json::Value> {
    cr::need_owner(who)?;
    let v = json_body(body)?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let t = tenant_or_404(&conn, s(&v, "tenant"))?;
        let row = hub::set_tenant_state(&conn, &t.id, state).map_err(|e| no(codes::BAD_REQUEST, 409, e))?;
        store::record_event(&conn, &who.label(), &format!("tenant_{state}"), None, None, None, serde_json::json!({"tenant": row.id, "epoch": row.epoch}));
        row
    };
    let mut stopped = 0usize;
    if state == "revoked" {
        if let Some(fp) = row.peer_fp.clone() {
            PeerStore::transact(|st| -> Result<((), bool), String> {
                let a = st.revoke_capability(&fp, hub::CAP_COMPUTE);
                let b = st.bump_epoch(&fp).is_some();
                Ok(((), a || b))
            })
            .map_err(|e| no("internal", 500, e))?
            .map_err(|e| no("internal", 500, e))?;
        }
        stopped = stop_tenant_jobs(&row.id, &who.label());
    }
    audit_line(who, &format!("tenant-{state}"), format!("ok tenant-{state} tenant={} epoch={} stopped={stopped}", row.id, row.epoch));
    compute_ws::notify_change();
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(serde_json::json!({ "ok": true, "tenant": tenant_json(&conn, &row), "stopped": stopped }))
}

/// A revoked tenant's queued jobs end `cancelled/revoked`; running ones are
/// told to stop. Returns how many were touched.
fn stop_tenant_jobs(tenant_id: &str, by: &str) -> usize {
    let open: Vec<store::JobRow> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_jobs(&conn, &store::JobFilter { tenant_id: Some(tenant_id), limit: 500, ..Default::default() })
            .into_iter()
            .filter(|j| !j.is_terminal())
            .collect()
    };
    let mut n = 0;
    for j in open {
        let done = compute_ws::with_kick_lock(|| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            if store::set_job_state_if(&conn, &j.id, "queued", JobState::Cancelled, Some("revoked"), Some("the hub owner revoked this server's compute"), compute::now()) == Ok(true) {
                store::record_event(&conn, by, "job_cancel", Some(&j.node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"state": "queued", "reason": "revoked"}));
                return true;
            }
            let Some(cur) = store::job_by_id(&conn, &j.id).filter(|c| !c.is_terminal() && c.state != "queued") else { return false };
            if !cur.node_id.is_empty() {
                compute_ws::send_cancel(&cur.node_id, &cur.id, cur.generation);
            }
            store::set_job_reason(&conn, &cur.id, Some("cancel_requested"), Some("the hub owner revoked this server's compute"));
            store::record_event(&conn, by, "job_cancel", Some(&cur.node_id), Some(&cur.workspace_id), Some(&cur.id), serde_json::json!({"state": cur.state, "reason": "revoked"}));
            true
        });
        if done {
            n += 1;
        }
    }
    n
}

fn tenants_grant(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    cr::need_owner(who)?;
    let v = json_body(body)?;
    let node = cr::node_or_404(s(&v, "node"))?;
    let pos = |k: &str, lo: i64, hi: i64| -> R<Option<i64>> {
        match n(&v, k) {
            None => Ok(None),
            Some(x) if (lo..=hi).contains(&x) => Ok(Some(x)),
            Some(_) => Err(no(codes::BAD_REQUEST, 400, format!("{k} is {lo}–{hi}"))),
        }
    };
    let limits = store::GrantLimits {
        max_job_secs: pos("maxJobSecs", 60, 7 * 86_400)?,
        max_disk_gb: pos("maxDiskGb", 1, 10_000)?,
        max_parallel: pos("maxParallel", 1, 64)?,
        max_queued: pos("maxQueued", 1, 1000)?,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let t = tenant_or_404(&conn, s(&v, "tenant"))?;
    if t.is_local() {
        return Err(no(codes::BAD_REQUEST, 400, "the local tenant uses workspace grants (k2 compute grant <node> --workspace WS)"));
    }
    if t.state == "revoked" {
        return Err(no(hcodes::TENANT_REVOKED, 409, "that tenant is revoked"));
    }
    let g = hub::set_tenant_grant(&conn, &t.id, &node.id, &limits, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
    store::record_event(&conn, &who.label(), "tenant_grant", Some(&node.id), None, None, serde_json::json!({"tenant": t.id, "grant": g}));
    drop(conn);
    audit_line(who, "tenant-grant", format!("ok tenant-grant tenant={} node={} parallel={} max_secs={}", t.id, node.name, g.max_parallel, g.max_job_secs));
    compute_ws::kick(&node.id);
    Ok(serde_json::json!({ "ok": true, "tenant": t.label, "node": node.name, "grant": g }))
}

fn tenants_ungrant(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    cr::need_owner(who)?;
    let v = json_body(body)?;
    let node = cr::node_or_404(s(&v, "node"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let t = tenant_or_404(&conn, s(&v, "tenant"))?;
    let removed = hub::revoke_tenant_grant(&conn, &t.id, &node.id).map_err(|e| no("internal", 500, e))?;
    store::record_event(&conn, &who.label(), "tenant_ungrant", Some(&node.id), None, None, serde_json::json!({"tenant": t.id, "removed": removed}));
    Ok(serde_json::json!({ "ok": true, "removed": removed }))
}

// ── blob upload (S2, V20/V21) ────────────────────────────────────────

/// The largest blob one upload may carry on this door: the relay cap over
/// the Connect tunnel, the LAN cap on a direct (LAN / loopback) door.
pub fn blob_cap_for(ingress: crate::routes::dispatcher::Ingress) -> u64 {
    match ingress {
        crate::routes::dispatcher::Ingress::Tunnel => hub::RELAY_MAX_BLOB_BYTES,
        _ => hub::LAN_MAX_BLOB_BYTES,
    }
}

fn blob_refusal(code: &'static str, status: u16, hint: impl Into<String>) -> CliResponse {
    refusal_response(&no(code, status, hint))
}

/// What a blob upload names in its (signed) query.
#[derive(Debug, Clone)]
struct BlobAsk {
    sha256: String,
    kind: String,
    remote_ws: Option<String>,
    remote: Option<String>,
    commit: Option<String>,
}

fn blob_ask(raw_query: &str) -> R<BlobAsk> {
    let p = k2_core::agent_hooks::parse_query_params(&format!("{BLOB_PATH}?{raw_query}"));
    let sha256 = p.get("sha256").filter(|x| hub::is_sha256_hex(x)).cloned().ok_or_else(|| no(codes::BAD_REQUEST, 400, "sha256 (lowercase hex) is required"))?;
    let kind = p.get("kind").cloned().unwrap_or_default();
    if kind != "bundle" && kind != "dirty" {
        return Err(no(codes::BAD_REQUEST, 400, "kind is bundle or dirty"));
    }
    let remote_ws = p.get("remoteWs").cloned().filter(|w| !w.is_empty());
    if remote_ws.as_deref().is_some_and(|w| !hub::valid_remote_id(w)) {
        return Err(no(codes::BAD_REQUEST, 400, "remoteWs is 1–128 of A-Z a-z 0-9 - _ ."));
    }
    let commit = p.get("commit").cloned().filter(|c| !c.is_empty());
    if kind == "bundle" && (remote_ws.is_none() || !commit.as_deref().is_some_and(is_hex40)) {
        return Err(no(codes::BAD_REQUEST, 400, "a bundle names its workspace (remoteWs) and its full commit"));
    }
    Ok(BlobAsk { sha256, kind, remote_ws, remote: p.get("remote").cloned().filter(|r| !r.is_empty()), commit })
}

/// `Content-Length` of a request head, if any.
fn content_length(head: &[u8]) -> Option<u64> {
    std::str::from_utf8(head).ok()?.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse().ok()))
}

/// `POST /cli/compute/hub/blob?sha256=&kind=bundle|dirty[&remoteWs=&remote=&commit=]`:
/// a tenant uploads a bundle or a `--dirty` blob. Signed over the declared
/// sha256 (checked BEFORE a byte of the body is read), capped per door
/// (relay 50 MiB, LAN 1 GiB) and per tenant (its blob store quota), then
/// streamed to disk in pieces, hashed as it goes and refused on any
/// mismatch. A bundle is fetched into the tenant's mirror for that
/// workspace and remote (the hub's key, V28) and its file dropped. The
/// connection is closed after the answer (a refused body is never read).
pub async fn serve_blob(
    stream: &mut tokio::net::TcpStream,
    buf: &mut [u8],
    raw_query: &str,
    header: Option<String>,
    ingress: crate::routes::dispatcher::Ingress,
) -> CliResponse {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Read the head off the socket (it was only peeked).
    let mut head: Vec<u8> = Vec::new();
    let body_start = loop {
        let n = match stream.read(buf).await {
            Ok(0) | Err(_) => return blob_refusal(codes::BAD_REQUEST, 400, "the request ended early"),
            Ok(n) => n,
        };
        head.extend_from_slice(&buf[..n]);
        if let Some(pos) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if head.len() > 64 * 1024 {
            return blob_refusal(codes::BAD_REQUEST, 400, "request head too large");
        }
    };
    let Some(declared) = content_length(&head[..body_start]) else {
        return blob_refusal(codes::BAD_REQUEST, 400, "a blob upload carries Content-Length");
    };
    let ask = match blob_ask(raw_query) {
        Ok(a) => a,
        Err(e) => return refusal_response(&e),
    };
    // Who is it? The signature covers the declared hash (the body hash).
    let (q, h, sha) = (raw_query.to_string(), header.clone(), ask.sha256.clone());
    let tenant = match tokio::task::spawn_blocking(move || {
        if !compute::enabled() {
            return Err(no("compute_disabled", 404, "compute is off"));
        }
        authenticate(BLOB_PATH, "POST", &q, h.as_deref(), &sha)
    })
    .await
    {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return refusal_response(&e),
        Err(e) => return CliResponse::internal_error(format!("worker join: {e}")),
    };
    let cap = blob_cap_for(ingress);
    if declared > cap {
        return blob_refusal(
            hcodes::BLOB_TOO_LARGE,
            413,
            format!("{} MB is over this door's {} MB per blob (the relay caps uploads; use a LAN route to the hub)", declared >> 20, cap >> 20),
        );
    }
    let (used, quota) = {
        let tid = tenant.id.clone();
        let quota = tenant.blob_cap_bytes();
        let used = tokio::task::spawn_blocking(move || {
            let db = k2_core::db::shared();
            let conn = db.lock();
            hub::blob_bytes(&conn, &tid)
        })
        .await
        .unwrap_or(u64::MAX);
        (used, quota)
    };
    if used.saturating_add(declared) > quota {
        return blob_refusal(
            hcodes::BLOB_QUOTA,
            409,
            format!("your server's blob store on this hub holds {} MB of {} MB; old blobs expire after {} days", used >> 20, quota >> 20, hub::ARTIFACT_KEEP_DAYS),
        );
    }
    // Stream the body to a part file, hashing as it goes.
    let dir = hub::tenant_dir(&tenant.id).join("blobs");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return CliResponse::internal_error(format!("blob dir: {e}"));
    }
    let part = dir.join(format!(".{}.part", crypto::hex(&crypto::random_bytes(8).unwrap_or_default())));
    let mut file = match tokio::fs::File::create(&part).await {
        Ok(f) => f,
        Err(e) => return CliResponse::internal_error(format!("blob file: {e}")),
    };
    let mut hasher = crypto::Sha256Stream::default();
    let mut got: u64 = 0;
    let first = &head[body_start..];
    let take = (first.len() as u64).min(declared) as usize;
    let mut failed: Option<CliResponse> = None;
    if take > 0 {
        hasher.update(&first[..take]);
        got += take as u64;
        if file.write_all(&first[..take]).await.is_err() {
            failed = Some(CliResponse::internal_error("blob write"));
        }
    }
    while failed.is_none() && got < declared {
        let want = ((declared - got) as usize).min(buf.len());
        match stream.read(&mut buf[..want]).await {
            Ok(0) | Err(_) => {
                failed = Some(blob_refusal(codes::BAD_REQUEST, 400, format!("the upload ended after {got} of {declared} bytes")));
            }
            Ok(n) => {
                hasher.update(&buf[..n]);
                got += n as u64;
                if file.write_all(&buf[..n]).await.is_err() {
                    failed = Some(CliResponse::internal_error("blob write"));
                }
            }
        }
    }
    if failed.is_none() && file.flush().await.is_err() {
        failed = Some(CliResponse::internal_error("blob flush"));
    }
    drop(file);
    if let Some(r) = failed {
        let _ = tokio::fs::remove_file(&part).await;
        return r;
    }
    let actual = hasher.finish_hex();
    if !crypto::ct_eq(actual.as_bytes(), ask.sha256.as_bytes()) {
        let _ = tokio::fs::remove_file(&part).await;
        return blob_refusal(hcodes::BLOB_MISMATCH, 400, "the bytes don't match the sha256 the upload named");
    }
    let r = tokio::task::spawn_blocking(move || store_blob(&tenant, &ask, &part, got)).await;
    match r {
        Ok(Ok(v)) => CliResponse::ok_json(v.to_string()),
        Ok(Err(e)) => refusal_response(&e),
        Err(e) => CliResponse::internal_error(format!("worker join: {e}")),
    }
}

/// Keep a verified upload: a `--dirty` blob in the tenant's store, a
/// bundle fetched into the tenant's mirror (then dropped).
fn store_blob(t: &TenantRow, ask: &BlobAsk, part: &Path, bytes: u64) -> R<serde_json::Value> {
    let now = compute::now();
    if ask.kind == "dirty" {
        let dst = hub::blob_path(&t.id, &ask.sha256);
        std::fs::rename(part, &dst).map_err(|e| no("internal", 500, format!("blob store: {e}")))?;
        let db = k2_core::db::shared();
        let conn = db.lock();
        hub::record_blob(&conn, &t.id, &ask.sha256, "dirty", bytes, now).map_err(|e| no("internal", 500, e))?;
        return Ok(serde_json::json!({"ok": true, "kind": "dirty", "sha256": ask.sha256, "bytes": bytes}));
    }
    let rws = ask.remote_ws.as_deref().unwrap_or_default();
    let commit = ask.commit.as_deref().unwrap_or_default();
    let remote = ask.remote.as_deref().and_then(compute::sync::fetchable_remote);
    let pk = hub::hub_project_key(&t.id, rws, remote.as_deref());
    let r = import_bundle(&t.id, &pk, part, commit);
    let _ = std::fs::remove_file(part);
    r?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::record_event(&conn, &format!("tenant:{}", t.label), "tenant_code", None, None, None, serde_json::json!({"tenant": t.id, "commit": commit, "bytes": bytes}));
    }
    Ok(serde_json::json!({"ok": true, "kind": "bundle", "sha256": ask.sha256, "bytes": bytes, "commit": commit, "imported": true}))
}

/// Fetch a bundle into the tenant's bare mirror (made on first use) and
/// keep `commit` reachable. Objects are fsck'd on the way in; a bundle
/// whose prerequisites the mirror lacks is refused (`bundle_prereq_missing`,
/// the submitter sends a full one).
pub(crate) fn import_bundle(tenant_id: &str, project_key: &str, bundle: &Path, commit: &str) -> R<()> {
    let m = hub::tenant_mirror(tenant_id, project_key);
    if !m.join("objects").is_dir() {
        std::fs::create_dir_all(&m).map_err(|e| no("internal", 500, format!("mirror dir: {e}")))?;
        let out = std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&m)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| no("internal", 500, format!("git init: {e}")))?;
        if !out.status.success() {
            return Err(no("internal", 500, format!("git init: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
    }
    let mut c = git_in(&m);
    c.args(["-c", "transfer.fsckObjects=true", "fetch", "-q", "--no-tags", "--no-write-fetch-head"])
        .arg(bundle)
        .arg("+refs/k2-compute/*:refs/k2-in/*");
    let out = c.output().map_err(|e| no("internal", 500, format!("git fetch: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>();
        if err.contains("prerequisite") || err.contains("lacks") {
            return Err(no("bundle_prereq_missing", 409, format!("the hub's mirror lacks this bundle's base commits; send a full bundle ({err})")));
        }
        return Err(no("bundle_invalid", 400, format!("the bundle didn't import: {err}")));
    }
    let keep = |args: &[&str]| -> bool {
        let mut c = git_in(&m);
        c.args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        c.status().is_ok_and(|s| s.success())
    };
    let ok = mirror_has(tenant_id, project_key, commit) && keep(&["update-ref", &format!("refs/k2-keep/{commit}"), commit]);
    // The transient refs go either way.
    let mut list = git_in(&m);
    list.args(["for-each-ref", "--format=%(refname)", "refs/k2-in/"]);
    if let Ok(o) = list.output() {
        for r in String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.starts_with("refs/k2-in/")) {
            keep(&["update-ref", "-d", r]);
        }
    }
    if !ok {
        return Err(no("bundle_invalid", 400, format!("the bundle doesn't carry commit {}", &commit[..commit.len().min(8)])));
    }
    Ok(())
}

// ── retention ────────────────────────────────────────────────────────

/// Daily: tenants' fetched artifacts after 7 days, unused blobs after 7
/// days (logs are `logs::sweep`'s, 14 days). Best effort.
pub fn sweep_tenants() {
    let root = compute::compute_dir().join("tenants");
    let Ok(rd) = std::fs::read_dir(&root) else { return };
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(hub::ARTIFACT_KEEP_DAYS as u64 * 86_400);
    for t in rd.flatten() {
        let Ok(arts) = std::fs::read_dir(t.path().join("artifacts")) else { continue };
        for a in arts.flatten() {
            if a.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < cutoff) {
                let _ = std::fs::remove_dir_all(a.path());
            }
        }
    }
    let gone = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        hub::expire_blobs(&conn, compute::now() - hub::ARTIFACT_KEEP_DAYS * 86_400)
    };
    for (t, sha) in gone {
        let _ = std::fs::remove_file(hub::blob_path(&t, &sha));
    }
}
