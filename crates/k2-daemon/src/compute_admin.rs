//! Node admin, controller side (`prd-k2-node-admin-v1` §7, §8, §10, §13).
//!
//! **Agents propose, owners approve, the node checks.**
//!
//! - `/cli/compute/admin/propose`: the owner, an Admin login, or an agent
//!   with the Agent switch "Allow this agent to operate compute nodes"
//!   (`agents_can_operate_compute`, Q14) files a held request. Always held.
//! - `/cli/compute/admin/approve`: **owner only** (Q2), and only with an
//!   unused vault confirm token for action `node-admin` bound to that
//!   request id (Touch ID / login password on the Mac, the vault password,
//!   or the Connect password on the web). The owner token alone is not
//!   enough (AR14: agents can read `daemon.token`); every passport is
//!   refused. On approval the controller seals one signed command per step
//!   (controller key + the owner approval key, a vault engine key that
//!   signs only here) and sends it over the node's live session.
//! - `deny` (owner/admin humans), `withdraw` (the proposer), `requests`
//!   (humans see all; an agent sees its own workspace's).
//! - `nodes/policy` show/set: a narrowing goes at once (operate class,
//!   controller signature only); a widening becomes a held request.
//! - `nodes/update` / `nodes/rollback`: a signed `k2-node` build (root
//!   helper key, §8.1) offered over the session; the node reports
//!   `update_state` (staged / swapped / healthy / rolled_back / refused).
//! - `nodes/health` / `nodes/alerts`: the latest health facts, offline and
//!   threshold alerts to the "alerts go to" workspace (daemon-owned, runs
//!   headless: [`spawn_watch`]).
//!
//! Everything is audited in `compute_events`.
//!
//! The wire is the node side's (`k2_node_proto::admin`, `::frames`
//! protocol 2, `::health`; `.k2/prds/node-admin-wire-v1.md`): this module
//! only uses its helpers (`sign_controller`, `owner_message`,
//! `owner_plan_message`, `with_owner_sig`, `AdminKeys::signed_part`,
//! `VERBS`, `code`).

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use k2_core::compute::admin::{self as admin, codes as acodes, RequestRow};
use k2_core::compute::proto::admin::{self as wire, Class, SignedCommand};
use k2_core::compute::proto::crypto;
use k2_core::compute::proto::frames::{self, Frame, Offer};
use k2_core::compute::proto::health::Health;
use k2_core::compute::{self as compute, codes, store};
use k2_core::log_debug;
use serde_json::{json, Value};

use crate::compute_routes::{json_body, no, node_or_404, owner_only, s, ws_by_token, Who, R};

pub const APPROVAL_KEY: &str = "/cli/compute/admin/approval-key";
pub const APPROVE: &str = "/cli/compute/admin/approve";
pub const DENY: &str = "/cli/compute/admin/deny";
pub const PROPOSE: &str = "/cli/compute/admin/propose";
pub const REQUESTS: &str = "/cli/compute/admin/requests";
pub const WITHDRAW: &str = "/cli/compute/admin/withdraw";
pub const NODE_ALERTS: &str = "/cli/compute/nodes/alerts";
pub const NODE_HEALTH: &str = "/cli/compute/nodes/health";
pub const NODE_POLICY: &str = "/cli/compute/nodes/policy";
pub const NODE_POLICY_SET: &str = "/cli/compute/nodes/policy/set";
pub const NODE_ROLLBACK: &str = "/cli/compute/nodes/rollback";
pub const NODE_UPDATE: &str = "/cli/compute/nodes/update";

/// Every node admin route (also listed in `compute_routes::ROUTES`).
pub const ROUTES: &[&str] = &[
    APPROVAL_KEY,
    APPROVE,
    DENY,
    PROPOSE,
    REQUESTS,
    WITHDRAW,
    NODE_ALERTS,
    NODE_HEALTH,
    NODE_POLICY,
    NODE_POLICY_SET,
    NODE_ROLLBACK,
    NODE_UPDATE,
];

/// POST-only (also in `compute_routes::POST_ROUTES`).
pub const POST_ROUTES: &[&str] = &[APPROVE, DENY, PROPOSE, WITHDRAW, NODE_ALERTS, NODE_POLICY_SET, NODE_ROLLBACK, NODE_UPDATE];

/// Passport-reachable (also in `session_token::is_agent_verb`). approve,
/// deny, alerts and approval-key are NOT here.
pub const AGENT_VERBS: &[&str] = &[PROPOSE, REQUESTS, WITHDRAW, NODE_HEALTH, NODE_POLICY, NODE_POLICY_SET, NODE_ROLLBACK, NODE_UPDATE];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

/// Route a node admin request (already past the preview flag).
pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    match path {
        PROPOSE => propose(who, body),
        REQUESTS => requests(who, params),
        APPROVE => approve(who, body),
        DENY => deny(who, body),
        WITHDRAW => withdraw(who, body),
        APPROVAL_KEY => approval_key(who),
        NODE_POLICY => policy_show(who, params),
        NODE_POLICY_SET => policy_set(who, body),
        NODE_HEALTH => health(who, params),
        NODE_ALERTS => alerts(who, body),
        NODE_UPDATE => update(who, body),
        NODE_ROLLBACK => rollback(who, body),
        _ => Err(no("not_found", 404, "no such compute admin route")),
    }
}

// ── seams: the vault, the controller key, the live session ───────────

fn vault() -> R<std::sync::Arc<k2_vault::LocalVault>> {
    #[cfg(test)]
    if let Some(v) = test_seams::vault() {
        return Ok(v);
    }
    crate::vault::get().map_err(|e| no(acodes::VAULT_CLOSED, 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))
}

fn controller_key() -> R<crypto::SigningKey> {
    #[cfg(test)]
    if let Some(k) = test_seams::controller_key() {
        return Ok(k);
    }
    compute::controller_key().map_err(|e| no("controller_key", 500, e))
}

fn assets_dir() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(d) = test_seams::assets_dir() {
        return d;
    }
    compute::compute_dir().join("node-assets")
}

fn online(node_id: &str) -> bool {
    #[cfg(test)]
    if let Some(on) = test_seams::online(node_id) {
        return on;
    }
    crate::compute_ws::is_online(node_id)
}

fn features(node_id: &str) -> Vec<String> {
    #[cfg(test)]
    if let Some(f) = test_seams::features(node_id) {
        return f;
    }
    crate::compute_ws::live_view(node_id).2.map(|o| o.features).unwrap_or_default()
}

fn send(node_id: &str, f: Frame) -> bool {
    #[cfg(test)]
    if let Some(sent) = test_seams::send(node_id, &f) {
        return sent;
    }
    crate::compute_ws::send_frame(node_id, f)
}

/// Serializes approval-key use: the first use mints the key, and two
/// racing mints would leave two versions (nodes pin one).
static APPROVAL_KEY_LOCK: Mutex<()> = Mutex::new(());

/// The owner approval public key (minted in the vault on first use).
pub(crate) fn approval_public_key() -> R<[u8; 32]> {
    let v = vault()?;
    let _g = APPROVAL_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    v.engine_public_key(admin::APPROVAL_ENGINE, admin::APPROVAL_KEY_NAME)
        .map_err(|e| no(acodes::VAULT_CLOSED, e.status, format!("approval key: {}", e.hint)))
}

/// Sign with the approval key. Only `approve` calls this, after it took
/// an owner confirm token for that request.
fn approval_sign(v: &k2_vault::LocalVault, msg: &[u8]) -> Result<[u8; 64], String> {
    let _g = APPROVAL_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    v.engine_sign(admin::APPROVAL_ENGINE, admin::APPROVAL_KEY_NAME, msg).map_err(|e| format!("{}: {}", e.code, e.hint))
}

// ── who ──────────────────────────────────────────────────────────────

/// Who proposes / operates, as stored on a request.
struct Operator {
    label: String,
    workspace: Option<String>,
    session: Option<String>,
}

fn operate_off() -> crate::compute_routes::Refusal {
    no(
        acodes::OPERATE_OFF,
        403,
        "this agent can't operate compute nodes: ask your owner to turn on \"Allow this agent to operate compute nodes\" \
         in Settings → Workspaces → <agent> → Agent (k2 compute operate on --workspace <ws>)",
    )
}

