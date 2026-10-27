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
//! - `nodes/wall`: turn the Linux job wall on (`wall.enable`, operate at
//!   `policy` on 0.46.4+ nodes, Q5): sent at once; an older node asks for
//!   the owner, so it becomes a held plan (wall.enable + unit.restart).
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

use crate::compute_manage::Manager;
use crate::compute_routes::{json_body, no, node_or_404, owner_only, s, ws_by_token, Who, R};
use k2_core::compute::proto::share as pshare;
use k2_core::compute::share;

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
/// Turn the Linux job wall on (prd-compute-update-continuity-v1 Q5).
pub const NODE_WALL: &str = "/cli/compute/nodes/wall";
/// Node sharing (prd-compute-shared-queue-v1 §9): the servers on a node
/// this server owns (`k2 compute node servers <node> …`).
pub const SERVERS: &str = "/cli/compute/nodes/servers";
pub const SERVERS_ADD: &str = "/cli/compute/nodes/servers/add";
pub const SERVERS_LIMITS: &str = "/cli/compute/nodes/servers/limits";
pub const SERVERS_PAUSE: &str = "/cli/compute/nodes/servers/pause";
pub const SERVERS_REVOKE: &str = "/cli/compute/nodes/servers/revoke";
/// Make another of this owner's own servers on a node its owner
/// (prd-macos-tenant-compute-v1 §6): owner only, held for approval.
pub const SERVERS_OWNER: &str = "/cli/compute/nodes/servers/owner";
/// Take over a node that says this server owns it now (after an owner
/// change on it): owner only, in person (prd-macos-tenant-compute-v1 MT36).
pub const TAKE_OWNER: &str = "/cli/compute/nodes/take-owner";

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
    NODE_WALL,
    SERVERS,
    SERVERS_ADD,
    SERVERS_LIMITS,
    SERVERS_PAUSE,
    SERVERS_REVOKE,
    SERVERS_OWNER,
    TAKE_OWNER,
];

/// POST-only (also in `compute_routes::POST_ROUTES`).
pub const POST_ROUTES: &[&str] = &[
    APPROVE,
    DENY,
    PROPOSE,
    WITHDRAW,
    NODE_ALERTS,
    NODE_POLICY_SET,
    NODE_ROLLBACK,
    NODE_UPDATE,
    NODE_WALL,
    SERVERS_ADD,
    SERVERS_LIMITS,
    SERVERS_PAUSE,
    SERVERS_REVOKE,
    SERVERS_OWNER,
    TAKE_OWNER,
];

/// Passport-reachable (also in `session_token::is_agent_verb`). approve,
/// deny, alerts and approval-key are NOT here.
pub const AGENT_VERBS: &[&str] = &[PROPOSE, REQUESTS, WITHDRAW, NODE_HEALTH, NODE_POLICY, NODE_POLICY_SET, NODE_ROLLBACK, NODE_UPDATE, NODE_WALL];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

/// Route a node admin request (already past the preview flag). `mgr`: a
/// local agent under "Allow managing compute nodes" (servers verbs).
pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<Value> {
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
        NODE_WALL => wall(who, body),
        SERVERS => servers_list(who, params, mgr),
        SERVERS_ADD => servers_add(who, body, mgr),
        SERVERS_LIMITS => servers_limits(who, body, mgr),
        SERVERS_PAUSE => servers_pause(who, body, mgr),
        SERVERS_REVOKE => servers_revoke(who, body),
        SERVERS_OWNER => servers_owner(who, body),
        TAKE_OWNER => take_owner(who, body),
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
    not_tenant(node)?;
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
        // Changing a node's owner is the server owner's alone, through its
        // own route (its checks, the full key on the card); never a
        // generic proposal, by an agent or anyone (review F4).
        if st.verb == pshare::VERB_OWNER {
            return Err(no(
                codes::OWNER_ONLY,
                403,
                "servers.owner is never proposed: the server's owner asks for it in Settings → Compute or with k2 compute node servers <node> owner <server>",
            ));
        }
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
    // V52: never queue node admin for a node another server owns.
    not_tenant(node)?;
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
    // V79: an add's enrol-code binding is a credential; never shown.
    v["body"] = admin::redact_body(&r.body);
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
    not_tenant(&node)?;
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
        if req.steps().is_ok_and(|s| s.iter().any(|st| st.verb.starts_with("servers."))) {
            // A servers.* change landed (or failed): mirror the list again.
            refresh_servers_soon(node_id);
        }
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

/// The k2-node build this server offers its nodes: always the one of this
/// server's own version, so nodes never get stuck behind a stale cache
/// after the server updates.
///
/// - The cache is per version (`node-assets/<version>/<asset>` + `.sig`);
///   another version's files (and the old unversioned ones) are never
///   used and are removed.
/// - A cached build is used when its signature checks out and it IS this
///   version; a build that fails the check is an error (tampered), one of
///   another version is replaced.
/// - Filling the cache prefers the k2-node bundled beside this daemon
///   (`K2.app/Contents/MacOS/k2-node`) with the release's `.sig` (a small
///   download), then the release asset itself. Every build is checked
///   (signature over the bytes, the asset's name and the version) before
///   it's offered.
fn load_build(os: &str, arch: &str) -> R<NodeBuild> {
    let asset = k2_root_proto::node_build::asset_name(os, arch);
    let want = server_version();
    let root = assets_dir();
    let dir = root.join(&want);
    prune_other_versions(&root, &want);
    let drop_cached = || {
        let _ = std::fs::remove_file(dir.join(&asset));
        let _ = std::fs::remove_file(dir.join(format!("{asset}.sig")));
    };
    match cached_build(os, arch, &dir, &asset) {
        Ok(Some(b)) if b.version == want => return Ok(b),
        // Another version's build under this version's name: replace it.
        Ok(Some(_)) => drop_cached(),
        Ok(None) => {}
        // A pair that fails the check (a crash between the two writes, or
        // tampering) is never offered: refilled once from the source; if
        // that fails too, the check's error stands.
        Err(bad) => {
            drop_cached();
            if fill_cache(os, arch, &asset, &want, &dir).is_err() {
                return Err(bad);
            }
            return match cached_build(os, arch, &dir, &asset)? {
                Some(b) if b.version == want => Ok(b),
                _ => Err(bad),
            };
        }
    }
    fill_cache(os, arch, &asset, &want, &dir)?;
    let b = cached_build(os, arch, &dir, &asset)?.ok_or_else(|| no(acodes::NO_UPDATE, 409, format!("no {asset} in {}", dir.display())))?;
    if b.version != want {
        return Err(no(acodes::NO_UPDATE, 409, format!("{asset} for {want} is version {} (a release asset mismatch)", b.version)));
    }
    Ok(b)
}

/// This server's version (the node build it offers matches it).
fn server_version() -> String {
    #[cfg(test)]
    if let Some(v) = test_seams::server_version() {
        return v;
    }
    env!("CARGO_PKG_VERSION").to_string()
}

/// The cached build in `dir`, checked; `None` when not cached.
fn cached_build(os: &str, arch: &str, dir: &std::path::Path, asset: &str) -> R<Option<NodeBuild>> {
    let (bin, sig) = (dir.join(asset), dir.join(format!("{asset}.sig")));
    if !bin.exists() || !sig.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&bin).map_err(|e| no(acodes::NO_UPDATE, 409, format!("read {}: {e}", bin.display())))?;
    let sig_text = std::fs::read_to_string(&sig).map_err(|e| no(acodes::NO_UPDATE, 409, format!("read {}: {e}", sig.display())))?;
    verify_build(os, arch, &bytes, &sig_text).map(Some)
}

