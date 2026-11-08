//! Compute package requests on the controller (0.46.9,
//! prd-compute-self-serve-environments-v1 §6, §12.2; migration 0271).
//!
//! - `GET  /cli/compute/nodes/packages?node=` — the node's host line,
//!   manager, this server's policy there, approved and installed names, and
//!   recent requests (`k2 compute node packages <node>`).
//! - `POST /cli/compute/nodes/packages/request {nodes, names, why, job?}` —
//!   file one batched request (any member with a grant on the nodes; the
//!   answer carries `pkgHeld` and the batch id for the `compute.packages`
//!   card). For a node this server owns it is decided here; for a node
//!   another server owns it goes through the node to that server.
//! - `GET  /cli/compute/nodes/packages/requests` — your requests (an agent),
//!   all of them (the owner or an Admin).
//! - `POST /cli/compute/nodes/packages/decide {request, approve, add?, note?}`
//!   — the owner or an Admin, a person (never a passport). An approval sends
//!   the signed `packages.allow {packages, install, add, request}` to the
//!   node, where root's guards run (dry run, deny list, origins, size cap).
//! - `POST /cli/compute/nodes/packages/policy {node, server, level?,
//!   profiles?, maxInstallMb?}` — narrowing applies at once; a widening is
//!   held for the owner (a node admin request, Settings → Compute →
//!   Approvals).
//!
//! An older node (no `pkg-requests`): the owner's approval falls back to
//! CE-24's `packages.allow {package}` per name (the job installs at its
//! next run); another server can't ask through it ("It can't be
//! requested.").

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::compute::pkgreq::{self as pr, PkgRow};
use k2_core::compute::proto::frames::Frame;
use k2_core::compute::proto::pkgpolicy::{self, codes as pcodes, state, PkgRequest, PkgRequestDecide, PkgRequestDone, PkgRequestFwd};
use k2_core::compute::store::{self, NodeRow};
use k2_core::compute::{self as compute};
use serde_json::{json, Value};

use crate::compute_routes::{need_credential, no, Who, R};

pub const STATUS: &str = "/cli/compute/nodes/packages";
pub const REQUEST: &str = "/cli/compute/nodes/packages/request";
pub const REQUESTS: &str = "/cli/compute/nodes/packages/requests";
pub const DECIDE: &str = "/cli/compute/nodes/packages/decide";
pub const POLICY: &str = "/cli/compute/nodes/packages/policy";

pub const ROUTES: &[&str] = &[STATUS, DECIDE, POLICY, REQUEST, REQUESTS];
pub const POST_ROUTES: &[&str] = &[DECIDE, POLICY, REQUEST];

/// How long filing waits for an online node's dry run, so the card shows
/// the full install set and size.
const DRY_WAIT: Duration = Duration::from_secs(8);

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    if POST_ROUTES.contains(&path) {
        // Mutating rows are POST-only; the dispatcher's `require_post`
        // answers 405 first, this keeps the module honest on its own.
        if body.is_empty() && path != DECIDE {
            return Err(no(pcodes::PACKAGE_NOT_APPROVED, 400, "send a JSON body"));
        }
    }
    match path {
        STATUS => status(who, params),
        REQUEST => request(who, body),
        REQUESTS => requests(who, params),
        DECIDE => decide(who, body),
        POLICY => policy(who, body),
        _ => Err(no("not_found", 404, "no such compute route")),
    }
}

fn node_offer(n: &NodeRow) -> Option<k2_core::compute::proto::frames::Offer> {
    let (_, _, live) = crate::compute_ws::live_view(&n.id);
    live.or_else(|| n.last_offer.clone())
}

fn has_feature(n: &NodeRow, f: &str) -> bool {
    node_offer(n).is_some_and(|o| o.features.iter().any(|x| x == f))
}

fn node_name(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::node_by_id(&conn, id).map(|n| n.name).unwrap_or_else(|| id.to_string())
}

fn is_mac(n: &NodeRow) -> bool {
    matches!(n.os.as_deref(), Some("macos") | Some("darwin"))
}

/// A person who decides: the owner token or an Admin login, never a
/// passport (an agent never approves its own ask).
fn decider(who: &Who) -> R<()> {
    need_credential(who)?;
    if who.passport().is_none() && who.is_admin() {
        return Ok(());
    }
    Err(no(
        "owner_only",
        403,
        "only this server's owner or an Admin decides package requests, in person: the card in chat, Settings → Compute → Approvals, \
         or k2 compute node packages approve|deny <request> from a terminal outside K2. An agent can never approve.",
    ))
}