/// The owner, an Admin login, or an agent/K2-tab passport whose workspace
/// has the operate switch on.
fn operator(who: &Who) -> R<Operator> {
    crate::compute_routes::need_credential(who)?;
    if who.is_admin() {
        return Ok(Operator { label: who.label(), workspace: None, session: None });
    }
    if let Some((ws, sid)) = who.passport() {
        let on = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            admin::agents_can_operate_compute(&conn, ws)
        };
        if !on {
            return Err(operate_off());
        }
        return Ok(Operator { label: who.label(), workspace: Some(ws.to_string()), session: Some(sid.to_string()) });
    }
    Err(owner_only())
}

/// Read access to a node's admin view: humans with Admin; Member logins
/// get health only (`allow_member`); passports need the operate switch
/// or a grant on the node.
fn reader(who: &Who, node: &store::NodeRow, allow_member: bool) -> R<()> {
    crate::compute_routes::need_credential(who)?;
    if who.is_admin() || (allow_member && matches!(who, Who::Login { .. })) {
        return Ok(());
    }
    if let Some((ws, _)) = who.passport() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if admin::agents_can_operate_compute(&conn, ws)
            || (store::agents_can_use_compute(&conn, ws) && store::grant(&conn, &node.id, ws).is_some())
        {
            return Ok(());
        }
        return Err(no(codes::NODE_NOT_FOUND, 404, format!("no compute node '{}' you can see", node.name)));
    }
    Err(owner_only())
}

/// The vault actor stamp a confirm token was minted for.
fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, .. } => Some(format!("connect:{username}")),
        _ => None,
    }
}

fn node_admin_ready(node: &store::NodeRow, feature: &str) -> R<()> {
    if node.state != "active" {
        return Err(no(codes::NODE_NOT_FOUND, 404, format!("{} isn't an active node", node.name)));
    }
    if !online(&node.id) {
        return Err(no(codes::NODE_OFFLINE, 409, format!("{} is offline; try again when it's back (k2 compute node health {})", node.name, node.name)));
    }
    if !features(&node.id).iter().any(|f| f == feature) {
        return Err(no(
            acodes::NODE_LACKS_ADMIN,
            409,
            format!("{} runs a k2-node without '{feature}': update it at the machine once (k2-node setup --remote-admin …)", node.name),
        ));
    }
    Ok(())
}

// ── sealing and sending ──────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_command(
    node: &store::NodeRow,
    controller_fp: &str,
    class: Class,
    verb: &str,
    args: &Value,
    plan: Option<wire::PlanRef>,
    proposed_by: &str,
    approval: Option<(&str, &str, &str)>,
    now: i64,
) -> R<wire::Command> {
    Ok(wire::Command {
        v: wire::ADMIN_V,
        cmd_id: crypto::random_id().map_err(|e| no("random", 500, e))?,
        node_fp: node.fingerprint.clone(),
        controller_fp: controller_fp.to_string(),
        class,
        verb: verb.to_string(),
        args: args.clone(),
        plan,
        proposed_by: proposed_by.to_string(),
        approved_by: approval.map(|a| a.0.to_string()),
        approved_via: approval.map(|a| a.1.to_string()),
        request_id: approval.map(|a| a.2.to_string()),
        iat: now,
        exp: now + wire::MAX_LIFETIME_SECS,
        nonce: wire::new_nonce().map_err(|e| no("random", 500, e))?,
    })
}

/// Results the routes wait for (read and operate verbs answer within
/// seconds). `None` until the node answers.
static WAITERS: (Mutex<Option<HashMap<String, Option<wire::AdminResult>>>>, Condvar) = (Mutex::new(None), Condvar::new());

fn wait_register(cmd_id: &str) {
    WAITERS.0.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(cmd_id.to_string(), None);
}

/// Hand a result to a waiting route. True when someone was waiting.
fn wait_deliver(r: &wire::AdminResult) -> bool {
    let (m, cv) = &WAITERS;
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    let Some(slot) = g.as_mut().and_then(|w| w.get_mut(&r.cmd_id)) else { return false };
    *slot = Some(r.clone());
    cv.notify_all();
    true
}

fn wait_take(cmd_id: &str, timeout: Duration) -> Option<wire::AdminResult> {
    let (m, cv) = &WAITERS;
    let deadline = Instant::now() + timeout;
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        if let Some(Some(r)) = g.as_ref().and_then(|w| w.get(cmd_id)) {
            let r = r.clone();
            g.as_mut().map(|w| w.remove(cmd_id));
            return Some(r);
        }
        let now = Instant::now();
        if now >= deadline {
            g.as_mut().map(|w| w.remove(cmd_id));
            return None;
        }
        g = cv.wait_timeout(g, deadline - now).map(|(g, _)| g).unwrap_or_else(|p| p.into_inner().0);
    }
}

/// Seal and send one read/operate command now (no request row). With
/// `wait`, block up to that long for the node's answer.
fn send_direct(who: &Who, node: &store::NodeRow, class: Class, verb: &str, args: &Value, wait: Option<Duration>) -> R<(String, Option<wire::AdminResult>)> {
    let key = controller_key()?;
    let cmd = build_command(node, &key.fingerprint(), class, verb, args, None, &who.label(), None, compute::now())?;
    let sealed = wire::sign_controller(&key, &cmd).map_err(|e| no("seal_failed", 500, e))?;
    if wait.is_some() {
        wait_register(&cmd.cmd_id);
    }
    if !send(&node.id, Frame::Admin(frames::AdminFrame { cmd: sealed })) {
        if wait.is_some() {
            wait_take(&cmd.cmd_id, Duration::ZERO);
        }
        return Err(no(codes::NODE_OFFLINE, 409, format!("{} went offline", node.name)));
    }
    if class != Class::Read {
        crate::compute_routes::audit(who, "admin_sent", Some(&node.id), None, None, json!({"cmdId": cmd.cmd_id, "verb": verb, "class": class.as_str()}));
    }
    let result = wait.and_then(|t| wait_take(&cmd.cmd_id, t));
    Ok((cmd.cmd_id, result))
}

fn send_operate(who: &Who, node: &store::NodeRow, verb: &str, args: &Value) -> R<String> {
    send_direct(who, node, Class::Operate, verb, args, None).map(|(id, _)| id)
}