/// Remove every other version's cache dir and the old unversioned files.
fn prune_other_versions(root: &std::path::Path, want: &str) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == want {
            continue;
        }
        let ft = e.file_type().ok();
        let r = if ft.is_some_and(|t| t.is_dir()) { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
        if let Err(x) = r {
            log_debug!("[compute] node-assets: remove {}: {x}", e.path().display());
        }
    }
}

/// Put this version's build and its `.sig` in `dir`: the bundled k2-node
/// with the release's `.sig` when they match, else the release asset.
fn fill_cache(os: &str, arch: &str, asset: &str, want: &str, dir: &std::path::Path) -> R<()> {
    std::fs::create_dir_all(dir).map_err(|e| no("store", 500, e.to_string()))?;
    let sig = fetch_release(asset, want, true)?;
    let sig_text = String::from_utf8(sig.clone()).map_err(|_| no(acodes::NO_UPDATE, 409, format!("{asset}.sig isn't text")))?;
    if let Some(bundled) = bundled_node(os, arch) {
        if let Ok(bytes) = std::fs::read(&bundled) {
            if verify_build(os, arch, &bytes, &sig_text).is_ok_and(|b| b.version == want) {
                return store(dir, asset, &bytes, &sig);
            }
        }
    }
    let bin = fetch_release(asset, want, false)?;
    store(dir, asset, &bin, &sig)
}

/// Each file is written whole (temp + rename); the pair is checked on
/// every read and refilled when it doesn't match.
fn store(dir: &std::path::Path, asset: &str, bin: &[u8], sig: &[u8]) -> R<()> {
    let put = |name: String, bytes: &[u8]| -> R<()> {
        let tmp = dir.join(format!(".{name}.tmp-{}", crypto::random_id().map_err(|e| no("random", 500, e))?));
        std::fs::write(&tmp, bytes).map_err(|e| no("store", 500, e.to_string()))?;
        std::fs::rename(&tmp, dir.join(&name)).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            no("store", 500, e.to_string())
        })
    };
    put(asset.to_string(), bin)?;
    put(format!("{asset}.sig"), sig)
}

/// The k2-node this K2 ships beside the daemon (macOS app bundle), for its
/// own platform only.
fn bundled_node(os: &str, arch: &str) -> Option<std::path::PathBuf> {
    #[cfg(test)]
    if let Some(p) = test_seams::bundled_node() {
        return Some(p);
    }
    if os != std::env::consts::OS || k2_root_proto::node_build::asset_name(os, arch) != k2_root_proto::node_build::asset_name(std::env::consts::OS, std::env::consts::ARCH) {
        return None;
    }
    let p = std::env::current_exe().ok()?.parent()?.join("k2-node");
    p.is_file().then_some(p)
}

/// One file of this version's release: the asset, or its `.sig`.
fn fetch_release(asset: &str, version: &str, sig: bool) -> R<Vec<u8>> {
    let name = if sig { format!("{asset}.sig") } else { asset.to_string() };
    let missing = || no(acodes::NO_UPDATE, 409, format!("no {name} for {version}: put the release asset and its .sig in {}", assets_dir().join(version).display()));
    #[cfg(test)]
    {
        let dir = test_seams::release_dir().ok_or_else(missing)?;
        return std::fs::read(dir.join(&name)).map_err(|_| missing());
    }
    #[cfg(not(test))]
    {
        if k2_core::airgap::enabled() {
            return Err(missing());
        }
        let base = std::env::var("K2_NODE_RELEASE_BASE").unwrap_or_else(|_| "https://github.com/Alakazam-211/K2/releases/download".into());
        let url = format!("{}/v{version}/{name}", base.trim_end_matches('/'));
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| no(acodes::NO_UPDATE, 502, format!("http client: {e}")))?;
        let r = client.get(&url).send().map_err(|e| no(acodes::NO_UPDATE, 502, format!("{url}: {e}")))?;
        if !r.status().is_success() {
            return Err(missing());
        }
        r.bytes().map(|b| b.to_vec()).map_err(|e| no(acodes::NO_UPDATE, 502, format!("{url}: {e}")))
    }
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
    let mut args = json!({"version": build.version, "sha256": build.sha256, "trigger": "explicit"});
    // V55 `--now`: skip a shared node's drain. Sent only to a shared node
    // (solo nodes never drain, and an older k2-node doesn't know it).
    if b_true(&v, "now") && node.shared {
        args[wire::ARG_NOW] = true.into();
    }
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
    // V55: a shared node finishes every server's running jobs first.
    let shared = node.shared;
    if shared {
        crate::compute_routes::audit(who, "node_update_drain", Some(&node.id), None, None, json!({"version": build.version}));
    }
    let note = if shared {
        "the node is shared: it takes no new jobs and waits for running ones to finish (up to 30 min) before it swaps; it rolls back by itself if the new build doesn't come back healthy in 120 s"
    } else {
        "the node verifies, swaps, and rolls back by itself if the new build doesn't come back healthy in 120 s"
    };
    Ok(json!({"ok": true, "node": node.name, "update": state, "shared": shared, "note": note}))
}

fn b_true(v: &Value, k: &str) -> bool {
    crate::compute_routes::b(v, k) == Some(true)
}

fn rollback(who: &Who, body: &[u8]) -> R<Value> {
    let _op = operator(who)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    node_admin_ready(&node, wire::FEATURE_SELF_UPDATE)?;
    let mut args = match s(&v, "reason") {
        Some(r) => json!({"reason": r.chars().take(200).collect::<String>()}),
        None => json!({}),
    };
    if b_true(&v, "now") && node.shared {
        args[wire::ARG_NOW] = true.into();
    }
    let cmd_id = send_operate(who, &node, "node.rollback", &args)?;
    let state = json!({"state": "rollback_requested", "at": compute::now(), "by": who.label(), "cmdId": cmd_id});
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::set_update(&conn, &node.id, &state);
    }
    Ok(json!({"ok": true, "node": node.name, "update": state, "note": "only to the node's own .prev build, never a download of an older one"}))
}