fn row_json(r: &PkgRow) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    v["node"] = json!(node_name(&r.node_id));
    v
}

// ── status ───────────────────────────────────────────────────────────

fn status(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    need_credential(who)?;
    let ws = who.passport().map(|(w, _)| w.to_string());
    let node = crate::compute_routes::resolve_node_for(who, params.get("node").map(String::as_str), ws.as_deref())?;
    let offer = node_offer(&node);
    let host = offer.as_ref().and_then(|o| o.host.clone());
    let pkg = offer.as_ref().and_then(|o| o.pkg.clone());
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        pr::list(&conn, &pr::Filter { node_id: Some(&node.id), workspace_id: ws.as_deref(), limit: 20, ..Default::default() })
    };
    Ok(json!({
        "node": node.name,
        "host": host.as_ref().map(host_json),
        "hostLine": host.as_ref().map(|h| h.line()),
        "packages": pkg.as_ref().map(|p| json!({
            "manager": p.manager,
            "level": p.level.as_str(),
            "profiles": p.profiles,
            "maxInstallMb": p.max_install_mb,
            "policy": p.policy().summary(),
            "approved": p.approved,
            "installed": p.installed,
            "indexAgeSecs": p.index_age_secs,
            "openRequests": p.open_requests,
        })),
        "takesRequests": has_feature(&node, pkgpolicy::FEATURE_PKG_REQUESTS),
        "owner": if node.is_tenant() { node.owner_label.clone().unwrap_or_else(|| "its owner".into()) } else { "this server".to_string() },
        "requests": rows.iter().map(row_json).collect::<Vec<_>>(),
        "note": if pkg.is_none() {
            Some(if is_mac(&node) { "a Mac: no system packages (its owner installs with brew at the machine)".to_string() } else { "this node's k2-node reports no package status (older than 0.46.9, or no package installer)".to_string() })
        } else { None },
    }))
}

/// The §4.3 `host` object, camelCase. GPU jobs (0.46.11): `gpus` (each
/// GPU's facts, `jobs: true` where this server may run GPU jobs) only when
/// the node reports any, so every other node's JSON is unchanged.
pub fn host_json(h: &k2_core::compute::proto::host::HostFacts) -> Value {
    let mut v = json!({
        "os": h.os, "arch": h.arch, "distro": h.distro, "distroLike": h.distro_like, "distroVersion": h.distro_version,
        "distroName": h.distro_name, "distroFlavor": h.distro_flavor, "pkgManager": h.pkg, "kernel": h.kernel, "libc": h.libc,
        "gpu": h.gpu, "rebootRequired": h.reboot_required, "short": h.short(), "line": h.line(),
    });
    if !h.gpus.is_empty() {
        v["gpus"] = h
            .gpus
            .iter()
            .map(|g| {
                json!({
                    "vendor": g.vendor, "pci": g.pci, "driver": g.driver, "ids": g.ids, "model": g.model, "render": g.render,
                    "vramBytes": g.vram_bytes, "gttBytes": g.gtt_bytes, "unified": g.unified, "jobs": g.jobs, "line": g.line(),
                })
            })
            .collect::<Vec<_>>()
            .into();
        v["gpuJobs"] = h.gpus.iter().any(|g| g.jobs).into();
    }
    v
}

// ── filing ───────────────────────────────────────────────────────────

fn request(who: &Who, body: &[u8]) -> R<Value> {
    need_credential(who)?;
    crate::compute_routes::passport_gate(who)?;
    let v = crate::compute_routes::json_body(body)?;
    let ws = crate::compute_routes::target_ws(who, crate::compute_routes::s(&v, "workspace"))?;
    let mut tokens: Vec<String> = v.get("nodes").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if let Some(n) = crate::compute_routes::s(&v, "node") {
        tokens.push(n.to_string());
    }
    if tokens.is_empty() {
        return Err(no(pcodes::PACKAGE_NOT_APPROVED, 400, "name the node(s): k2 compute node packages request <node>[,<node>…] <name>[,<name>…] --why \"…\""));
    }
    let mut nodes = Vec::new();
    for t in &tokens {
        let n = crate::compute_routes::resolve_node_for(who, Some(t), Some(&ws.id))?;
        if !nodes.iter().any(|x: &NodeRow| x.id == n.id) {
            nodes.push(n);
        }
    }
    let names: Vec<String> = v.get("names").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let why = crate::compute_routes::s(&v, "why").map(str::trim).filter(|s| !s.is_empty());
    let job = crate::compute_routes::s(&v, "job");
    let session = who.passport().map(|(_, s)| s.to_string());
    let asked_by = ws.handle.clone();
    file(who, &ws, session.as_deref(), &asked_by, &nodes, &names, why, job)
}