/// Send the next step of every approved request on `node_id` (node
/// online with the admin feature). One step in flight per request.
pub fn pump(node_id: &str) {
    if !online(node_id) || !features(node_id).iter().any(|f| f == wire::FEATURE_ADMIN) {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    for r in admin::sendable(&conn, node_id) {
        let step = r.next_step;
        let Some(cmd) = r.commands.get(step as usize).cloned() else { continue };
        if admin::command_exp(&cmd).is_some_and(|e| e + wire::CLOCK_SKEW_SECS < compute::now()) {
            let _ = admin::set_result(&conn, &r.id, "expired", &json!({"code": wire::code::EXPIRED, "step": step + 1}));
            store::record_event(&conn, "controller", "admin_expired", Some(node_id), r.proposer_workspace.as_deref(), None, json!({"request": r.id}));
            continue;
        }
        if !send(node_id, Frame::Admin(frames::AdminFrame { cmd })) {
            return;
        }
        admin::mark_sent(&conn, &r.id, step);
        store::record_event(
            &conn,
            "controller",
            "admin_sent",
            Some(node_id),
            r.proposer_workspace.as_deref(),
            None,
            json!({"request": r.id, "step": step + 1, "of": r.commands.len(), "cmdId": r.cmd_ids.get(step as usize)}),
        );
    }
}

/// Seal an approved request: one command per step, controller-signed;
/// owner-signed when privileged. A plan is approved once: the owner key
/// signs the canonical `Plan` (`owner_plan_message`) and every step
/// carries that signature with `plan_b64` (wire §2).
#[allow(clippy::too_many_arguments)]
fn seal_request(
    req: &RequestRow,
    node: &store::NodeRow,
    key: &crypto::SigningKey,
    approved_by: &str,
    via: &str,
    now: i64,
    owner_sign: &dyn Fn(&[u8]) -> Result<[u8; 64], String>,
) -> R<(Vec<SignedCommand>, Vec<String>)> {
    let steps = req.steps().map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
    let fp = key.fingerprint();
    let label = Class::parse_label(&req.class);
    let mut classes = Vec::new();
    for st in &steps {
        let c = admin::step_class(&st.verb).ok_or_else(|| no(acodes::VERB_NOT_ALLOWED, 400, format!("{} is no longer an admin verb", st.verb)))?;
        classes.push(c.max(label));
    }
    let privileged = classes.contains(&Class::Privileged);
    let seal_err = |e: String| no("seal_failed", 500, e);
    let mut sealed = Vec::new();
    let mut ids = Vec::new();
    if steps.len() == 1 {
        let cmd = build_command(node, &fp, classes[0], &steps[0].verb, &steps[0].args, None, &req.proposed_by, Some((approved_by, via, &req.id)), now)?;
        let mut sc = wire::sign_controller(key, &cmd).map_err(seal_err)?;
        if privileged {
            let body = cmd.canonical().map_err(seal_err)?;
            sc = sc.with_owner_sig(&owner_sign(&wire::owner_message(&body)).map_err(seal_err)?);
        }
        ids.push(cmd.cmd_id);
        sealed.push(sc);
        return Ok((sealed, ids));
    }
    let plan = wire::Plan {
        v: wire::ADMIN_V,
        plan_id: req.id.clone(),
        node_fp: node.fingerprint.clone(),
        controller_fp: fp.clone(),
        request_id: req.id.clone(),
        steps: steps.iter().map(|st| wire::PlanStep { verb: st.verb.clone(), args: st.args.clone() }).collect(),
        iat: now,
        exp: now + wire::MAX_LIFETIME_SECS,
    };
    let plan_bytes = plan.canonical().map_err(seal_err)?;
    let digest = format!("sha256:{}", crypto::sha256_hex(&plan_bytes));
    let plan_sig = if privileged { Some(owner_sign(&wire::owner_plan_message(&plan_bytes)).map_err(seal_err)?) } else { None };
    for (i, st) in steps.iter().enumerate() {
        let r = wire::PlanRef { plan_id: plan.plan_id.clone(), step: i as u32, of: steps.len() as u32, digest: digest.clone() };
        let cmd = build_command(node, &fp, classes[i], &st.verb, &st.args, Some(r), &req.proposed_by, Some((approved_by, via, &req.id)), now)?;
        let mut sc = wire::sign_controller(key, &cmd).map_err(seal_err)?;
        sc.plan_b64 = Some(crypto::b64(&plan_bytes));
        if let Some(sig) = &plan_sig {
            sc = sc.with_owner_sig(sig);
        }
        ids.push(cmd.cmd_id);
        sealed.push(sc);
    }
    Ok((sealed, ids))
}

trait ClassLabel {
    fn parse_label(s: &str) -> Class;
}

impl ClassLabel for Class {
    /// A stored request class; anything unknown is treated as privileged.
    fn parse_label(s: &str) -> Class {
        match s {
            "read" => Class::Read,
            "operate" => Class::Operate,
            _ => Class::Privileged,
        }
    }
}

// ── propose / list / decide ──────────────────────────────────────────

fn propose(who: &Who, body: &[u8]) -> R<Value> {
    let op = operator(who)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    if node.state != "active" {
        return Err(no(codes::NODE_NOT_FOUND, 404, format!("{} isn't an active node", node.name)));
    }
    let kind = s(&v, "kind").unwrap_or("command");
    let body_v = match kind {
        "plan" => json!({"steps": v.get("steps").cloned().unwrap_or(Value::Null)}),
        "command" | "service" => json!({"verb": s(&v, "verb").unwrap_or(""), "args": v.get("args").cloned().unwrap_or_else(|| json!({}))}),
        _ => return Err(no(acodes::BAD_ARGS, 400, "kind is command, plan or service")),
    };
    let reason = s(&v, "reason");
    let (class, summary) = classify_body(&node.name, kind, &body_v)?;
    file_request(&op, &node, kind, class, &body_v, &summary, reason)
}

/// Check every step; the request class is the highest step class.
fn classify_body(node_name: &str, kind: &str, body: &Value) -> R<(Class, String)> {
    let steps = admin::steps_of(kind, body).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
    let mut class = Class::Operate;
    let mut lines = Vec::new();
    for st in &steps {
        let c = admin::validate_step(&st.verb, &st.args).map_err(|(code, hint)| no(code, 400, hint))?;
        if kind == "service" && !st.verb.starts_with("svc.") {
            return Err(no(acodes::BAD_ARGS, 400, "a service request is svc.apply, svc.adopt or svc.remove"));
        }
        class = class.max(c);
        lines.push(admin::summarize(node_name, &st.verb, &st.args));
    }
    let summary = if lines.len() == 1 {
        lines.remove(0)
    } else {
        format!("plan of {} steps: {}", lines.len(), lines.iter().enumerate().map(|(i, l)| format!("{}) {l}", i + 1)).collect::<Vec<_>>().join("; "))
    };
    Ok((class, summary))
}

fn file_request(op: &Operator, node: &store::NodeRow, kind: &str, class: Class, body: &Value, summary: &str, reason: Option<&str>) -> R<Value> {
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = admin::insert_request(
            &conn,
            &admin::NewRequest {
                node_id: &node.id,
                kind,
                class,
                body,
                summary,
                reason,
                proposed_by: &op.label,
                proposer_workspace: op.workspace.as_deref(),
                proposer_session: op.session.as_deref(),
                now: compute::now(),
            },
        )
        .map_err(|(code, hint)| no(code, if code == acodes::TOO_MANY_PENDING { 429 } else { 400 }, hint))?;
        store::record_event(
            &conn,
            &op.label,
            "admin_proposed",
            Some(&node.id),
            op.workspace.as_deref(),
            None,
            json!({"request": r.id, "class": r.class, "summary": r.summary, "digest": r.digest}),
        );
        r
    };
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncSettings,
        json!({"compute": "admin_request", "id": r.id, "node": node.name, "summary": r.summary}),
    );
    Ok(json!({
        "held": true,
        "request": request_json(&r, Some(&node.name)),
        "hint": format!("held for the owner: they approve it in Settings → Compute → Approvals (or k2 compute node admin approve {})", r.id),
    }))
}

fn request_json(r: &RequestRow, node_name: Option<&str>) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    v["node"] = node_name.map(str::to_string).or_else(|| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &r.node_id).map(|n| n.name)
    })
    .into();
    if let Some(ws) = &r.proposer_workspace {
        v["proposerWorkspaceName"] = crate::compute_routes::ws_handle(ws).into();
    }
    v
}

fn requests(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    crate::compute_routes::need_credential(who)?;
    let own_ws = match who.passport() {
        Some((ws, _)) => Some(ws.to_string()),
        None if who.is_admin() => None,
        None => return Err(owner_only()),
    };
    let node = match params.get("node").map(String::as_str).filter(|x| !x.is_empty()) {
        Some(t) => Some(node_or_404(Some(t))?),
        None => None,
    };
    let state = params.get("state").map(String::as_str).filter(|x| !x.is_empty()).unwrap_or("open");
    let state = if state == "all" { None } else { Some(state) };
    let limit = params.get("limit").and_then(|x| x.parse().ok()).unwrap_or(50);
    let (rows, names) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        expire_and_notify(&conn);
        let rows = admin::list_requests(
            &conn,
            &admin::ReqFilter { node_id: node.as_ref().map(|n| n.id.as_str()), state, proposer_workspace: own_ws.as_deref(), limit },
        );
        let names: HashMap<String, String> = store::list_nodes(&conn, true).into_iter().map(|n| (n.id, n.name)).collect();
        (rows, names)
    };
    let list: Vec<Value> = rows.iter().map(|r| request_json(r, names.get(&r.node_id).map(String::as_str))).collect();
    Ok(json!({ "requests": list, "canApprove": who.is_owner() }))
}