/// `POST /cli/compute/nodes/wall {node, restart?, reason?}` (Q5): turn the
/// Linux job wall on. An operator (owner, Admin login, or an agent with
/// "Allow this agent to operate compute nodes") sends it at once: 0.46.4+
/// executors class `wall.enable` operate at `policy`. `restart` (default
/// true) restarts k2-node once its running jobs end. An older node answers
/// `approval_required`: then it is held for the owner. A node offering
/// `wall-restart` answers that only when its owner opted out of the wall
/// at setup (or set their own job user): the held command is the same
/// `wall.enable {restart}`, which restarts through a drain that never
/// kills a job. Any other node (older, or a build whose version says
/// nothing reliable) gets the safe hold: `wall.enable {}` alone, the wall
/// taking effect at k2-node's next restart. Never `unit.restart`, which
/// would kill unwalled jobs (A-N4, round 4 item 4).
fn wall(who: &Who, body: &[u8]) -> R<Value> {
    let op = operator(who)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    not_tenant(&node)?;
    let restart = match v.get("restart") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(no(acodes::BAD_ARGS, 400, "restart is true or false")),
    };
    let args = json!({"restart": restart});
    admin::validate_step("wall.enable", &args).map_err(|(code, hint)| no(code, 400, hint))?;
    node_admin_ready(&node, wire::FEATURE_ADMIN)?;
    let wall_restart = features(&node.id).iter().any(|f| f == wire::FEATURE_WALL_RESTART);
    let hold = |why: String| {
        let body_v = if wall_restart { json!({"verb": "wall.enable", "args": args.clone()}) } else { json!({"verb": "wall.enable", "args": {}}) };
        let kind = "command";
        let (_, summary) = classify_body(&node.name, kind, &body_v)?;
        let reason = s(&v, "reason").map(str::to_string).unwrap_or(why);
        file_request(&op, &node, kind, Class::Privileged, &body_v, &summary, Some(&reason))
    };
    let (cmd_id, result) = send_direct(who, &node, Class::Operate, "wall.enable", &args, Some(ANSWER_WAIT))?;
    crate::compute_routes::audit(who, "wall_enable", Some(&node.id), op.workspace.as_deref(), None, json!({"restart": restart, "cmdId": cmd_id}));
    match result {
        Some(r) if r.state == "refused" && r.code.as_deref() == Some(wire::code::APPROVAL_REQUIRED) => {
            hold(if wall_restart {
                "this machine's owner turned the job wall off at setup (or set their own job user), so turning it on needs their approval".to_string()
            } else {
                let v = node.node_version.as_deref().filter(|v| !v.trim().is_empty()).map(|v| format!("k2-node {v}")).unwrap_or_else(|| "k2-node of an unknown version".into());
                format!("{v} doesn't offer safe wall restarts (wall-restart), so turning the wall on needs the owner's approval there, and it's held without a restart: the wall takes effect when k2-node next restarts")
            })
        }
        Some(r) if r.state != "done" => Err(no(
            "node_refused",
            409,
            format!("{} refused wall.enable: {}", node.name, r.message.as_deref().or(r.code.as_deref()).unwrap_or("?")),
        )),
        Some(r) => {
            let note = wall_note(&r.result, restart);
            Ok(json!({"ok": true, "node": node.name, "cmdId": cmd_id, "result": r.result, "note": note}))
        }
        None => Ok(json!({"ok": true, "node": node.name, "cmdId": cmd_id, "note": "sent; the node hasn't answered yet (k2 compute node health)"})),
    }
}

/// What a `wall.enable` answer means, in one line.
fn wall_note(result: &Value, restart: bool) -> String {
    if let Some(n) = result.get("note").and_then(Value::as_str) {
        return n.to_string();
    }
    if result.pointer("/restart/restarted").is_some() {
        return "the wall is set up; k2-node restarted to use it (no job was running)".into();
    }
    if restart {
        return "the wall is set up; k2-node restarts to use it once its running jobs end (if any still run after 30 min it gives up and restarts nothing; asking again, k2 compute node wall <node>, finishes it)".into();
    }
    "the wall is set up; it takes effect when k2-node restarts".into()
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
            let mut state = json!({
                "state": phase, "version": u.version, "from": u.from, "code": u.code, "reason": u.message,
                "transferId": u.transfer_id, "cmdId": u.cmd_id, "at": u.at,
            });
            // The executor's step log (0.46.4 nodes; prd-compute-update-continuity-v1 §5.9).
            if !u.steps.is_empty() {
                state["steps"] = serde_json::to_value(&u.steps).unwrap_or_default();
            }
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
        // In the daemon's runtime: a wake spawns the agent's session (`crate::rt`).
        let spawned = crate::rt::spawn("compute-admin-note", move || {
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
        if let Err(e) = spawned {
            log_debug!("[compute] admin note not delivered: {e}");
        }
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

/// Start the 60 s watch (boot). Runs with no client attached. It also
/// starts the compute ticker at boot (V77).
pub fn spawn_watch() {
    tokio::spawn(async {
        // This task starts before boot opens the database: wait for it, or
        // the first tick's ticker check panics and takes the whole watch
        // (offline alerts, request expiry, V77) down for the process's life.
        while k2_core::db::try_shared().is_none() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            crate::compute_ws::ensure_ticker_at_boot();
            let _ = tokio::task::spawn_blocking(watch_once).await;
        }
    });
}

// ── shared nodes: the servers on a node (prd-compute-shared-queue-v1) ─

/// `not_node_owner`: another server owns this node (V52). Its admin,
/// updates, health and server list are that server's.
fn not_tenant(node: &store::NodeRow) -> R<()> {
    if node.is_tenant() {
        return Err(no(
            pshare::codes::NOT_NODE_OWNER,
            403,
            format!(
                "{} is shared with this server by {}: its admin, updates and servers are that server's (k2 compute node servers {} shows this server's share)",
                node.name,
                node.owner_label.as_deref().unwrap_or("its owner"),
                node.name
            ),
        ));
    }
    Ok(())
}

/// This server's role on a node flipped (prd-macos-tenant-compute-v1
/// §6.3, MT35): an owner change on the node, or a re-enrol.
///
/// - To `tenant`: the mirrored server list goes, every open admin request
///   for the node fails `not_node_owner` (its proposer is told), and the
///   person gets one note.
/// - To `owner`: the cached health from before goes (so the approval key
///   is pinned again at once: the new owner's `admin.toml` has none), the
///   server list is mirrored, and the person gets one note.
pub fn on_role_change(node_id: &str, from: &str, node: &store::NodeRow) {
    let now = compute::now();
    if node.is_tenant() {
        let failed = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::replace_node_servers(&conn, node_id, &[], now);
            admin::forget_health(&conn, node_id);
            let open = admin::list_requests(&conn, &admin::ReqFilter { node_id: Some(node_id), state: Some("open"), limit: 500, ..Default::default() });
            for r in &open {
                let _ = admin::set_result(&conn, &r.id, "failed", &json!({"code": pshare::codes::NOT_NODE_OWNER, "message": "the node now belongs to another server"}));
            }
            store::record_event(
                &conn,
                "node",
                "role_change",
                Some(node_id),
                None,
                None,
                json!({"from": from, "to": "tenant", "owner": node.owner_label, "failed": open.iter().map(|r| r.id.clone()).collect::<Vec<_>>()}),
            );
            open
        };
        let owner = node.owner_label.clone().unwrap_or_else(|| "another server".into());
        for r in failed {
            notify_proposer(&r, &format!("[k2 compute] {} failed: {} now belongs to {owner} (not_node_owner): {}", r.id, node.name, r.summary));
        }
        SERVERS_META.lock().unwrap_or_else(|p| p.into_inner()).as_mut().map(|m| m.remove(node_id));
        share_note(node_id, &format!("role:tenant:{now}"), &format!("{} now belongs to {owner}; this server is a tenant there (its own share only).", node.name));
    } else {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            admin::forget_health(&conn, node_id);
            store::record_event(&conn, "node", "role_change", Some(node_id), None, None, json!({"from": from, "to": "owner"}));
        }
        KEYS_SENT.lock().unwrap_or_else(|p| p.into_inner()).as_mut().map(|m| m.remove(node_id));
        refresh_servers_soon(node_id);
        share_note(node_id, &format!("role:owner:{now}"), &format!("{} is now this server's: its admin, updates, servers and approvals are here.", node.name));
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "role_change", "node": node.name}));
}

/// The node says this server owns it now while this server still has it as
/// a tenant ([`store::NodeRow::owner_claim`]; MT36). Never taken at the
/// node's word (a node another person owns could say anything to be sent
/// admin or secrets here): it stays a tenant, and the person is told how
/// to take it over in person ([`take_owner`]).
pub fn on_owner_claim(node: &store::NodeRow) {
    let owner = node.owner_label.clone().unwrap_or_else(|| "another server".into());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::record_event(&conn, "node", "owner_claim", Some(&node.id), None, None, json!({"owner": node.owner_label}));
    }
    share_note(
        &node.id,
        &format!("owner-claim:{}", compute::now()),
        &format!(
            "{} (key {}) says this server is its owner now (an owner change from {owner}). It stays {owner}'s here: no admin, no secrets. \
             Only the node says so: take it over only if you changed its owner yourself and that key is the node you know. Taking it over \
             makes this server its admin (root admin on that machine), and every compute:<workspace>/* secret grant starts sending its \
             secrets to it: {}. To take it over in person: k2 compute node take-owner {} (it shows all this again, asks you to type \
             the node's name, then Touch ID or your vault password). If you didn't change its owner, leave it: nothing changes.",
            node.name,
            node.fingerprint,
            crate::compute_secrets::star_grants_text(&star_grants()),
            node.name
        ),
    );
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "owner_claim", "node": node.name}));
}