/// File one batched request for `names` on `nodes` (the request verb, and
/// a submit that hit an uncovered package). Returns the response body.
#[allow(clippy::too_many_arguments)]
pub(crate) fn file(who: &Who, ws: &crate::compute_routes::Ws, session: Option<&str>, asked_by: &str, nodes: &[NodeRow], names: &[String], why: Option<&str>, job: Option<&str>) -> R<Value> {
    pkgpolicy::check_request(names, why).map_err(|e| no(pcodes::PACKAGE_NOT_APPROVED, 400, e))?;
    let canon = pkgpolicy::expand_names(names).map_err(|e| no(k2_core::compute::proto::profiles::PROFILE_UNKNOWN, 400, e))?;
    if let Some((n, d)) = canon.iter().find_map(|n| pkgpolicy::denied(n).map(|d| (n.clone(), d))) {
        return Err(no(pcodes::PACKAGE_DENIED, 409, format!("{n} is on K2's deny list ({d}): it changes how a machine boots, logs in or starts services. It can't be requested; a node's owner installs it by hand if they want it")));
    }
    let batch = pr::new_id();
    let now = compute::now();
    let mut filed: Vec<PkgRow> = Vec::new();
    let mut cant: Vec<String> = Vec::new();
    for n in nodes {
        if is_mac(n) {
            cant.push(format!("{}: a Mac installs no system packages; its owner runs brew install {} at the machine ({})", n.name, canon.join(" "), pcodes::PACKAGES_LINUX_ONLY));
            continue;
        }
        let takes = has_feature(n, pkgpolicy::FEATURE_PKG_REQUESTS);
        if n.is_tenant() && !takes {
            cant.push(format!(
                "{}: this node's k2-node is too old to take requests from other servers; its owner ({}) approves at the machine: sudo k2-node packages allow {}",
                n.name,
                n.owner_label.as_deref().unwrap_or("its owner"),
                canon.join(" ")
            ));
            continue;
        }
        if !n.is_tenant() {
            let off = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::compute::admin::admin_cols(&conn, &n.id).remote_admin.as_deref() == Some("off")
            };
            if off {
                cant.push(format!("{}: remote admin is off there, so K2 can't install for you; at the machine: sudo k2-node packages allow {} (or turn on remote admin: k2 compute local remote-admin policy)", n.name, canon.join(" ")));
                continue;
            }
        }
        let open = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            pr::open_count(&conn, &n.id, Some(&ws.id))
        };
        if open >= pkgpolicy::MAX_OPEN_PER_SERVER as i64 {
            return Err(no(pcodes::TOO_MANY_PENDING, 429, format!("{} package requests from {} already wait on {}; wait for the owner's answer (k2 compute node packages requests)", open, ws.handle, n.name)));
        }
        let origin = if n.is_tenant() { pr::SENT } else { pr::OWN };
        let server = if n.is_tenant() { n.owner_label.clone().unwrap_or_else(|| "its owner".into()) } else { "own".into() };
        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            pr::insert(
                &conn,
                &pr::NewRow {
                    batch_id: &batch,
                    origin,
                    node_id: &n.id,
                    server: &server,
                    workspace_id: Some(&ws.id),
                    session_id: session,
                    job_id: job,
                    asked_by,
                    why,
                    names: &canon,
                    host: node_offer(n).and_then(|o| o.host).map(|h| h.line()).as_deref(),
                    now,
                    ..Default::default()
                },
            )
            .map_err(|e| no("internal", 500, e))?
        };
        if takes {
            let f = Frame::PkgRequest(PkgRequest { id: row.id.clone(), names: canon.clone(), job_id: job.map(str::to_string), why: why.map(str::to_string), asked_by: asked_by.to_string() });
            if !crate::compute_ws::send_frame(&n.id, f) && n.is_tenant() {
                let db = k2_core::db::shared();
                let conn = db.lock();
                pr::set_state(&conn, &row.id, state::FAILED, None, Some("the node was offline; ask again when it's back"), None, now);
                cant.push(format!("{}: offline; ask again when it's back", n.name));
                continue;
            }
        }
        crate::compute_routes::audit(who, "pkg_requested", Some(&n.id), Some(&ws.id), job, json!({"request": row.id, "batch": batch, "names": canon, "origin": origin, "why": why}));
        filed.push(row);
    }
    if filed.is_empty() {
        return Err(no(pcodes::REQUESTS_UNSUPPORTED, 409, format!("It can't be requested. {}", cant.join("; "))).with("cant", cant));
    }
    // An online own node answers with its dry run within a moment: wait for
    // it, so the card shows every package and the size.
    let deadline = Instant::now() + DRY_WAIT;
    loop {
        let fresh: Vec<PkgRow> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            pr::batch(&conn, &batch)
        };
        let waiting = fresh.iter().any(|r| r.origin == pr::OWN && r.dry.is_none() && crate::compute_ws::is_online(&r.node_id) && has_feature_id(&r.node_id, pkgpolicy::FEATURE_PKG_REQUESTS));
        filed = fresh;
        if !waiting || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "pkg_request", "id": batch}));
    let own = filed.iter().any(|r| r.origin == pr::OWN);
    let summary = pr::summary(&filed, &node_name);
    let sent: Vec<String> = filed.iter().filter(|r| r.origin == pr::SENT).map(|r| format!("{} ({})", node_name(&r.node_id), r.server)).collect();
    let mut hint = Vec::new();
    if own {
        hint.push(format!("held for this server's owner or an Admin: they approve it on the card, or in Settings → Compute → Approvals (k2 compute node packages approve {batch})"));
    }
    if !sent.is_empty() {
        hint.push(format!("asked the node's owner for {}: you hear back here when they decide", sent.join(", ")));
    }
    Ok(json!({
        "pkgHeld": own,
        "request": { "id": batch, "summary": summary, "rows": filed.iter().map(row_json).collect::<Vec<_>>() },
        "cant": cant,
        "hint": hint.join("; "),
    }))
}