fn request_or_404(token: Option<&str>) -> R<RequestRow> {
    let t = token.ok_or_else(|| no(acodes::BAD_ARGS, 400, "name a request (car_…)"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    admin::find_request(&conn, t).ok_or_else(|| no(acodes::REQUEST_NOT_FOUND, 404, format!("no admin request '{t}' (k2 compute node admin list)")))
}

fn decide_err(code: &'static str) -> crate::compute_routes::Refusal {
    match code {
        acodes::APPROVAL_EXPIRED => no(code, 410, "this request expired (24 h); ask the agent to propose it again"),
        acodes::REQUEST_NOT_FOUND => no(code, 404, "no such request"),
        _ => no(acodes::NOT_PENDING, 409, "this request was already decided"),
    }
}

fn approve(who: &Who, body: &[u8]) -> R<Value> {
    // Owner only (Q2). Passports, Admin logins and app passes: refused.
    if !who.is_owner() {
        crate::compute_routes::need_credential(who)?;
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "only the server's owner approves node admin requests, in person: Settings → Compute → Approvals, or \
             k2 compute node admin approve <id> from a terminal outside K2. An agent can never approve.",
        ));
    }
    let v = json_body(body)?;
    let req = request_or_404(s(&v, "request"))?;
    let Some(token) = s(&v, "confirmToken") else {
        return Err(no(
            acodes::VAULT_CONFIRM_REQUIRED,
            403,
            "approving needs a fresh owner confirm (Touch ID, your login or vault password): k2 compute node admin approve <id> asks for it",
        ));
    };
    let stamp = confirm_stamp(who).ok_or_else(owner_only)?;
    let vault = vault()?;
    let proof = vault
        .confirm
        .take(token, admin::CONFIRM_ACTION, Some(&req.id), &stamp)
        .map_err(|_| no(acodes::VAULT_CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for this request, within 60 seconds"))?;
    let via = match proof {
        k2_vault::HumanProof::Confirmed { method } => method,
        k2_vault::HumanProof::None => return Err(no(acodes::VAULT_CONFIRM_REQUIRED, 403, "no owner confirm")),
    };
    let node = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &req.node_id)
    }
    .filter(|n| n.state == "active")
    .ok_or_else(|| no(codes::NODE_NOT_FOUND, 404, "the node was removed"))?;
    // Seal before deciding, so a sealing failure leaves the request pending.
    let key = controller_key()?;
    let now = compute::now();
    let approved_by = who.label();
    let owner_sign = |m: &[u8]| -> Result<[u8; 64], String> { approval_sign(&vault, m) };
    let (sealed, ids) = seal_request(&req, &node, &key, &approved_by, &via, now, &owner_sign)?;
    let decided = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = admin::decide(&conn, &req.id, "approved", &approved_by, Some(&via), None, now).map_err(decide_err)?;
        admin::set_commands(&conn, &r.id, &sealed, &ids).map_err(|e| no("store", 500, e))?;
        store::record_event(
            &conn,
            &approved_by,
            "admin_approved",
            Some(&node.id),
            r.proposer_workspace.as_deref(),
            None,
            json!({"request": r.id, "via": via, "digest": r.digest, "cmdIds": ids}),
        );
        r
    };
    pump(&node.id);
    let after = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::request_by_id(&conn, &decided.id)
    }
    .unwrap_or(decided);
    notify_proposer(&after, &format!("[k2 compute] {} approved by the owner: {}", after.id, after.summary));
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "admin_request", "id": after.id}));
    let sent = after.state == "sent";
    Ok(json!({
        "ok": true,
        "request": request_json(&after, Some(&node.name)),
        "sent": sent,
        "note": if sent { "sent to the node; its result comes back here" } else { "approved; it goes to the node when it's online (within the hour)" },
    }))
}

fn deny(who: &Who, body: &[u8]) -> R<Value> {
    if who.passport().is_some() || !who.is_admin() {
        crate::compute_routes::need_credential(who)?;
        return Err(owner_only());
    }
    let v = json_body(body)?;
    let req = request_or_404(s(&v, "request"))?;
    let why = s(&v, "why").map(|w| w.chars().take(500).collect::<String>());
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = admin::decide(&conn, &req.id, "denied", &who.label(), None, why.as_deref(), compute::now()).map_err(decide_err)?;
        store::record_event(&conn, &who.label(), "admin_denied", Some(&r.node_id), r.proposer_workspace.as_deref(), None, json!({"request": r.id, "why": why}));
        r
    };
    notify_proposer(
        &r,
        &format!("[k2 compute] {} denied by the owner{}: {}", r.id, why.as_deref().map(|w| format!(" ({w})")).unwrap_or_default(), r.summary),
    );
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "admin_request", "id": r.id}));
    Ok(json!({"ok": true, "request": request_json(&r, None)}))
}

fn withdraw(who: &Who, body: &[u8]) -> R<Value> {
    crate::compute_routes::need_credential(who)?;
    let v = json_body(body)?;
    let req = request_or_404(s(&v, "request"))?;
    let mine = match who.passport() {
        Some((ws, _)) => req.proposer_workspace.as_deref() == Some(ws),
        None => who.is_admin() && req.proposer_workspace.is_none() && req.proposed_by == who.label(),
    };
    if !mine {
        return Err(no("forbidden", 403, "only the proposer withdraws a request (the owner denies instead)"));
    }
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = admin::decide(&conn, &req.id, "withdrawn", &who.label(), None, None, compute::now()).map_err(decide_err)?;
        store::record_event(&conn, &who.label(), "admin_withdrawn", Some(&r.node_id), r.proposer_workspace.as_deref(), None, json!({"request": r.id}));
        r
    };
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "admin_request", "id": r.id}));
    Ok(json!({"ok": true, "request": request_json(&r, None)}))
}

fn approval_key(who: &Who) -> R<Value> {
    crate::compute_routes::need_admin(who)?;
    if who.passport().is_some() {
        return Err(owner_only());
    }
    let pk = approval_public_key()?;
    let k = wire::ApprovalKey(pk);
    Ok(json!({"keyId": k.encode(), "fingerprint": k.fingerprint()}))
}

fn expire_and_notify(conn: &rusqlite::Connection) {
    let now = compute::now();
    let mut gone = admin::expire_due(conn, now);
    gone.extend(admin::expire_unsent(conn, now));
    for r in gone {
        store::record_event(conn, "controller", "admin_expired", Some(&r.node_id), r.proposer_workspace.as_deref(), None, json!({"request": r.id}));
        notify_proposer(&r, &format!("[k2 compute] {} expired without running: {}", r.id, r.summary));
    }
}

// ── policy (S3) ──────────────────────────────────────────────────────

/// How long a route waits for a read/operate answer from the node.
#[cfg(not(test))]
const ANSWER_WAIT: Duration = Duration::from_secs(10);
#[cfg(test)]
const ANSWER_WAIT: Duration = Duration::from_secs(3);
/// A cached policy younger than this is used without asking the node.
const POLICY_FRESH_SECS: i64 = 300;

/// Ask the node for its policy (`policy.show`, read) and cache it.
fn refresh_policy(who: &Who, node: &store::NodeRow) -> R<Option<serde_json::Map<String, Value>>> {
    let (_, r) = send_direct(who, node, Class::Read, "policy.show", &json!({}), Some(ANSWER_WAIT))?;
    let Some(r) = r else { return Ok(None) };
    if r.state != "done" {
        return Err(no(
            "node_refused",
            409,
            format!("{} refused policy.show: {}", node.name, r.code.as_deref().or(r.message.as_deref()).unwrap_or("?")),
        ));
    }
    let policy = r.result.get("policy").and_then(Value::as_object).cloned();
    if let Some(p) = &policy {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::set_policy_cache(&conn, &node.id, p, compute::now());
    }
    Ok(policy)
}

fn policy_show(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    let node = node_or_404(params.get("node").map(String::as_str))?;
    reader(who, &node, false)?;
    let live = node_admin_ready(&node, wire::FEATURE_ADMIN).is_ok();
    if live {
        refresh_policy(who, &node)?;
    }
    let cols = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::admin_cols(&conn, &node.id)
    };
    Ok(json!({
        "node": node.name,
        "online": online(&node.id),
        "policy": cols.policy,
        "at": cols.policy_at,
        "live": live,
        "remoteAdmin": cols.remote_admin,
        "known": cols.policy.is_some(),
        "hint": if cols.policy.is_some() { "narrowing applies at once; widening waits for the owner" } else { "this node hasn't reported its policy yet (needs a k2-node with admin, online)" },
    }))
}