/// Every `compute:<ws>/*` grant ([`crate::compute_secrets::star_grants`]),
/// through this module's vault seam.
fn star_grants() -> Option<Vec<(String, String)>> {
    vault().ok().and_then(|v| crate::compute_secrets::star_grants_in(&v))
}

/// The confirm subject for taking over `node_id` (never a request id).
pub fn take_owner_subject(node_id: &str) -> String {
    format!("take-owner:{node_id}")
}

/// `POST /cli/compute/nodes/take-owner {node, confirmToken}` (MT36): the
/// one way a node this server has as a tenant becomes its own after its
/// enrolment. Both halves of an owner change must be there:
///
/// - the node confirmed it: its live offer says this server is its owner
///   (the node shows that only from its new owner pin, after root's
///   approved swap: F3);
/// - the person here approved it: the server's owner, in person (a fresh
///   vault confirm for this node; never an agent, a passport or an admin).
///
/// Then the row is owned (owner label cleared) and the role change runs
/// (the approval key is pinned at once, the server list mirrored).
fn take_owner(who: &Who, body: &[u8]) -> R<Value> {
    if who.passport().is_some() || !who.is_owner() {
        crate::compute_routes::need_credential(who)?;
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "only the server's owner takes over a node, in person: k2 compute node take-owner <node> from a terminal outside K2. An agent never can.",
        ));
    }
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    if node.state != "active" {
        return Err(no(codes::NODE_NOT_FOUND, 404, format!("{} isn't an active node", node.name)));
    }
    if !node.is_tenant() {
        return Err(no(acodes::BAD_ARGS, 409, format!("{} is this server's already", node.name)));
    }
    let owner = node.owner_label.clone().unwrap_or_else(|| "its owner".into());
    if !node.owner_claim() {
        return Err(no(
            NO_OWNER_CLAIM,
            409,
            format!(
                "{} doesn't say this server owns it: {owner} changes its owner first (k2 compute node servers {} owner <this server>, approved there); then {} shows the swap done and you can take it over here",
                node.name, node.name, node.name
            ),
        ));
    }
    if !online(&node.id) {
        return Err(no(codes::NODE_OFFLINE, 409, format!("{} isn't connected now: take it over while its session is live", node.name)));
    }
    // What the person confirms (verification M1): the node's word is only
    // a claim, so the confirm is the guard; the CLI shows this first.
    if crate::compute_routes::b(&v, "preview") == Some(true) {
        let grants = star_grants();
        return Ok(json!({
            "preview": true,
            "node": node.name,
            "fingerprint": node.fingerprint,
            "from": node.owner_label,
            "becomesAdmin": true,
            "grantsKnown": grants.is_some(),
            "secretGrants": grants.as_ref().map(|g| g.iter().map(|(ws, s)| json!({"workspace": ws, "secret": s})).collect::<Vec<_>>()),
            "secretGrantsText": crate::compute_secrets::star_grants_text(&grants),
        }));
    }
    let Some(token) = s(&v, "confirmToken") else {
        return Err(no(acodes::VAULT_CONFIRM_REQUIRED, 403, "taking over a node needs a fresh owner confirm (Touch ID, your login or vault password): k2 compute node take-owner <node> asks for it"));
    };
    let stamp = confirm_stamp(who).ok_or_else(owner_only)?;
    let vault = vault()?;
    let proof = vault
        .confirm
        .take(token, admin::CONFIRM_ACTION, Some(&take_owner_subject(&node.id)), &stamp)
        .map_err(|_| no(acodes::VAULT_CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for this node, within 60 seconds"))?;
    let via = match proof {
        k2_vault::HumanProof::Confirmed { method } => method,
        k2_vault::HumanProof::None => return Err(no(acodes::VAULT_CONFIRM_REQUIRED, 403, "no owner confirm")),
    };
    let after = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // Both halves again, under the lock the write takes: the row is
        // still an active tenant whose node says this server owns it.
        let now = store::node_by_id(&conn, &node.id).filter(|n| n.state == "active" && n.owner_claim());
        if now.is_none() {
            return Err(no(acodes::BAD_ARGS, 409, format!("{} changed while you confirmed; look again (k2 compute nodes)", node.name)));
        }
        store::mark_node_owned(&conn, &node.id).map_err(|e| no("store", 500, e))?;
        store::record_event(&conn, &who.label(), "owner_taken", Some(&node.id), None, None, json!({"from": node.owner_label, "via": via}));
        store::node_by_id(&conn, &node.id)
    }
    .ok_or_else(|| no(codes::NODE_NOT_FOUND, 404, "the node was removed"))?;
    on_role_change(&node.id, "tenant", &after);
    Ok(json!({"ok": true, "node": node.name, "role": "owner", "from": node.owner_label, "via": via}))
}

/// [`take_owner`] on a node that doesn't say this server owns it.
pub const NO_OWNER_CLAIM: &str = "no_owner_claim";

/// An operator ([`operator`]) or a local agent under "Allow managing
/// compute nodes": may propose an add or a widening, pause, resume and
/// narrow (§3).
fn servers_operator(who: &Who, mgr: Option<&Manager>) -> R<Operator> {
    if let Some(m) = mgr {
        return Ok(Operator { label: m.label.clone(), workspace: Some(m.project_id.clone()), session: Some(m.session_id.clone()) });
    }
    operator(who)
}

/// The node takes servers (`servers` feature, 0.46.3+), and is ours.
fn servers_ready(node: &store::NodeRow) -> R<()> {
    node_admin_ready(node, wire::FEATURE_ADMIN)?;
    if !features(&node.id).iter().any(|f| f == pshare::FEATURE_SERVERS) {
        return Err(no(
            pshare::codes::NODE_TOO_OLD,
            409,
            format!("{} runs a k2-node older than 0.46.3 and can't take a second server: update {} first (k2 compute node update {})", node.name, node.name, node.name),
        ));
    }
    Ok(())
}

static SERVERS_REFRESHING: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

/// Each node's last `servers.list` facts (`max_servers`, `solo`), from
/// whichever refresh landed last: the route's own, or the ticker's that
/// raced it (macOS r8: the ticker's answer landed and the route's timed
/// out, so the reply had no `maxServers`/`solo` though its rows were new).
static SERVERS_META: Mutex<Option<HashMap<String, (u8, bool)>>> = Mutex::new(None);

fn servers_meta(node_id: &str) -> Option<(u8, bool)> {
    SERVERS_META.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(node_id).copied())
}

/// Mirror the node's server list now (blocking, up to [`ANSWER_WAIT`]).
/// Owner side only. Audits and notes the changes it sees.
fn refresh_servers(node: &store::NodeRow) -> R<pshare::ServersList> {
    servers_ready(node)?;
    let (_, r) = send_direct(&Who::Owner, node, Class::Read, pshare::VERB_LIST, &json!({}), Some(ANSWER_WAIT))?;
    let Some(r) = r else {
        return Err(no("node_timeout", 504, format!("{} didn't answer servers.list in time; try again", node.name)));
    };
    if r.state != "done" {
        return Err(no("node_refused", 409, format!("{} refused servers.list: {}", node.name, r.message.as_deref().or(r.code.as_deref()).unwrap_or("?"))));
    }
    let (meta, rows) = share::parse_servers_list(&r.result).map_err(|e| no("node_answer", 502, e))?;
    let now = compute::now();
    let notes = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let before = store::node_servers(&conn, &node.id);
        store::replace_node_servers(&conn, &node.id, &rows, now).map_err(|e| no("store", 500, e))?;
        server_changes(&conn, node, &before, &rows)
    };
    for (key, text) in notes {
        share_note(&node.id, &key, &text);
    }
    SERVERS_META.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(node.id.clone(), (meta.max_servers, meta.solo));
    Ok(meta)
}