fn has_feature_id(node_id: &str, f: &str) -> bool {
    crate::compute_ws::live_view(node_id).2.is_some_and(|o| o.features.iter().any(|x| x == f))
}

fn requests(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    need_credential(who)?;
    let ws = who.passport().map(|(w, _)| w.to_string());
    if ws.is_none() && !who.is_admin() {
        return Err(no("owner_only", 403, "an Admin or the owner sees every request; an agent sees its own"));
    }
    let node = match params.get("node").map(String::as_str).filter(|x| !x.is_empty()) {
        Some(t) => Some(crate::compute_routes::resolve_node_for(who, Some(t), ws.as_deref())?),
        None => None,
    };
    let st = params.get("state").map(String::as_str).filter(|x| !x.is_empty() && *x != "all");
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        expire_pass_on(&conn);
        pr::list(&conn, &pr::Filter { node_id: node.as_ref().map(|n| n.id.as_str()), workspace_id: ws.as_deref(), state: st, decidable_only: false, limit: params.get("limit").and_then(|x| x.parse().ok()).unwrap_or(50) })
    };
    Ok(json!({ "requests": rows.iter().map(row_json).collect::<Vec<_>>(), "canDecide": ws.is_none() && who.is_admin() }))
}

// ── deciding ─────────────────────────────────────────────────────────

/// `packages.allow` commands in flight: cmd id → row id.
static IN_FLIGHT: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn in_flight<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut g = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

fn decide(who: &Who, body: &[u8]) -> R<Value> {
    decider(who)?;
    let v = crate::compute_routes::json_body(body)?;
    let id = crate::compute_routes::s(&v, "request").ok_or_else(|| no(pcodes::PACKAGE_NOT_APPROVED, 400, "name the request (cpr_…)"))?;
    let approve = v.get("approve").and_then(Value::as_bool).ok_or_else(|| no(pcodes::PACKAGE_NOT_APPROVED, 400, "approve: true or false"))?;
    let add = v.get("add").and_then(Value::as_bool).unwrap_or(false);
    let note = crate::compute_routes::s(&v, "note").map(|n| n.chars().filter(|c| !c.is_control()).take(400).collect::<String>());
    decide_batch(who, id, approve, add, note.as_deref())
}