fn policy_set(who: &Who, body: &[u8]) -> R<Value> {
    let op = operator(who)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    let changes = v.get("set").and_then(Value::as_object).cloned().ok_or_else(|| no(acodes::BAD_ARGS, 400, "set must be an object of key=value"))?;
    let args = Value::Object(changes.clone());
    admin::validate_step("policy.set", &args).map_err(|(code, hint)| no(code, 400, hint))?;
    let cols = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::admin_cols(&conn, &node.id)
    };
    let mut current = cols.policy.clone();
    if cols.policy_at.is_none_or(|at| compute::now() - at > POLICY_FRESH_SECS) && node_admin_ready(&node, wire::FEATURE_ADMIN).is_ok() {
        current = refresh_policy(who, &node)?.or(current);
    }
    let hold = |why: &str| {
        let summary = admin::summarize(&node.name, "policy.set", &args);
        let body_v = json!({"verb": "policy.set", "args": args});
        let reason = s(&v, "reason").map(str::to_string).or_else(|| (!why.is_empty()).then(|| why.to_string()));
        file_request(&op, &node, "command", Class::Privileged, &body_v, &summary, reason.as_deref())
    };
    match admin::classify_policy_change(&current.unwrap_or_default(), &changes) {
        Class::Read => Ok(json!({"ok": true, "node": node.name, "changed": false, "note": "nothing to change"})),
        Class::Operate => {
            node_admin_ready(&node, wire::FEATURE_ADMIN)?;
            let (cmd_id, result) = send_direct(who, &node, Class::Operate, "policy.set", &args, Some(ANSWER_WAIT))?;
            crate::compute_routes::audit(who, "policy_set", Some(&node.id), op.workspace.as_deref(), None, json!({"set": args, "class": "operate", "cmdId": cmd_id}));
            match result {
                // The node saw a widening we didn't: hold it for the owner.
                Some(r) if r.state == "refused" && r.code.as_deref() == Some(wire::code::APPROVAL_REQUIRED) => hold("the node counts this as a widening"),
                Some(r) if r.state != "done" => Err(no(
                    "node_refused",
                    409,
                    format!("{} refused: {}", node.name, r.message.as_deref().or(r.code.as_deref()).unwrap_or("?")),
                )),
                Some(r) => {
                    if let Some(p) = r.result.get("policy").and_then(Value::as_object) {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        admin::set_policy_cache(&conn, &node.id, p, compute::now());
                    }
                    Ok(json!({"ok": true, "node": node.name, "changed": true, "class": "operate", "cmdId": cmd_id, "policy": r.result.get("policy"), "note": "narrowing applied on the node"}))
                }
                None => Ok(json!({"ok": true, "node": node.name, "changed": true, "class": "operate", "cmdId": cmd_id, "note": "narrowing sent; the node hasn't answered yet"})),
            }
        }
        _ => hold(""),
    }
}

// ── health and alerts (S6) ───────────────────────────────────────────

fn health(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    let node = node_or_404(params.get("node").map(String::as_str))?;
    reader(who, &node, true)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let h = admin::health(&conn, &node.id);
    let cols = admin::admin_cols(&conn, &node.id);
    let alerts_to = admin::setting(&conn, admin::ALERTS_TO);
    let pending = admin::list_requests(&conn, &admin::ReqFilter { node_id: Some(&node.id), state: Some("pending"), limit: 500, ..Default::default() }).len();
    let needs_pin = admin::needs_pin(&conn, &node.id);
    drop(conn);
    Ok(json!({
        "node": node.name,
        "nodeId": node.id,
        "online": online(&node.id),
        "lastSeen": node.last_seen,
        "nodeVersion": node.node_version,
        "at": h.as_ref().map(|(at, _)| *at),
        "facts": h.map(|(_, f)| f),
        "remoteAdmin": cols.remote_admin,
        "approvalKeyFp": cols.approval_key_fp,
        "alertAfterSecs": cols.alert_after_secs,
        "updateRing": cols.update_ring,
        "update": cols.update,
        "alertsTo": alerts_to.as_deref().map(crate::compute_routes::ws_handle),
        "pendingApprovals": pending,
        "features": features(&node.id),
        "needsPin": needs_pin,
        "pinCommand": needs_pin.then_some(admin::PIN_COMMAND),
    }))
}

fn alerts(who: &Who, body: &[u8]) -> R<Value> {
    if who.passport().is_some() {
        crate::compute_routes::need_credential(who)?;
        return Err(owner_only());
    }
    crate::compute_routes::need_admin(who)?;
    let v = json_body(body)?;
    let now = compute::now();
    let mut out = json!({"ok": true});
    if let Some(to) = v.get("to") {
        let ws = match to.as_str().map(str::trim).filter(|x| !x.is_empty() && *x != "none") {
            Some(t) => Some(ws_by_token(t).ok_or_else(|| no("workspace_not_found", 404, format!("no workspace '{t}'")))?),
            None => None,
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::set_setting(&conn, admin::ALERTS_TO, ws.as_ref().map(|w| w.id.as_str()), &who.label(), now).map_err(|e| no("store", 500, e))?;
        store::record_event(&conn, &who.label(), "alerts_to_set", None, ws.as_ref().map(|w| w.id.as_str()), None, json!({}));
        out["alertsTo"] = ws.map(|w| w.handle).into();
    }
    if let Some(tok) = s(&v, "node") {
        let node = node_or_404(Some(tok))?;
        let after = match v.get("after") {
            Some(Value::Null) => None,
            Some(Value::String(x)) if x == "off" => None,
            Some(x) => Some(x.as_i64().ok_or_else(|| no(acodes::BAD_ARGS, 400, "after is seconds, or \"off\""))?),
            None => Some(admin::DEFAULT_ALERT_AFTER_SECS),
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::set_alert_after(&conn, &node.id, after).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
        if let Some(ring) = s(&v, "ring") {
            admin::set_update_ring(&conn, &node.id, ring).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
        }
        store::record_event(&conn, &who.label(), "alerts_set", Some(&node.id), None, None, json!({"afterSecs": after}));
        out["node"] = node.name.into();
        out["alertAfterSecs"] = after.into();
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "alerts"}));
    Ok(out)
}

/// When security updates were first seen pending on a node (settings key).
fn security_key(node_id: &str) -> String {
    format!("security_since:{node_id}")
}

/// A `health` frame from a node.
pub fn on_health(node_id: &str, h: Health) {
    if !h.fits() {
        log_debug!("[compute] node {node_id}: health frame over the cap dropped");
        return;
    }
    let now = compute::now();
    let alerts = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(node) = store::node_by_id(&conn, node_id) else { return };
        let _ = admin::store_health(&conn, node_id, now, &h);
        admin::set_reported_admin(&conn, node_id, h.executor.remote_admin.as_deref(), h.executor.approval_key_fps.first().map(String::as_str));
        let cols = admin::admin_cols(&conn, node_id);
        let floor = cols
            .policy
            .as_ref()
            .and_then(|p| p.get("disk_floor_gb"))
            .and_then(Value::as_u64)
            .map(|gb| gb << 30)
            .unwrap_or(admin::DEFAULT_DISK_FLOOR_BYTES);
        let sec_pending = h.updates.as_ref().and_then(|u| u.security).is_some_and(|n| n > 0);
        let key = security_key(node_id);
        let since = match (sec_pending, admin::setting(&conn, &key).and_then(|v| v.parse::<i64>().ok())) {
            (true, Some(t)) => Some(t),
            (true, None) => {
                let _ = admin::set_setting(&conn, &key, Some(&now.to_string()), "node", now);
                Some(now)
            }
            (false, prev) => {
                if prev.is_some() {
                    let _ = admin::set_setting(&conn, &key, None, "node", now);
                }
                None
            }
        };
        admin::threshold_alerts(&node.name, &h, floor, since, now)
            .into_iter()
            .filter(|a| !admin::alert_recent(&conn, node_id, a.kind, now))
            .collect::<Vec<admin::Alert>>()
    };
    for a in alerts {
        send_alert(node_id, a.kind, &a.text);
    }
    crate::compute_ws::notify_change();
}