/// Audit lines and notes for what changed between two server lists.
fn server_changes(conn: &rusqlite::Connection, node: &store::NodeRow, before: &[share::ServerRow], after: &[share::ServerRow]) -> Vec<(String, String)> {
    let mut notes = Vec::new();
    for a in after.iter().filter(|a| a.slot != pshare::OWNER_SLOT) {
        let was = before.iter().find(|b| b.slot == a.slot && b.label == a.label).map(|b| b.state.as_str());
        if was == Some(a.state.as_str()) {
            continue;
        }
        let kind = match a.state.as_str() {
            "active" if was == Some("pending") => "server_confirm",
            "active" | "pending" if was.is_none() => "server_add",
            "expired" => "share_expired",
            "revoked" => "server_revoke",
            "paused" | "active" => "server_pause",
            _ => "server_state",
        };
        store::record_event(conn, "node", kind, Some(&node.id), None, None, json!({"server": a.label, "slot": a.slot, "from": was, "to": a.state}));
        if a.state == "active" && matches!(was, None | Some("pending")) {
            let until = a.expires_at.map(|e| format!(" until {}", clock_day(e))).unwrap_or_default();
            notes.push((format!("shared:{}:{}", a.slot, a.label), format!("{} is now shared with {}{until}.", node.name, a.label)));
        }
        if a.state == "expired" {
            notes.push((format!("expired:{}:{}", a.slot, a.label), format!("{}: {}'s share has ended.", node.name, a.label)));
        }
        if a.state == "refused" {
            let why = a.usage.as_ref().and_then(|u| u.get("refusal")).and_then(Value::as_str).unwrap_or("refused");
            notes.push((format!("refused:{}:{}", a.slot, a.label), format!("{}: {} was approved but its attach is refused ({why}); it must update to 0.46.3.", node.name, a.label)));
        }
    }
    notes
}

fn clock_day(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0).map(|d| d.format("%b %-d").to_string()).unwrap_or_default()
}

/// Refresh a node's server list on a worker thread (at most one at a time
/// per node). Used after a `servers.*` change and by the ticker.
pub fn refresh_servers_soon(node_id: &str) {
    {
        let mut g = SERVERS_REFRESHING.lock().unwrap_or_else(|p| p.into_inner());
        if !g.get_or_insert_with(Default::default).insert(node_id.to_string()) {
            return;
        }
    }
    let id = node_id.to_string();
    std::thread::spawn(move || {
        let node = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::node_by_id(&conn, &id)
        };
        if let Some(n) = node {
            if let Err(e) = refresh_servers(&n) {
                log_debug!("[compute] servers.list on {}: {} {}", n.name, e.code, e.hint);
            }
        }
        if let Some(s) = SERVERS_REFRESHING.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            s.remove(&id);
        }
    });
}

/// The compute ticker (headless, §12): mirror each owned node's server
/// list every `servers_refresh_secs`, and note shares ending in 24 h or
/// ended (owner side).
pub fn servers_tick(now: i64) {
    let t = compute::retry::tuning();
    let (nodes, mirrors) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let nodes: Vec<store::NodeRow> = store::list_nodes(&conn, false).into_iter().filter(|n| n.state == "active" && !n.is_tenant()).collect();
        let mirrors: Vec<(String, Option<i64>, Vec<share::ServerRow>)> =
            nodes.iter().map(|n| (n.id.clone(), store::node_servers_seen_at(&conn, &n.id), store::node_servers(&conn, &n.id))).collect();
        (nodes, mirrors)
    };
    for (n, (_, seen, rows)) in nodes.iter().zip(mirrors) {
        let able = online(&n.id) && features(&n.id).iter().any(|f| f == pshare::FEATURE_SERVERS) && features(&n.id).iter().any(|f| f == wire::FEATURE_ADMIN);
        // Solo nodes stay as they were: nothing to mirror until a second
        // server is on the list, the node says it's shared, or an add is open.
        let adding = || {
            let db = k2_core::db::shared();
            let conn = db.lock();
            admin::list_requests(&conn, &admin::ReqFilter { node_id: Some(&n.id), state: Some("open"), limit: 50, ..Default::default() })
                .iter()
                .any(|r| r.steps().is_ok_and(|s| s.iter().any(|st| st.verb.starts_with("servers."))))
        };
        let shared = n.shared || rows.iter().any(|r| r.slot != pshare::OWNER_SLOT);
        if able && seen.is_none_or(|s| now - s >= t.servers_refresh_secs) && (shared || adding()) {
            refresh_servers_soon(&n.id);
        }
        for r in rows.iter().filter(|r| r.slot != pshare::OWNER_SLOT && r.state == "active") {
            let Some(ends) = r.expires_at else { continue };
            if ends > now && ends - now <= share::EXPIRY_NOTE_SECS {
                share_note(&n.id, &format!("expiring:{}:{}:{ends}", r.slot, r.label), &format!("{}: {}'s share ends {} (in under 24 h).", n.name, r.label, clock_day(ends)));
            }
        }
    }
}

/// A sharing note to the "alerts go to" workspace, once per `key` per
/// node (audited as `share_note`). Both servers get one for an add, an
/// end date coming up, an end, a pause or a revoke (§4.1, §4.3).
pub fn share_note(node_id: &str, key: &str, text: &str) {
    let to = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let seen: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM compute_events WHERE kind = 'share_note' AND node_id = ?1 AND json_extract(detail_json, '$.key') = ?2",
                rusqlite::params![node_id, key],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if seen {
            return;
        }
        let to = admin::setting(&conn, admin::ALERTS_TO);
        store::record_event(&conn, "controller", "share_note", Some(node_id), to.as_deref(), None, json!({"key": key, "text": text}));
        to
    };
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "share", "text": text}));
    if let Some(ws) = to {
        deliver(Some(&ws), None, "compute node sharing", &format!("[k2 compute] {text}"));
    }
}

/// The share as JSON for the CLI (`servers` list rows, `nodes`).
fn server_json(r: &share::ServerRow, now: i64) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    v["role"] = if r.slot == pshare::OWNER_SLOT { "owner" } else { "tenant" }.into();
    // Flat for scripts (`--json`): the weight, the pending SAS, a refusal.
    v["weight"] = r.limits.weight.into();
    if let Some(e) = r.expires_at {
        v["expiresInSecs"] = (e - now).into();
    }
    v["sas"] = r
        .usage
        .as_ref()
        .and_then(|u| u.get("sas"))
        .and_then(Value::as_str)
        .filter(|_| r.state == "pending")
        .map(k2_core::compute::proto::pairing::display_sas)
        .into();
    v["refusal"] = r.usage.as_ref().and_then(|u| u.get("refusal")).cloned().filter(|_| r.state == "refused").unwrap_or(Value::Null);
    // Job tiers (T7): does its Priority count here? null = the node is
    // older than 0.46.5 and doesn't say.
    v["priority"] = server_priority(r).into();
    v
}

/// A mirrored server's Priority word (`servers.list`, k2-node 0.46.5+).
fn server_priority(r: &share::ServerRow) -> Option<bool> {
    r.usage.as_ref().and_then(|u| u.get("priority")).and_then(Value::as_bool)
}