/// Decide every pending row of a batch (the card's apply and deny call
/// this too).
pub(crate) fn decide_batch(who: &Who, id: &str, approve: bool, add: bool, note: Option<&str>) -> R<Value> {
    decider(who)?;
    let rows: Vec<PkgRow> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        expire_pass_on(&conn);
        pr::batch(&conn, id)
    };
    if rows.is_empty() {
        return Err(no("request_not_found", 404, format!("no package request {id} (k2 compute node packages requests)")));
    }
    let pending: Vec<&PkgRow> = rows.iter().filter(|r| r.state == state::PENDING && r.origin != pr::SENT).collect();
    if pending.is_empty() {
        let states: Vec<String> = rows.iter().map(|r| format!("{} on {}", r.state, node_name(&r.node_id))).collect();
        return Err(no("not_pending", 409, format!("request {id} was already decided ({})", states.join(", "))));
    }
    let now = compute::now();
    let mut out = Vec::new();
    for r in pending {
        let node = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::node_by_id(&conn, &r.node_id)
        };
        let Some(node) = node else { continue };
        if !approve {
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                pr::set_state(&conn, &r.id, state::DENIED, Some(&who.label()), note, None, now);
            }
            if let Some(nr) = &r.node_request {
                crate::compute_ws::send_frame(&node.id, Frame::PkgRequestDecide(PkgRequestDecide { id: nr.clone(), note: note.map(str::to_string) }));
            }
            crate::compute_routes::audit(who, "pkg_denied", Some(&node.id), r.workspace_id.as_deref(), r.job_id.as_deref(), json!({"request": r.id, "names": r.names, "note": note}));
            if r.origin == pr::OWN {
                tell(r, &format!("{}'s owner denied installing {}{}", node.name, r.names.join(" "), note.map(|n| format!(": {n}")).unwrap_or_default()));
            }
            out.push(json!({"request": r.id, "node": node.name, "state": state::DENIED}));
            continue;
        }
        if !crate::compute_ws::is_online(&node.id) {
            out.push(json!({"request": r.id, "node": node.name, "state": state::PENDING, "note": "the node is offline; decide again when it's back"}));
            continue;
        }
        if has_feature(&node, pkgpolicy::FEATURE_PKG_REQUESTS) {
            let args = json!({"packages": r.names, "install": true, "add": add, "request": r.node_request.clone().unwrap_or_else(|| r.id.clone())});
            let cmd = crate::compute_admin::send_operate_now(who, &node, k2_core::compute::proto::admin::VERB_PACKAGES_ALLOW, &args)?;
            in_flight(|m| m.insert(cmd.clone(), r.id.clone()));
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                pr::set_state(&conn, &r.id, "approved", Some(&who.label()), note, Some(add), now);
            }
            crate::compute_routes::audit(who, "pkg_approved", Some(&node.id), r.workspace_id.as_deref(), r.job_id.as_deref(), json!({"request": r.id, "names": r.names, "add": add, "cmdId": cmd}));
            out.push(json!({"request": r.id, "node": node.name, "state": "approved", "note": "on its way to the node: root installs it now, after its dry run and guards"}));
        } else {
            // An older node (CE-24): one name at a time onto its approved
            // list; the job's next run installs them.
            for name in &r.names {
                crate::compute_admin::send_operate_now(who, &node, k2_core::compute::proto::admin::VERB_PACKAGES_ALLOW, &json!({"package": name}))?;
            }
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                pr::set_state(&conn, &r.id, "approved", Some(&who.label()), Some("added to the node's approved list; its k2-node is older, so the job installs them at its next run"), Some(true), now);
            }
            crate::compute_routes::audit(who, "pkg_approved", Some(&node.id), r.workspace_id.as_deref(), r.job_id.as_deref(), json!({"request": r.id, "names": r.names, "legacy": true}));
            tell(r, &format!("{}'s owner approved {}: they're on its approved list now; run your job again and it installs them first", node.name, r.names.join(" ")));
            out.push(json!({"request": r.id, "node": node.name, "state": "approved", "note": "added to the approved list (an older k2-node): the job installs them at its next run"}));
        }
    }
    crate::compute_ws::notify_change();
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "pkg_request", "id": id}));
    Ok(json!({"request": id, "rows": out}))
}

/// Tell the agent that asked (own rows), in its session or inbox.
fn tell(r: &PkgRow, text: &str) {
    crate::compute_admin::deliver_note(r.workspace_id.as_deref(), r.session_id.as_deref(), "Compute packages", &format!("[compute] {text} (request {})", r.batch_id));
}