/// The node attached. Back-online alert if an offline alert went out.
pub fn on_attach(node_id: &str, boot_id: &str) {
    if let Some(m) = KEYS_SENT.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        m.remove(node_id);
    }
    let now = compute::now();
    let text = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let cols = admin::admin_cols(&conn, node_id);
        if cols.offline_alerted_at.is_none() {
            return;
        }
        admin::set_offline_alerted(&conn, node_id, None);
        let Some(node) = store::node_by_id(&conn, node_id) else { return };
        let since: Option<i64> = conn
            .query_row(
                "SELECT json_extract(detail_json, '$.since') FROM compute_events WHERE kind = 'alert_sent' AND node_id = ?1 \
                 AND json_extract(detail_json, '$.kind') = 'offline' ORDER BY id DESC LIMIT 1",
                rusqlite::params![node_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let old_boot = admin::health(&conn, node_id).map(|(_, f)| f.boot_id);
        let down = now - since.or(cols.offline_alerted_at).unwrap_or(now);
        admin::back_online_text(&node.name, down, old_boot.as_deref(), Some(boot_id))
    };
    send_alert(node_id, "back_online", &text);
}

static KEYS_SENT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// Every offer: pin the approval key on admin-capable nodes that don't
/// report it yet (at most every 10 min), then send queued commands.
pub fn on_offer(node_id: &str, o: &Offer) {
    if !o.features.iter().any(|f| f == wire::FEATURE_ADMIN) {
        return;
    }
    maybe_send_keys(node_id);
    pump(node_id);
}

fn maybe_send_keys(node_id: &str) {
    {
        let mut g = KEYS_SENT.lock().unwrap_or_else(|p| p.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        if m.get(node_id).is_some_and(|t| t.elapsed() < Duration::from_secs(600)) {
            return;
        }
        m.insert(node_id.to_string(), Instant::now());
    }
    let Ok(pk) = approval_public_key() else { return };
    let ours = wire::ApprovalKey(pk);
    let (node, reported) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(n) = store::node_by_id(&conn, node_id) else { return };
        let reported = admin::health(&conn, node_id).map(|(_, f)| f.executor.approval_key_fps).unwrap_or_default();
        (n, reported)
    };
    // Pinned already, or the node pins other keys (rotation needs an old
    // key's signature and a person; never automatic).
    if reported.contains(&ours.fingerprint()) || !reported.is_empty() {
        return;
    }
    let Ok(key) = controller_key() else { return };
    let Ok(nonce) = wire::new_nonce() else { return };
    let mut frame = wire::AdminKeys { keys: vec![ours.encode()], node_fp: node.fingerprint.clone(), iat: compute::now(), nonce, sig_c: String::new(), sig_o: None };
    let Ok(part) = frame.signed_part() else { return };
    let Ok(sig_c) = key.sign_domain(wire::DOMAIN_KEYS_CONTROLLER, &[&part]) else { return };
    frame.sig_c = sig_c;
    if send(node_id, Frame::AdminKeys(frame)) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::record_event(&conn, "controller", "admin_keys_sent", Some(node_id), None, None, json!({"fingerprint": ours.fingerprint()}));
    }
}

/// "Needs one-time pin" (wire §6): a node enrolled after its setup has no
/// controller key pinned for root until someone runs `sudo k2-node admin
/// pin` at it. Any answer that names that sets the state; any executor
/// success clears it.
fn track_pin(node_id: &str, r: &wire::AdminResult) {
    let needs = admin::result_needs_pin(r.code.as_deref(), r.message.as_deref());
    if !needs && r.state != "done" {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let was = admin::needs_pin(&conn, node_id);
    if needs != was {
        admin::set_needs_pin(&conn, node_id, needs, compute::now());
        store::record_event(&conn, "node", if needs { "needs_pin" } else { "pinned" }, Some(node_id), None, None, json!({"code": r.code}));
        drop(conn);
        k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "needs_pin"}));
    }
}

/// An `admin_result` frame: a waiting route's answer, the approval-key
/// pin's outcome, or a request step. Finish (or advance) the request and
/// tell the proposer. Facts only (exit code, the node's refusal code);
/// output tails stay in the request for humans to read.
pub fn on_admin_result(node_id: &str, r: wire::AdminResult) {
    track_pin(node_id, &r);
    if wait_deliver(&r) {
        crate::compute_ws::notify_change();
        return;
    }
    let mut msg: Option<(RequestRow, String)> = None;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let detail = json!({
            "cmdId": r.cmd_id, "state": r.state, "code": r.code, "message": r.message, "exit": r.exit,
            "auditSeq": r.audit_seq, "digest": r.digest,
        });
        if r.cmd_id == "admin_keys" {
            store::record_event(&conn, "node", "admin_keys_result", Some(node_id), None, None, detail);
            return;
        }
        let Some(req) = admin::request_by_cmd_id(&conn, &r.cmd_id) else {
            store::record_event(&conn, "node", "admin_result", Some(node_id), None, None, detail);
            if r.state != "done" {
                log_debug!("[compute] node {node_id}: operate command {} {} ({:?})", r.cmd_id, r.state, r.code);
            }
            drop(conn);
            crate::compute_ws::notify_change();
            return;
        };
        if req.node_id != node_id {
            log_debug!("[compute] node {node_id} reported a result for another node's command");
            return;
        }
        store::record_event(&conn, "node", "admin_result", Some(node_id), req.proposer_workspace.as_deref(), None, detail.clone());
        let step = req.cmd_ids.iter().position(|c| c == &r.cmd_id).unwrap_or(0);
        let mut result = detail;
        result["step"] = (step + 1).into();
        result["of"] = req.cmd_ids.len().into();
        result["stdoutTail"] = r.stdout_tail.clone().into();
        result["stderrTail"] = r.stderr_tail.clone().into();
        result["result"] = r.result.clone();
        if r.state == "done" && step + 1 < req.cmd_ids.len() {
            admin::set_step_result(&conn, &req.id, &result);
            admin::reopen_for_next(&conn, &req.id);
        } else {
            let final_state = if r.state == "done" { "done" } else { "failed" };
            let _ = admin::set_result(&conn, &req.id, final_state, &result);
            let what = match (final_state, r.code.as_deref(), r.exit) {
                ("done", _, Some(e)) => format!("done (exit {e})"),
                ("done", _, None) => "done".to_string(),
                (_, Some(c), _) => format!("{} on the node ({c})", r.state),
                (_, None, Some(e)) => format!("failed (exit {e})"),
                _ => "failed".to_string(),
            };
            msg = Some((req.clone(), format!("[k2 compute] {} {what}: {}", req.id, req.summary)));
        }
    }
    if let Some((req, text)) = msg {
        notify_proposer(&req, &text);
    }
    pump(node_id);
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "admin_request"}));
    crate::compute_ws::notify_change();
}

// ── update and rollback (S5) ─────────────────────────────────────────

/// `(os, arch)` of a node row as the build names them (`macos`/`linux`,
/// `aarch64`/`x86_64`, never `arm64`).
pub(crate) fn node_platform(node: &store::NodeRow) -> R<(&'static str, &'static str)> {
    let os = match node.os.as_deref().or_else(|| node.labels.get("os").map(String::as_str)) {
        Some("macos") | Some("darwin") => "macos",
        Some("linux") => "linux",
        o => return Err(no(acodes::NO_UPDATE, 409, format!("no k2-node build for os {o:?}"))),
    };
    let arch = match node.arch.as_deref().or_else(|| node.labels.get("arch").map(String::as_str)) {
        Some("aarch64") | Some("arm64") => "aarch64",
        Some("x86_64") | Some("amd64") => "x86_64",
        a => return Err(no(acodes::NO_UPDATE, 409, format!("no k2-node build for arch {a:?}"))),
    };
    Ok((os, arch))
}

/// A signed build the controller checked.
pub(crate) struct NodeBuild {
    pub bytes: Vec<u8>,
    pub sig: String,
    pub version: String,
    pub protocol: u32,
    pub sha256: String,
}

/// §8.1 with the node side's checker (`k2_root_proto::node_build`): the
/// root-helper key's signature, the trusted comment naming exactly this
/// os/arch asset, the binary format, one matching build marker. The
/// executor checks again (and the code signature on macOS) before any
/// swap.
pub(crate) fn verify_build(os: &str, arch: &str, bytes: &[u8], sig_text: &str) -> R<NodeBuild> {
    let keys = k2_root_proto::keys::trusted_keys(cfg!(test));
    let v = k2_root_proto::node_build::verify_node_build(bytes, sig_text, &keys, os, arch)
        .map_err(|m| no(acodes::NO_UPDATE, 409, format!("{}: {m}", k2_root_proto::node_build::asset_name(os, arch))))?;
    Ok(NodeBuild { sha256: crypto::sha256_hex(bytes), bytes: bytes.to_vec(), sig: sig_text.to_string(), version: v.version.to_string(), protocol: v.protocol })
}