/// `GET /cli/compute/nodes/servers?node=N[&refresh=1]`: on a node this
/// server owns, every server (mirrored from `servers.list`, refreshed when
/// stale); on a node another server owns, this server's own share.
fn servers_list(who: &Who, params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<Value> {
    let node = node_or_404(params.get("node").map(String::as_str))?;
    if mgr.is_none() {
        reader(who, &node, false)?;
    }
    let now = compute::now();
    if node.is_tenant() {
        return Ok(json!({
            "node": node.name,
            "role": "tenant",
            "owner": node.owner_label,
            "shared": node.shared,
            "share": node.share,
            "line": node.share.as_ref().map(share::share_line),
        }));
    }
    let want_refresh = params.get("refresh").is_some_and(|x| x == "1" || x == "true");
    let mut note = None;
    let mut meta: Option<pshare::ServersList> = None;
    let stale = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_servers_seen_at(&conn, &node.id).is_none_or(|s| now - s >= compute::retry::tuning().servers_refresh_secs)
    };
    if want_refresh || stale {
        match refresh_servers(&node) {
            Ok(m) => meta = Some(m),
            // A node without servers (solo, older): just the owner.
            Err(e) if e.code == pshare::codes::NODE_TOO_OLD => note = Some(e.hint),
            Err(e) => note = Some(format!("{} (showing the last list)", e.hint)),
        }
    }
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_servers(&conn, &node.id)
    };
    let list: Vec<Value> = if rows.is_empty() {
        // Solo: the owner server alone (§4.4).
        vec![json!({"slot": 0, "label": crate::compute_ws::controller_label(), "state": "active", "trust": "own", "role": "owner", "solo": true})]
    } else {
        rows.iter().map(|r| server_json(r, now)).collect()
    };
    let pending = admin::list_requests(
        &k2_core::db::shared().lock(),
        &admin::ReqFilter { node_id: Some(&node.id), state: Some("open"), limit: 50, ..Default::default() },
    )
    .into_iter()
    .filter(|r| r.steps().is_ok_and(|s| s.iter().any(|st| st.verb.starts_with("servers."))))
    .map(|r| request_json(&r, Some(&node.name)))
    .collect::<Vec<_>>();
    // From the node when it was just asked, else from the last mirror
    // (this daemon's run), like the rows.
    let (max_servers, solo) = match (&meta, servers_meta(&node.id)) {
        (Some(m), _) => (Some(m.max_servers), Some(m.solo)),
        (None, Some((m, s))) => (Some(m), Some(s)),
        (None, None) => (None, None),
    };
    // Who may do the owner-only acts here (revoke, change the owner; Q9):
    // the server's owner in person, never an Admin login or an agent.
    let owner_acts = who.is_owner() && who.passport().is_none() && mgr.is_none();
    Ok(json!({"node": node.name, "role": "owner", "shared": node.shared, "maxServers": max_servers, "solo": solo,
        "servers": list, "requests": pending, "note": note, "ownerActs": owner_acts}))
}

fn limits_from(v: &Value) -> R<pshare::ShareOverrides> {
    let set = v.get("set").or_else(|| v.get("limits")).cloned().unwrap_or_else(|| json!({}));
    if !set.is_object() {
        return Err(no(acodes::BAD_ARGS, 400, "limits are key=value (weight, max_running, max_queued, cpu_hours_day, max_job_secs, max_cpus, max_mem_gb, disk_gb, network, top_class, gate, exclusive)"));
    }
    serde_json::from_value(set).map_err(|e| no(acodes::BAD_ARGS, 400, format!("limits: {e}")))
}

/// `POST /cli/compute/nodes/servers/add {node, url, code: "CODE.FP",
/// trust, forSecs, set{…}, reason}`: always held for the owner's approval
/// (Touch ID or the vault password), then sent as `servers.add`.
fn servers_add(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<Value> {
    let op = servers_operator(who, mgr)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    servers_ready(&node)?;
    // V59: any second server needs a walled node.
    if !features(&node.id).iter().any(|f| f == frames::FEATURE_JOB_WALL) {
        return Err(no(
            pshare::codes::TENANT_NEEDS_JOB_WALL,
            409,
            format!("{} has no job wall: turn it on first (k2 compute node admin propose {} wall.enable), then add the server", node.name, node.name),
        ));
    }
    let trust = match s(&v, "trust").unwrap_or("own") {
        "own" => pshare::Trust::Own,
        "team" => pshare::Trust::Team,
        _ => return Err(no(acodes::BAD_ARGS, 400, "--trust is own (your own servers) or team (other people's)")),
    };
    let macos = matches!(node.os.as_deref(), Some("macos") | Some("darwin"));
    if trust == pshare::Trust::Team && (macos || !features(&node.id).iter().any(|f| f == pshare::FEATURE_TENANT_WALL)) {
        return Err(no(
            pshare::codes::TEAM_NEEDS_TENANT_WALL,
            409,
            format!(
                "{} can't take other people's servers yet: {}. Add your own servers with --trust own",
                node.name,
                if macos { "Macs take them in 0.46.4 (a user per server)" } else { "its tenant wall isn't on" }
            ),
        ));
    }
    let cols = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::admin_cols(&conn, &node.id)
    };
    if cols.remote_admin.as_deref().is_some_and(|l| l != "full") {
        return Err(no(
            acodes::REMOTE_ADMIN_OFF,
            409,
            format!(
                "adding a server needs remote admin `full` on {} (it's {}): at the machine run sudo k2-node servers add …, or raise the level there",
                node.name,
                cols.remote_admin.as_deref().unwrap_or("?")
            ),
        ));
    }
    let url = s(&v, "url").ok_or_else(|| no(acodes::BAD_ARGS, 400, "--url is the new server's address (https://baden.k2.dev)"))?;
    let code = s(&v, "code").ok_or_else(|| no(acodes::BAD_ARGS, 400, "--code is the CODE.FP the new server's `k2 compute node add` printed"))?;
    let (binding, fp_prefix) = share::split_add_code(code).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
    let for_secs = match crate::compute_routes::n(&v, "forSecs") {
        Some(0) => None,
        Some(x) => Some(x),
        // §7 defaults: 7 days for other people's servers, none for your own.
        None if trust == pshare::Trust::Team => Some(7 * 86_400),
        None => None,
    };
    let args = serde_json::to_value(pshare::AddArgs { url: url.to_string(), code_binding: binding, fp_prefix, trust, for_secs, limits: limits_from(&v)? })
        .map_err(|e| no("internal", 500, e.to_string()))?;
    pshare::check_verb_args(pshare::VERB_ADD, &args).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
    // One pending server at a time (§4.1).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let open = admin::list_requests(&conn, &admin::ReqFilter { node_id: Some(&node.id), state: Some("open"), limit: 100, ..Default::default() })
            .into_iter()
            .any(|r| r.steps().is_ok_and(|s| s.iter().any(|st| st.verb == pshare::VERB_ADD)));
        let pending = store::node_servers(&conn, &node.id).iter().any(|r| r.state == "pending");
        if open || pending {
            return Err(no(
                pshare::codes::SERVER_ADD_PENDING,
                409,
                format!("{} already has a server being added: confirm its code, or wait for its 10 minutes to pass (k2 compute node servers {})", node.name, node.name),
            ));
        }
    }
    let summary = admin::summarize(&node.name, pshare::VERB_ADD, &args);
    let body_v = json!({"verb": pshare::VERB_ADD, "args": args});
    let out = file_request(&op, &node, "command", Class::Privileged, &body_v, &summary, s(&v, "reason"))?;
    crate::compute_routes::audit(who, "server_add", Some(&node.id), op.workspace.as_deref(), None, json!({"url": url, "trust": trust.as_str(), "request": out["request"]["id"]}));
    let mut out = out;
    if trust == pshare::Trust::Team {
        out["warning"] = format!(
            "Jobs from {url} get their own user and folders on {}. They can't read your files or other servers' jobs. They do share the machine's kernel, CPU and disk, and they can reach the internet and any service running on this machine's loopback. Only add people you trust.",
            node.name
        )
        .into();
    }
    Ok(out)
}