/// Root's answer to a `packages.allow` this module sent. True when it was
/// one of ours.
pub fn on_admin_result(node_id: &str, r: &k2_core::compute::proto::admin::AdminResult) -> bool {
    let Some(row_id) = in_flight(|m| m.remove(&r.cmd_id)) else { return false };
    let now = compute::now();
    let (row, node) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (pr::get(&conn, &row_id), store::node_by_id(&conn, node_id).map(|n| n.name).unwrap_or_default())
    };
    let Some(row) = row else { return true };
    let ok = r.state == "done";
    let note = if ok {
        r.result.get("dry").and_then(|d| d.get("installs")).and_then(Value::as_array).map(|a| format!("{} packages with dependencies", a.len()))
    } else {
        Some(format!("{}: {}", r.code.clone().unwrap_or_default(), r.message.clone().unwrap_or_default()))
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        pr::set_state(&conn, &row.id, if ok { state::INSTALLED } else { state::FAILED }, None, note.as_deref(), None, now);
        store::record_event(&conn, "node", if ok { "pkg_installed" } else { "pkg_failed" }, Some(node_id), row.workspace_id.as_deref(), row.job_id.as_deref(), json!({"request": row.id, "names": row.names, "note": note}));
    }
    if row.origin == pr::OWN {
        let text = if ok {
            format!("{node} installed {} (approved by its owner); run your job again", row.names.join(" "))
        } else {
            format!("{node} couldn't install {}: {}", row.names.join(" "), note.clone().unwrap_or_default())
        };
        tell(&row, &text);
    }
    true
}

// ── from the node ────────────────────────────────────────────────────

/// `pkg_request_fwd`: our own row's dry run, or another server's request
/// for a node we own.
pub fn on_fwd(node_id: &str, f: PkgRequestFwd) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if f.server == "own" {
        if let Some(r) = pr::get(&conn, &f.server_request).filter(|r| r.node_id == node_id && r.origin == pr::OWN) {
            pr::set_node_info(&conn, &r.id, &f.id, f.dry.as_ref(), Some(&f.host));
        }
        return;
    }
    if pr::by_node_request(&conn, node_id, &f.id).is_some() {
        return;
    }
    let names: Vec<String> = f.names.iter().filter(|n| k2_core::compute::proto::pkgmap::SysName::parse(n).is_some()).take(pkgpolicy::MAX_REQUEST_NAMES).cloned().collect();
    if names.is_empty() {
        return;
    }
    let server: String = f.server.chars().filter(|c| !c.is_control()).take(120).collect();
    let asked_by: String = f.asked_by.chars().filter(|c| !c.is_control()).take(80).collect();
    let why: Option<String> = f.why.as_ref().map(|w| w.chars().filter(|c| !c.is_control()).take(pkgpolicy::MAX_WHY).collect());
    let batch = pr::new_id();
    match pr::insert(
        &conn,
        &pr::NewRow {
            batch_id: &batch,
            origin: pr::FWD,
            node_id,
            node_request: Some(&f.id),
            server: &server,
            asked_by: &asked_by,
            why: why.as_deref(),
            job_id: f.job_id.as_deref(),
            names: &names,
            natives: serde_json::to_value(&f.natives).unwrap_or_default(),
            dry: f.dry.as_ref(),
            host: Some(&f.host),
            now: compute::now(),
            expires_at: Some(f.expires_at.min(compute::now() + pkgpolicy::REQUEST_TTL_SECS)),
            ..Default::default()
        },
    ) {
        Ok(r) => {
            store::record_event(&conn, "node", "pkg_requested", Some(node_id), None, None, json!({"request": r.id, "server": server, "names": names, "why": why}));
            drop(conn);
            k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "pkg_request", "id": batch}));
        }
        Err(e) => k2_core::log_debug!("[compute] package request from {server}: {e}"),
    }
}