fn load_build(os: &str, arch: &str) -> R<NodeBuild> {
    let asset = k2_root_proto::node_build::asset_name(os, arch);
    let dir = assets_dir();
    let bin = dir.join(&asset);
    let sig = dir.join(format!("{asset}.sig"));
    if !bin.exists() || !sig.exists() {
        fetch_build(&asset, &dir)?;
    }
    let bytes = std::fs::read(&bin).map_err(|e| no(acodes::NO_UPDATE, 409, format!("read {}: {e}", bin.display())))?;
    let sig_text = std::fs::read_to_string(&sig).map_err(|e| no(acodes::NO_UPDATE, 409, format!("read {}: {e}", sig.display())))?;
    verify_build(os, arch, &bytes, &sig_text)
}

/// Download this release's build into `dir` (not air-gapped, not tests).
fn fetch_build(asset: &str, dir: &std::path::Path) -> R<()> {
    let missing = || no(acodes::NO_UPDATE, 409, format!("no {asset} here: put the release asset and its .sig in {}", dir.display()));
    if cfg!(test) || k2_core::airgap::enabled() {
        return Err(missing());
    }
    let base = std::env::var("K2_NODE_RELEASE_BASE").unwrap_or_else(|_| "https://github.com/Alakazam-211/K2/releases/download".into());
    let url = format!("{}/v{}/{asset}", base.trim_end_matches('/'), env!("CARGO_PKG_VERSION"));
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| no(acodes::NO_UPDATE, 502, format!("http client: {e}")))?;
    let get = |u: &str| -> R<Vec<u8>> {
        let r = client.get(u).send().map_err(|e| no(acodes::NO_UPDATE, 502, format!("{u}: {e}")))?;
        if !r.status().is_success() {
            return Err(missing());
        }
        r.bytes().map(|b| b.to_vec()).map_err(|e| no(acodes::NO_UPDATE, 502, format!("{u}: {e}")))
    };
    let bin = get(&url)?;
    let sig = get(&format!("{url}.sig"))?;
    std::fs::create_dir_all(dir).map_err(|e| no("store", 500, e.to_string()))?;
    std::fs::write(dir.join(format!("{asset}.sig")), sig).map_err(|e| no("store", 500, e.to_string()))?;
    std::fs::write(dir.join(asset), bin).map_err(|e| no("store", 500, e.to_string()))?;
    Ok(())
}

fn newer(candidate: &str, running: Option<&str>) -> bool {
    use k2_root_proto::version::Semver;
    match (Semver::parse(candidate), running.and_then(|r| Semver::parse(r.split(['-', ' ']).next().unwrap_or(r)))) {
        (Some(c), Some(r)) => c > r,
        (Some(_), None) => true,
        _ => false,
    }
}

fn update(who: &Who, body: &[u8]) -> R<Value> {
    let _op = operator(who)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    node_admin_ready(&node, wire::FEATURE_SELF_UPDATE)?;
    let (os, arch) = node_platform(&node)?;
    let build = load_build(os, arch)?;
    if !newer(&build.version, node.node_version.as_deref()) {
        return Err(no(
            acodes::NO_UPDATE,
            409,
            format!("{} already runs {} (offered {}); nodes only move to newer signed builds", node.name, node.node_version.as_deref().unwrap_or("?"), build.version),
        ));
    }
    if b_true(&v, "canary") {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = admin::set_update_ring(&conn, &node.id, "canary");
    }
    let transfer_id = crypto::random_id().map_err(|e| no("random", 500, e))?;
    let args = json!({"version": build.version, "sha256": build.sha256, "trigger": "explicit"});
    let key = controller_key()?;
    let cmd = build_command(&node, &key.fingerprint(), Class::Operate, "node.update", &args, None, &who.label(), None, compute::now())?;
    let sealed = wire::sign_controller(&key, &cmd).map_err(|e| no("seal_failed", 500, e))?;
    let offer = frames::UpdateOffer {
        transfer_id: transfer_id.clone(),
        version: build.version.clone(),
        protocol: build.protocol,
        os: os.to_string(),
        arch: arch.to_string(),
        sha256: build.sha256.clone(),
        size: build.bytes.len() as u64,
        sig: build.sig.clone(),
        cmd: sealed,
    };
    if !send(&node.id, Frame::UpdateOffer(offer)) {
        return Err(no(codes::NODE_OFFLINE, 409, format!("{} went offline", node.name)));
    }
    let chunks: Vec<&[u8]> = build.bytes.chunks(frames::TRANSFER_CHUNK).collect();
    let last = chunks.len().saturating_sub(1);
    for (i, c) in chunks.iter().enumerate() {
        let f = Frame::UpdateChunk(frames::UpdateChunk { transfer_id: transfer_id.clone(), seq: i as u32, data: crypto::b64(c), last: i == last });
        if !send(&node.id, f) {
            return Err(no(codes::NODE_OFFLINE, 409, format!("{} went offline mid-transfer", node.name)));
        }
    }
    let state = json!({"state": "offered", "version": build.version, "from": node.node_version, "transferId": transfer_id, "cmdId": cmd.cmd_id, "at": compute::now(), "by": who.label()});
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::set_update(&conn, &node.id, &state);
    }
    crate::compute_routes::audit(who, "update_offered", Some(&node.id), None, None, json!({"version": build.version, "cmdId": cmd.cmd_id, "sha256": build.sha256}));
    Ok(json!({"ok": true, "node": node.name, "update": state, "note": "the node verifies, swaps, and rolls back by itself if the new build doesn't come back healthy in 120 s"}))
}

fn b_true(v: &Value, k: &str) -> bool {
    crate::compute_routes::b(v, k) == Some(true)
}

fn rollback(who: &Who, body: &[u8]) -> R<Value> {
    let _op = operator(who)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    node_admin_ready(&node, wire::FEATURE_SELF_UPDATE)?;
    let args = match s(&v, "reason") {
        Some(r) => json!({"reason": r.chars().take(200).collect::<String>()}),
        None => json!({}),
    };
    let cmd_id = send_operate(who, &node, "node.rollback", &args)?;
    let state = json!({"state": "rollback_requested", "at": compute::now(), "by": who.label(), "cmdId": cmd_id});
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::set_update(&conn, &node.id, &state);
    }
    Ok(json!({"ok": true, "node": node.name, "update": state, "note": "only to the node's own .prev build, never a download of an older one"}))
}

fn phase_str(p: wire::UpdatePhase) -> String {
    serde_json::to_value(p).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "unknown".into())
}

/// An `update_state` frame. `healthy`/`rolled_back` may repeat after a
/// re-attach (wire §1): deduplicated by `cmd_id` + phase.
pub fn on_update_state(node_id: &str, u: frames::UpdateState) {
    let phase = phase_str(u.phase);
    let (name, fresh) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(node) = store::node_by_id(&conn, node_id) else { return };
        let prev = admin::admin_cols(&conn, node_id).update;
        let dup = prev.as_ref().is_some_and(|p| {
            p.get("state").and_then(Value::as_str) == Some(phase.as_str())
                && p.get("cmdId").and_then(Value::as_str) == u.cmd_id.as_deref()
                && p.get("version").and_then(Value::as_str) == Some(u.version.as_str())
        });
        if !dup {
            let state = json!({
                "state": phase, "version": u.version, "from": u.from, "code": u.code, "reason": u.message,
                "transferId": u.transfer_id, "cmdId": u.cmd_id, "at": u.at,
            });
            admin::set_update(&conn, node_id, &state);
            store::record_event(&conn, "node", &format!("update_{phase}"), Some(node_id), None, None, state);
        }
        (node.name, !dup)
    };
    if fresh && matches!(u.phase, wire::UpdatePhase::RolledBack | wire::UpdatePhase::Refused | wire::UpdatePhase::Failed) {
        let why = u.message.as_deref().or(u.code.as_deref()).map(|r| format!(" ({r})")).unwrap_or_default();
        let text = match u.phase {
            wire::UpdatePhase::RolledBack => format!("{name}: k2-node {} didn't come back healthy and was rolled back{why}", u.version),
            wire::UpdatePhase::Refused => format!("{name}: k2-node refused update {}{why}", u.version),
            _ => format!("{name}: k2-node update {} failed{why}", u.version),
        };
        send_alert(node_id, "update_failed", &text);
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "node_update"}));
    crate::compute_ws::notify_change();
}