/// The mirrored share of `server` on `node` (a label or `slot:<n>`),
/// refreshed when stale.
fn server_row(node: &store::NodeRow, server: &str) -> R<share::ServerRow> {
    let stale = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_servers_seen_at(&conn, &node.id).is_none_or(|t| compute::now() - t >= compute::retry::tuning().servers_refresh_secs)
    };
    if stale {
        refresh_servers(node)?;
    }
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_servers(&conn, &node.id)
    };
    let slot = server.strip_prefix("slot:").and_then(|n| n.parse::<u8>().ok());
    rows.into_iter()
        .find(|r| slot.map_or(r.label == server && r.state != "revoked", |n| r.slot == n))
        .ok_or_else(|| no(codes::NODE_NOT_FOUND, 404, format!("no server '{server}' on {} (k2 compute node servers {})", node.name, node.name)))
}

/// The `server` argument. `owner_ok`: `limits` may name this server
/// (slot 0, the owner) for its own weight; pause and revoke never.
fn server_arg(v: &Value, owner_ok: bool) -> R<String> {
    let server = s(v, "server").ok_or_else(|| no(acodes::BAD_ARGS, 400, "name the server (its label, or slot:<n>)"))?;
    if server == "slot:0" && !owner_ok {
        return Err(no(acodes::BAD_ARGS, 400, "slot 0 is this server, the node's owner"));
    }
    Ok(server.to_string())
}

/// Send an operate `servers.*` verb now and read the node's answer.
fn servers_operate(who: &Who, node: &store::NodeRow, verb: &str, args: &Value) -> R<(String, Option<wire::AdminResult>)> {
    pshare::check_verb_args(verb, args).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
    let r = send_direct(who, node, Class::Operate, verb, args, Some(ANSWER_WAIT))?;
    if let Some(res) = r.1.as_ref().filter(|x| x.state != "done" && x.code.as_deref() != Some(wire::code::APPROVAL_REQUIRED)) {
        return Err(no(
            "node_refused",
            409,
            format!("{} refused {verb}: {}", node.name, res.message.as_deref().or(res.code.as_deref()).unwrap_or("?")),
        ));
    }
    refresh_servers_soon(&node.id);
    Ok(r)
}