/// `pkg_request_done`: the node's owner decided a request we sent.
pub fn on_done(node_id: &str, d: PkgRequestDone) {
    let now = compute::now();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(r) = pr::get(&conn, &d.server_request).filter(|r| r.node_id == node_id && r.origin == pr::SENT) else { return };
        let st = match d.state.as_str() {
            state::INSTALLED => state::INSTALLED,
            state::DENIED => state::DENIED,
            state::EXPIRED => state::EXPIRED,
            _ => state::FAILED,
        };
        let note: Option<String> = d.note.as_ref().map(|n| n.chars().filter(|c| !c.is_control()).take(400).collect());
        pr::set_state(&conn, &r.id, st, Some(&d.owner), note.as_deref(), None, now);
        store::record_event(&conn, "node", &format!("pkg_{st}"), Some(node_id), r.workspace_id.as_deref(), r.job_id.as_deref(), json!({"request": r.id, "owner": d.owner, "note": note}));
        r
    };
    let node = node_name(node_id);
    let owner: String = d.owner.chars().filter(|c| !c.is_control()).take(120).collect();
    let text = match d.state.as_str() {
        state::INSTALLED => format!("{node}'s owner ({owner}) approved and installed {}; run your job again", row.names.join(" ")),
        state::DENIED => format!("{node}'s owner ({owner}) denied installing {}{}", row.names.join(" "), d.note.as_ref().map(|n| format!(": {n}")).unwrap_or_default()),
        state::EXPIRED => format!("{node}'s owner ({owner}) didn't answer your request for {} within 24 h", row.names.join(" ")),
        _ => format!("{node} couldn't install {}: {}", row.names.join(" "), d.note.clone().unwrap_or_default()),
    };
    tell(&row, &text);
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "pkg_request", "id": row.batch_id}));
}

fn expire_pass_on(conn: &rusqlite::Connection) {
    for r in pr::expire(conn, compute::now()) {
        tell(&r, &format!("nobody answered your request for {} on {} within 24 h", r.names.join(" "), node_name_conn(conn, &r.node_id)));
    }
}

fn node_name_conn(conn: &rusqlite::Connection, id: &str) -> String {
    store::node_by_id(conn, id).map(|n| n.name).unwrap_or_else(|| id.to_string())
}

/// The watch: expire unanswered requests.
pub fn expire_pass() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    expire_pass_on(&conn);
}

// ── policy ───────────────────────────────────────────────────────────

fn policy(who: &Who, body: &[u8]) -> R<Value> {
    let v = crate::compute_routes::json_body(body)?;
    let node = crate::compute_routes::node_or_404(crate::compute_routes::s(&v, "node"))?;
    if node.is_tenant() {
        return Err(no("not_node_owner", 403, format!("{} belongs to another server ({}): its owner sets its package policy", node.name, node.owner_label.as_deref().unwrap_or("its owner"))));
    }
    let mut args = json!({"server": crate::compute_routes::s(&v, "server").unwrap_or("own")});
    if let Some(l) = crate::compute_routes::s(&v, "level") {
        args["level"] = json!(l);
    }
    if let Some(p) = v.get("profiles") {
        args["profiles"] = p.clone();
    }
    if let Some(m) = v.get("maxInstallMb") {
        args["max_install_mb"] = m.clone();
    }
    pkgpolicy::check_policy_args(&args).map_err(|e| no("bad_args", 400, e))?;
    if !has_feature(&node, pkgpolicy::FEATURE_PKG_STATUS) {
        return Err(no(pcodes::REQUESTS_UNSUPPORTED, 409, format!("{}'s k2-node has no package policy yet (older than 0.46.9): update it (k2 compute node update {})", node.name, node.name)));
    }
    // Narrowing goes at once; the executor refuses a widening without the
    // owner's approval (`approval_required`), and then it's held.
    let body = json!({"node": node.name, "kind": "command", "verb": k2_core::compute::proto::admin::VERB_PACKAGES_POLICY, "args": args, "reason": crate::compute_routes::s(&v, "reason").unwrap_or("package policy")});
    match crate::compute_admin::send_and_wait(who, &node, k2_core::compute::proto::admin::VERB_PACKAGES_POLICY, &args)? {
        Some(r) if r.state == "done" => Ok(json!({"node": node.name, "applied": true, "result": r.result, "note": "narrowing applied on the node"})),
        Some(r) if r.code.as_deref() == Some(k2_core::compute::proto::admin::code::APPROVAL_REQUIRED) => {
            crate::compute_admin::handle(crate::compute_admin::PROPOSE, who, body.to_string().as_bytes(), &HashMap::new(), None)
        }
        Some(r) => Err(no("node_refused", 409, format!("{}: {}", r.code.unwrap_or_default(), r.message.unwrap_or_default()))),
        None => Err(no("no_answer", 504, format!("{} didn't answer in time; check with k2 compute node packages {}", node.name, node.name))),
    }
}