// ── delivery ─────────────────────────────────────────────────────────

/// Tell the proposer how its request went: its session if it had one,
/// else its workspace inbox. Humans see it in Settings.
fn notify_proposer(r: &RequestRow, text: &str) {
    let Some(ws) = r.proposer_workspace.clone() else { return };
    let session = r.proposer_session.clone();
    deliver(Some(&ws), session.as_deref(), &format!("compute request {}", r.id), text);
}

/// An alert to the "alerts go to" workspace (§10.2), once per kind per
/// node per 6 h (back-online always), audited as `alert_sent`.
pub fn send_alert(node_id: &str, kind: &str, text: &str) {
    let now = compute::now();
    let to = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if kind != "back_online" && kind != "offline" && admin::alert_recent(&conn, node_id, kind, now) {
            return;
        }
        let since = if kind == "offline" { store::node_by_id(&conn, node_id).and_then(|n| n.last_seen) } else { None };
        let to = admin::setting(&conn, admin::ALERTS_TO);
        store::record_event(&conn, "controller", "alert_sent", Some(node_id), to.as_deref(), None, json!({"kind": kind, "since": since, "to": to}));
        to
    };
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "alert", "kind": kind, "text": text}));
    if let Some(ws) = to {
        deliver(Some(&ws), None, &format!("compute node alert: {kind}"), &format!("[k2 compute alert] {text}"));
    }
}

fn deliver(ws_id: Option<&str>, session: Option<&str>, title: &str, text: &str) {
    #[cfg(test)]
    {
        test_seams::record_message(ws_id.unwrap_or(""), session, text);
        let _ = title;
    }
    #[cfg(not(test))]
    {
        let ws_path = ws_id.and_then(crate::compute_routes::ws_by_id).map(|w| w.path);
        let session = session.map(str::to_string);
        let (title, text) = (title.to_string(), text.to_string());
        std::thread::spawn(move || {
            if let Some(sid) = session.as_deref().filter(|s| !s.is_empty()) {
                let r = crate::workspace_msg::deliver_live(sid, &text, "compute", "", true, crate::workspace_msg::DEFAULT_WAKE_TIMEOUT);
                if r.success {
                    return;
                }
            }
            if let Some(p) = ws_path {
                if let Err(e) = k2_core::inbox::compose(std::path::Path::new(&p), &title, &text, None, Some("compute"), Some("compute")) {
                    log_debug!("[compute] admin inbox delivery: {e}");
                }
                if session.is_none() {
                    let _ = crate::workspace_msg::deliver_live(&p, &text, "compute", "", true, crate::workspace_msg::DEFAULT_WAKE_TIMEOUT);
                }
            }
        });
    }
}

// ── the watch (headless) ─────────────────────────────────────────────

/// One pass: expire requests, offline alerts.
pub fn watch_once() {
    if compute::enabled() {
        watch_pass();
    }
}

fn watch_pass() {
    let now = compute::now();
    let due: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        expire_and_notify(&conn);
        store::list_nodes(&conn, false)
            .into_iter()
            .filter(|nd| admin::offline_due(&nd.state, online(&nd.id), nd.last_seen, &admin::admin_cols(&conn, &nd.id), now))
            .map(|nd| {
                let h = admin::health(&conn, &nd.id).map(|(_, f)| f);
                let text = admin::offline_text(&nd.name, nd.last_seen.unwrap_or(now), nd.last_route.as_deref(), h.as_ref());
                admin::set_offline_alerted(&conn, &nd.id, Some(now));
                (nd.id, text)
            })
            .collect()
    };
    for (id, text) in due {
        send_alert(&id, "offline", &text);
    }
}

/// Start the 60 s watch (boot). Runs with no client attached.
pub fn spawn_watch() {
    tokio::spawn(async {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let _ = tokio::task::spawn_blocking(watch_once).await;
        }
    });
}

// ── test seams ───────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_seams {
    use super::*;
    use std::sync::{Arc, OnceLock};

    pub struct FakeNode {
        pub online: bool,
        pub features: Vec<String>,
        pub sent: Vec<Frame>,
    }

    static NODES: Mutex<Option<HashMap<String, FakeNode>>> = Mutex::new(None);
    static VAULT: OnceLock<(k2_vault::fsutil::TestDir, Arc<k2_vault::LocalVault>)> = OnceLock::new();
    static KEY: OnceLock<String> = OnceLock::new();
    static MESSAGES: Mutex<Vec<(String, Option<String>, String)>> = Mutex::new(Vec::new());
    thread_local! {
        static ASSETS: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    pub fn register(node_id: &str, online: bool, features: &[&str]) {
        NODES.lock().unwrap().get_or_insert_with(HashMap::new).insert(
            node_id.to_string(),
            FakeNode { online, features: features.iter().map(|s| s.to_string()).collect(), sent: Vec::new() },
        );
    }

    pub fn set_online(node_id: &str, on: bool) {
        if let Some(n) = NODES.lock().unwrap().as_mut().and_then(|m| m.get_mut(node_id)) {
            n.online = on;
        }
    }

    pub fn take_sent(node_id: &str) -> Vec<Frame> {
        NODES.lock().unwrap().as_mut().and_then(|m| m.get_mut(node_id)).map(|n| std::mem::take(&mut n.sent)).unwrap_or_default()
    }

    pub(super) fn online(node_id: &str) -> Option<bool> {
        NODES.lock().unwrap().as_ref().and_then(|m| m.get(node_id)).map(|n| n.online)
    }

    pub(super) fn features(node_id: &str) -> Option<Vec<String>> {
        NODES.lock().unwrap().as_ref().and_then(|m| m.get(node_id)).map(|n| n.features.clone())
    }

    pub(super) fn send(node_id: &str, f: &Frame) -> Option<bool> {
        let mut g = NODES.lock().unwrap();
        let n = g.as_mut()?.get_mut(node_id)?;
        if !n.online {
            return Some(false);
        }
        n.sent.push(f.clone());
        Some(true)
    }

    pub fn vault_handle() -> Arc<k2_vault::LocalVault> {
        VAULT
            .get_or_init(|| {
                let dir = k2_vault::fsutil::TestDir::new("compute-admin");
                let ks: Arc<dyn k2_vault::KeyStore> = Arc::new(k2_vault::keystore::fake::FakeKeyStore::new());
                let v = Arc::new(
                    k2_vault::LocalVault::open(dir.path().join("vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
                        .expect("open test vault"),
                );
                (dir, v)
            })
            .1
            .clone()
    }

    pub(super) fn vault() -> Option<Arc<k2_vault::LocalVault>> {
        Some(vault_handle())
    }

    pub fn key_pem() -> String {
        KEY.get_or_init(|| crypto::SigningKey::generate().unwrap().to_pkcs8_pem().unwrap()).clone()
    }

    pub(super) fn controller_key() -> Option<crypto::SigningKey> {
        crypto::SigningKey::from_pkcs8_pem(&key_pem()).ok()
    }

    pub fn set_assets_dir(p: Option<std::path::PathBuf>) {
        ASSETS.with(|a| *a.borrow_mut() = p);
    }

    pub(super) fn assets_dir() -> Option<std::path::PathBuf> {
        Some(ASSETS.with(|a| a.borrow().clone()).unwrap_or_else(|| std::env::temp_dir().join("k2-compute-admin-no-assets")))
    }

    pub(super) fn record_message(ws: &str, session: Option<&str>, text: &str) {
        MESSAGES.lock().unwrap().push((ws.to_string(), session.map(str::to_string), text.to_string()));
    }

    /// Messages delivered to `ws` (taken).
    pub fn take_messages(ws: &str) -> Vec<String> {
        let mut g = MESSAGES.lock().unwrap();
        let (mine, rest): (Vec<_>, Vec<_>) = g.drain(..).partition(|(w, _, _)| w == ws);
        *g = rest;
        mine.into_iter().map(|(_, _, t)| t).collect()
    }
}

#[cfg(test)]
mod tests;