/// `POST /cli/compute/nodes/servers/limits {node, server, set{…},
/// forSecs, priority}`: a narrowing applies at once; a widening is held for
/// the owner's approval (§4.3). `priority` (job tiers, T8) goes only to a
/// node offering `tiers`: an older one would refuse the whole change.
fn servers_limits(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<Value> {
    let op = servers_operator(who, mgr)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    servers_ready(&node)?;
    let server = server_arg(&v, true)?;
    let priority = match v.get("priority") {
        None | Some(Value::Null) => None,
        Some(x) => Some(x.as_bool().ok_or_else(|| no(acodes::BAD_ARGS, 400, "priority is true or false"))?),
    };
    if priority.is_some() && !features(&node.id).iter().any(|f| f == k2_core::compute::proto::tiers::FEATURE_TIERS) {
        return Err(no(
            pshare::codes::NODE_TOO_OLD,
            409,
            format!("{} runs a k2-node older than 0.46.5: it has no Priority setting per server yet (k2 compute node update {})", node.name, node.name),
        ));
    }
    let largs = pshare::LimitsArgs { server: server.clone(), set: limits_from(&v)?, for_secs: crate::compute_routes::n(&v, "forSecs"), priority };
    let args = serde_json::to_value(&largs).map_err(|e| no("internal", 500, e.to_string()))?;
    pshare::check_verb_args(pshare::VERB_LIMITS, &args).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
    let row = server_row(&node, &server)?;
    let summary = admin::summarize(&node.name, pshare::VERB_LIMITS, &args);
    let hold = |why: &str| {
        let body_v = json!({"verb": pshare::VERB_LIMITS, "args": args});
        let reason = s(&v, "reason").map(str::to_string).or_else(|| (!why.is_empty()).then(|| why.to_string()));
        file_request(&op, &node, "command", Class::Privileged, &body_v, &summary, reason.as_deref())
    };
    match admin::classify_servers_limits(&row.limits, row.expires_at, server_priority(&row), &largs, compute::now()) {
        Class::Read => Ok(json!({"ok": true, "node": node.name, "server": row.label, "changed": false, "note": "nothing to change"})),
        Class::Operate => {
            let (cmd_id, result) = servers_operate(who, &node, pshare::VERB_LIMITS, &args)?;
            crate::compute_routes::audit(who, "server_limits", Some(&node.id), op.workspace.as_deref(), None, json!({"server": row.label, "args": args, "class": "operate", "cmdId": cmd_id}));
            if result.as_ref().is_some_and(|r| r.code.as_deref() == Some(wire::code::APPROVAL_REQUIRED)) {
                return hold("the node counts this as a widening");
            }
            Ok(json!({"ok": true, "node": node.name, "server": row.label, "changed": true, "class": "operate", "cmdId": cmd_id, "note": "narrowed at once"}))
        }
        _ => {
            crate::compute_routes::audit(who, "server_limits", Some(&node.id), op.workspace.as_deref(), None, json!({"server": row.label, "args": args, "class": "privileged"}));
            hold("")
        }
    }
}

/// `POST /cli/compute/nodes/servers/pause {node, server, action:
/// pause|resume}`: at once (operate).
fn servers_pause(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<Value> {
    let op = servers_operator(who, mgr)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    servers_ready(&node)?;
    let server = server_arg(&v, false)?;
    let verb = match s(&v, "action").unwrap_or("pause") {
        "pause" => pshare::VERB_PAUSE,
        "resume" => pshare::VERB_RESUME,
        _ => return Err(no(acodes::BAD_ARGS, 400, "action is pause or resume")),
    };
    let args = json!({"server": server});
    let (cmd_id, _) = servers_operate(who, &node, verb, &args)?;
    crate::compute_routes::audit(who, "server_pause", Some(&node.id), op.workspace.as_deref(), None, json!({"server": server, "verb": verb, "cmdId": cmd_id}));
    let note = if verb == pshare::VERB_PAUSE { "its queued jobs wait; running ones finish; it can still queue" } else { "its queued jobs may start again" };
    Ok(json!({"ok": true, "node": node.name, "server": server, "action": verb, "cmdId": cmd_id, "note": note}))
}

/// `POST /cli/compute/nodes/servers/revoke {node, server, now, wipe}`:
/// the owner only (never an agent, §3); at once.
fn servers_revoke(who: &Who, body: &[u8]) -> R<Value> {
    if who.passport().is_some() || !who.is_owner() {
        crate::compute_routes::need_credential(who)?;
        return Err(owner_only());
    }
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    servers_ready(&node)?;
    let server = server_arg(&v, false)?;
    let now = crate::compute_routes::b(&v, "now") == Some(true);
    let wipe = crate::compute_routes::b(&v, "wipe") == Some(true);
    let args = serde_json::to_value(pshare::RevokeArgs { server: server.clone(), now, wipe }).map_err(|e| no("internal", 500, e.to_string()))?;
    let (cmd_id, _) = servers_operate(who, &node, pshare::VERB_REVOKE, &args)?;
    crate::compute_routes::audit(who, "server_revoke", Some(&node.id), None, None, json!({"server": server, "now": now, "wipe": wipe, "cmdId": cmd_id}));
    share_note(&node.id, &format!("revoked:{server}:{}", compute::now()), &format!("{}: {server} was revoked; its queued jobs are cancelled{}.", node.name, if now { " and its running jobs stopped" } else { ", running ones finish" }));
    Ok(json!({
        "ok": true,
        "node": node.name,
        "server": server,
        "cmdId": cmd_id,
        "note": format!("queued jobs cancelled; running jobs {}; its files are wiped {}", if now { "stopped" } else { "finish" }, if wipe { "now" } else { "in 24 h" }),
    }))
}

/// `POST /cli/compute/nodes/servers/owner {node, server, old: keep|revoke,
/// caches: move|wipe, now, reason}` (prd-macos-tenant-compute-v1 §6): make
/// another of this owner's own servers on the node its owner. The owner
/// only (Q9: never an agent, not even a proposal), always held for the
/// owner's approval (Settings confirm: Touch ID or the vault password),
/// then sent as `servers.owner`, naming the target's full key so the node
/// can't slip another in. The node drains first, swaps the two servers'
/// slots and re-pins root's `admin.toml`; this server stays on the node as
/// one of the owner's servers (`old = keep`) or leaves (`revoke`).
fn servers_owner(who: &Who, body: &[u8]) -> R<Value> {
    if who.passport().is_some() || !who.is_owner() {
        crate::compute_routes::need_credential(who)?;
        return Err(no(codes::OWNER_ONLY, 403, "only the server's owner changes a node's owner server (Settings → Compute, or k2 compute node servers <node> owner <server> from a terminal outside K2)"));
    }
    let op = operator(who)?;
    let v = json_body(body)?;
    let node = node_or_404(s(&v, "node"))?;
    servers_ready(&node)?;
    if !features(&node.id).iter().any(|f| f == pshare::FEATURE_SERVERS_OWNER) {
        return Err(no(
            pshare::codes::NODE_TOO_OLD,
            409,
            format!("{} runs a k2-node older than 0.46.4, which can't change its owner remotely: update it (k2 compute node update {}), or at the machine run sudo k2-node servers owner <server>", node.name, node.name),
        ));
    }
    let cols = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        admin::admin_cols(&conn, &node.id)
    };
    if cols.remote_admin.as_deref().is_some_and(|l| l != "full") {
        return Err(no(
            acodes::REMOTE_ADMIN_OFF,
            409,
            format!(
                "changing the owner needs remote admin `full` on {} (it's {}): at the machine run sudo k2-node servers owner <server>, or raise the level there",
                node.name,
                cols.remote_admin.as_deref().unwrap_or("?")
            ),
        ));
    }
    let server = server_arg(&v, false)?;
    let old = match s(&v, "old") {
        None => pshare::OldOwner::Keep,
        Some(x) => pshare::OldOwner::parse(x).ok_or_else(|| no(acodes::BAD_ARGS, 400, "old is keep (this server stays on the node) or revoke (it leaves)"))?,
    };
    let caches = match s(&v, "caches") {
        None => pshare::Caches::Move,
        Some(x) => pshare::Caches::parse(x).ok_or_else(|| no(acodes::BAD_ARGS, 400, "caches is move (each server keeps its warm caches) or wipe"))?,
    };
    let now = crate::compute_routes::b(&v, "now") == Some(true);
    let row = server_row(&node, &server)?;
    if row.slot == pshare::OWNER_SLOT {
        return Err(no(acodes::BAD_ARGS, 400, "slot 0 is this server, the node's owner already"));
    }
    if row.trust != pshare::Trust::Own.as_str() {
        return Err(no(
            pshare::codes::OWNER_NEEDS_OWN_SERVER,
            409,
            format!("{} is another person's server (trust team): only one of your own servers can own {}, since the owner gets root admin there", row.label, node.name),
        ));
    }
    if row.state != "active" {
        return Err(no(pshare::codes::OWNER_TARGET_NOT_READY, 409, format!("{} is {} on {}; only an active server can take over", row.label, row.state, node.name)));
    }
    if row.usage.as_ref().and_then(|u| u.get("online")).and_then(Value::as_bool) != Some(true) {
        return Err(no(pshare::codes::OWNER_TARGET_NOT_READY, 409, format!("{} isn't connected to {} now; it must be online to take over", row.label, node.name)));
    }
    let fp = row.fp.clone().filter(|f| pshare::valid_fp(f)).ok_or_else(|| {
        no(pshare::codes::OWNER_TARGET_NOT_READY, 409, format!("{} hasn't finished enrolling on {} (no full key yet)", row.label, node.name))
    })?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let open = admin::list_requests(&conn, &admin::ReqFilter { node_id: Some(&node.id), state: Some("open"), limit: 100, ..Default::default() })
            .into_iter()
            .any(|r| r.steps().is_ok_and(|s| s.iter().any(|st| st.verb == pshare::VERB_OWNER)));
        if open {
            return Err(no("owner_change_pending", 409, format!("an owner change for {} is already waiting: approve or deny it in Settings → Compute → Approvals", node.name)));
        }
    }
    let args = serde_json::to_value(pshare::OwnerArgs { server: row.label.clone(), fp: Some(fp.clone()), old, caches, now }).map_err(|e| no("internal", 500, e.to_string()))?;
    pshare::check_verb_args(pshare::VERB_OWNER, &args).map_err(|e| no(acodes::BAD_ARGS, 400, e))?;
    let summary = admin::summarize(&node.name, pshare::VERB_OWNER, &args);
    let body_v = json!({"verb": pshare::VERB_OWNER, "args": args});
    let out = file_request(&op, &node, "command", Class::Privileged, &body_v, &summary, s(&v, "reason"))?;
    crate::compute_routes::audit(
        who,
        "server_owner",
        Some(&node.id),
        None,
        None,
        json!({"server": row.label, "fp": fp, "old": old.as_str(), "caches": caches.as_str(), "now": now, "request": out["request"]["id"]}),
    );
    let mut out = out;
    out["note"] = format!(
        "after you approve, {} drains, then {} becomes its owner{}; {} k2-node restarts once.",
        node.name,
        row.label,
        if old == pshare::OldOwner::Revoke { " and this server leaves it" } else { " and this server stays on it as one of your servers" },
        if caches == pshare::Caches::Wipe { "both servers' caches are wiped;" } else { "each server keeps its caches;" }
    )
    .into();
    Ok(out)
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

    thread_local! {
        static SERVER_VERSION: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
        static RELEASE_DIR: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
        static BUNDLED: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    /// The version this (test) server reports.
    pub fn set_server_version(v: Option<&str>) {
        SERVER_VERSION.with(|s| *s.borrow_mut() = v.map(str::to_string));
    }

    pub(super) fn server_version() -> Option<String> {
        SERVER_VERSION.with(|s| s.borrow().clone())
    }

    /// Where "the release" serves its assets from (tests never download).
    pub fn set_release_dir(p: Option<std::path::PathBuf>) {
        RELEASE_DIR.with(|r| *r.borrow_mut() = p);
    }

    pub(super) fn release_dir() -> Option<std::path::PathBuf> {
        RELEASE_DIR.with(|r| r.borrow().clone())
    }

    /// The k2-node "bundled" beside the daemon.
    pub fn set_bundled_node(p: Option<std::path::PathBuf>) {
        BUNDLED.with(|b| *b.borrow_mut() = p);
    }

    pub(super) fn bundled_node() -> Option<std::path::PathBuf> {
        BUNDLED.with(|b| b.borrow().clone())
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
