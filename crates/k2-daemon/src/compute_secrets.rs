//! Compute secret-env (prd-compute-secret-env-v1): a vault secret reaches a
//! compute job as an env var, sealed to the node at spawn time.
//!
//! The controller's side:
//! - `compute:` vault consumers as people write them
//!   (`compute:[<ws>/]<node|*>`) and as they're shown;
//! - submit (SE1–SE8): `secrets` in the run body → secret **ids** pinned in
//!   the job's controller-only `JobCtl`, env names in `Plan.secret_env`,
//!   grant coverage per node (`--any` skips nodes it doesn't cover);
//! - release (SE9–SE12): on `secret_need`, every check again, the values
//!   from the vault as `Requester::Compute` (one audit row each), sealed to
//!   the node's one-time key, an event either way;
//! - the belt (SE24): from release until the attempt ends, each log chunk
//!   is redacted here too; a hit means the node missed one.
//!
//! Values exist here only inside a release: resolved, sealed, put in the
//! belt's redactor, then dropped (`SecretBytes` zeroes on drop). Nothing of
//! a value is ever written to `k2.db`, the compute dir, an event or a log.
//!
//! Lane secrets (0.46.10, prd-ci-on-compute-for-clients-v1 §4.4, C3b): a
//! grant may name a lane (`compute:<ws>/lane:<owner>/<lane>`, stored as
//! `compute:<ws id>/@lane-<seat id>`, V26). A secret reaches a node of
//! another owner that has this server on one of its lanes only through
//! [`secret_gate`] (V25, `k2_core::compute::lane_secrets`): checked at
//! submit, at `--any` placement and again at every release, before a value
//! is resolved.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use k2_core::compute::lane_secrets::{self as lanes, SeatView};
use k2_core::compute::proto::frames::{Offer, SecretEnv, SecretNeed};
use k2_core::compute::proto::secret::{self as psecret, codes as scodes};
use k2_core::compute::store::{self, JobRow, NodeRow, SecretPin};
use k2_core::compute::{self as compute};
use k2_vault::redact::Redactor;
use k2_vault::{Actor, Consumer, Requester, Scope, ScopeKind, SecretStore, Selector, Use, VaultError};

use crate::compute_routes::{no, R};

// ── consumers (`k2 vault grant --to compute:…`) ──────────────────────

/// `compute:[<ws>/]<node|*>` → the vault consumer
/// `compute:<workspace id>/<node id or *>`; `compute:[<ws>/]lane:<owner>/<lane>`
/// → `compute:<workspace id>/@lane-<seat id>` (V26: this server's seat on
/// that owner's lane). `caller_ws` is the caller's own workspace, used when
/// no workspace is named.
pub fn consumer(spec: &str, caller_ws: Option<&str>) -> Result<Consumer, VaultError> {
    let bad = |m: &str| {
        VaultError::bad_request(
            "bad_consumer",
            format!("{m} (use compute:<workspace>/<node>, compute:<workspace>/* for every node this server owns, or compute:<workspace>/lane:<owner>/<lane> for your seat on another owner's lane)"),
        )
    };
    let (ws_tok, node_tok) = if spec.starts_with(LANE_TOKEN) {
        (None, spec)
    } else {
        match spec.split_once('/') {
            Some((w, n)) => (Some(w), n),
            None => (None, spec),
        }
    };
    let ws_id = match ws_tok {
        Some(w) if !w.is_empty() => crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?,
        Some(_) => return Err(bad("empty workspace")),
        None => caller_ws.map(str::to_string).ok_or_else(|| bad("name the workspace"))?,
    };
    if node_tok.is_empty() {
        return Err(bad("name a node or *"));
    }
    if let Some(l) = node_tok.strip_prefix(LANE_TOKEN) {
        // Lane grants are CI on compute's (0.46.10 ships it dark): no new
        // lane grant while its preview is off.
        if !compute::ci_enabled() {
            return Err(VaultError::new(compute::CI_PREVIEW_OFF, 409, compute::ci_preview_hint("A lane grant (compute:<workspace>/lane:<owner>/<lane>)")));
        }
        let Some((owner, lane)) = l.split_once('/').filter(|(o, l)| !o.is_empty() && !l.is_empty() && !l.contains('/')) else {
            return Err(bad("a lane is lane:<owner>/<lane>"));
        };
        let seat = lanes::seat_book()
            .seat_named(owner, lane)
            .ok_or_else(|| VaultError::not_found(format!("this server has no seat on {owner}'s lane {lane} (a seat is offered by the node owner and accepted here in person)")))?;
        let c = Consumer::compute_lane(&ws_id, &seat.seat_id);
        if c.compute_lane_parts().is_none() {
            return Err(bad("that workspace or seat can't be named here"));
        }
        return Ok(c);
    }
    let node_id = if node_tok == "*" {
        "*".to_string()
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::find_node(&conn, node_tok)
            .filter(|n| n.state != "revoked")
            .map(|n| n.id)
            .ok_or_else(|| VaultError::not_found(format!("no compute node '{node_tok}' (k2 compute nodes)")))?
    };
    let c = Consumer::compute(&ws_id, &node_id);
    if c.compute_parts().is_none() {
        return Err(bad("that workspace or node can't be named here"));
    }
    Ok(c)
}

/// The CLI's lane form: `compute:<ws>/lane:<owner>/<lane>`.
pub const LANE_TOKEN: &str = "lane:";

/// How people see a compute consumer: `compute:<ws handle>/<node name>`,
/// or `compute:<ws handle>/lane:<owner>/<lane>` for a lane grant.
pub fn label(c: &Consumer) -> String {
    if let Some((ws, seat_id)) = c.compute_lane_parts() {
        let lane = match lanes::seat_book().seat(seat_id) {
            Some(s) => format!("{LANE_TOKEN}{}/{}", s.owner_id, s.lane),
            None => format!("{LANE_TOKEN}? (seat {seat_id}, removed)"),
        };
        return format!("compute:{}/{lane}", crate::vault::workspace_label(ws));
    }
    let Some((ws, node)) = c.compute_parts() else { return c.label() };
    let node_name = if node == "*" {
        "*".to_string()
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, node).map(|n| n.name).unwrap_or_else(|| format!("{node} (removed)"))
    };
    format!("compute:{}/{node_name}", crate::vault::workspace_label(ws))
}

// ── submit ───────────────────────────────────────────────────────────

/// One `--secret NAME[=ENVNAME]` from the run body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub name: String,
    pub env: Option<String>,
}

/// `secrets: [{"name": "...", "env": "..."}]` (or `"NAME[=ENV]"` strings).
/// At most 16 (SE3: 400, exit 2).
pub(crate) fn asks_from(v: &serde_json::Value) -> R<Vec<Ask>> {
    let list = match v.get("secrets") {
        None | Some(serde_json::Value::Null) => return Ok(Vec::new()),
        Some(serde_json::Value::Array(a)) => a,
        Some(_) => return Err(no("secret_invalid", 400, "secrets is a list of {\"name\": NAME, \"env\": ENVNAME}")),
    };
    if list.len() > psecret::MAX_SECRETS {
        return Err(no(scodes::TOO_LARGE, 400, format!("a job gets at most {} secrets (--secret)", psecret::MAX_SECRETS)));
    }
    let mut out = Vec::new();
    for x in list {
        let (name, env) = match x {
            serde_json::Value::String(s) => match s.split_once('=') {
                Some((n, e)) => (n.to_string(), Some(e.to_string())),
                None => (s.clone(), None),
            },
            serde_json::Value::Object(_) => (
                x.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                x.get("env").and_then(|e| e.as_str()).filter(|e| !e.is_empty()).map(str::to_string),
            ),
            _ => return Err(no("secret_invalid", 400, "--secret NAME[=ENVNAME]")),
        };
        let ok = if k2_vault::refs::VaultRef::is_ref(&name) { k2_vault::refs::VaultRef::parse(&name).is_ok() } else { k2_vault::model::validate_name(&name, true).is_ok() };
        if !ok {
            return Err(no("secret_invalid", 400, format!("--secret {name}: a vault secret name (letters, digits, _ . -) or a k2v:// reference")));
        }
        out.push(Ask { name, env });
    }
    Ok(out)
}

/// A secret the job asked for, found in the vault (no value).
#[derive(Debug, Clone)]
pub struct Found {
    pub ask: Ask,
    pub secret_id: String,
    pub secret_name: String,
    /// The `--scope` a person names it by (`ws:<handle>`, `server`).
    pub scope_arg: String,
}

/// May `ws_id`'s compute jobs name the secret `m` at all (review B-L4,
/// A-L3)? Its own workspace's secrets, yes; a person (owner/admin) any;
/// an agent another scope's (server, another workspace) only once a
/// person granted it to this workspace's compute jobs. Anything else
/// answers exactly like a missing secret.
fn may_name(vault: &k2_vault::LocalVault, ws_id: &str, admin: bool, m: &k2_vault::SecretMeta) -> bool {
    if m.namespace != k2_vault::Namespace::User {
        return false;
    }
    if (m.scope.kind == ScopeKind::Workspace && m.scope.id == ws_id) || admin {
        return true;
    }
    vault
        .grants(&Selector::Id(m.id.clone()), &Actor::System("compute".into()))
        .is_ok_and(|gs| gs.iter().any(|g| g.consumer.compute_workspace() == Some(ws_id)))
}

/// Each ask → its secret id: `NAME` in the job's workspace scope, or a
/// `k2v://` reference the caller may name ([`may_name`]). Missing or not
/// nameable → the same 404 (exit 3).
pub(crate) fn find(ws_id: &str, ws_handle: &str, admin: bool, asks: &[Ask]) -> R<Vec<Found>> {
    if asks.is_empty() {
        return Ok(Vec::new());
    }
    let vault = crate::vault::get().map_err(|e| no(scodes::VAULT_LOCKED, 503, format!("the vault isn't open ({}); try again once it is", e.code)))?;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for a in asks {
        let sel = if k2_vault::refs::VaultRef::is_ref(&a.name) {
            crate::vault::selector_for_ref(&a.name).map_err(|e| no("secret_invalid", 400, format!("--secret {}: {}", a.name, e.hint)))?
        } else {
            Selector::user(Scope::workspace(ws_id), a.name.clone())
        };
        let missing = || no("secret_not_found", 404, format!("--secret {}: no such secret in workspace {ws_handle}'s vault (k2 vault list)", a.name));
        let m = match vault.meta(&sel, &Actor::System("compute".into())) {
            Ok(m) => m,
            Err(_) => {
                // Round 2, item 5: a grants query on this path too, so a
                // missing secret and one the caller may not name take about
                // as long (residual difference: one row lookup; accepted).
                let _ = vault.grants_to(&Consumer::compute(ws_id, "*"));
                return Err(missing());
            }
        };
        if !may_name(&vault, ws_id, admin, &m) {
            return Err(missing());
        }
        if !seen.insert(m.id.clone()) {
            return Err(no("secret_invalid", 400, format!("--secret {} is named twice", a.name)));
        }
        let scope_arg = match m.scope.kind {
            ScopeKind::Server => "server".to_string(),
            ScopeKind::Workspace => format!("ws:{}", crate::vault::workspace_label(&m.scope.id)),
            k => format!("{k}:{}", m.scope.id),
        };
        out.push(Found { ask: a.clone(), secret_id: m.id, secret_name: m.name, scope_arg });
    }
    Ok(out)
}

fn expired(g: &k2_vault::Grant) -> bool {
    g.expires_at
        .as_deref()
        .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
        .is_some_and(|t| t.timestamp() < compute::now())
}

/// What kind of grant covers a secret on a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// `compute:<ws>/<node>`.
    Node,
    /// `compute:<ws>/*` (nodes this server owns only).
    Star,
    /// `compute:<ws>/@lane-<seat>`: a seat that names this node.
    Lane(String),
}

/// The env grant that lets `ws_id`'s jobs on `node` have `secret_id`, and
/// how: one naming the node, `*` on a node this server owns (Q2), or a
/// lane grant for a seat of this server that names the node (C3b). No
/// value is resolved (SV15). Coverage only: [`secret_gate`] decides
/// whether a covered secret may be sealed there.
pub fn covering(ws_id: &str, node: &NodeRow, secret_id: &str) -> Option<(k2_vault::Grant, Via)> {
    let vault = crate::vault::get().ok()?;
    let mut consumers = vec![(Consumer::compute(ws_id, &node.id), Via::Node)];
    if node.owned_for_secrets() {
        consumers.push((Consumer::compute(ws_id, "*"), Via::Star));
    }
    // Lane grants count only while CI on compute is on (0.46.10, dark):
    // off, a lane grant made earlier covers nothing.
    if compute::ci_enabled() {
        for s in lanes::seat_book().seats_with_node(&node.id) {
            consumers.push((Consumer::compute_lane(ws_id, &s.seat_id), Via::Lane(s.seat_id)));
        }
    }
    consumers.into_iter().find_map(|(c, via)| {
        vault
            .grants_to(&c)
            .ok()?
            .into_iter()
            .map(|(g, _)| g)
            .find(|g| g.secret_id == secret_id && g.delivery == k2_vault::Delivery::Env && !expired(g))
            .map(|g| (g, via))
    })
}

/// [`covering`]'s grant only.
pub fn covering_grant(ws_id: &str, node: &NodeRow, secret_id: &str) -> Option<k2_vault::Grant> {
    covering(ws_id, node, secret_id).map(|(g, _)| g)
}

/// `--any` placement (SE6 + C3b): every secret is covered on `node` and
/// may be sealed there for a job with this `exclusive`.
pub fn placeable(ws_id: &str, node: &NodeRow, offer: Option<&Offer>, secret_ids: &[String], exclusive: bool) -> bool {
    secret_ids.iter().all(|id| covering(ws_id, node, id).is_some_and(|(_, via)| secret_gate(node, offer, &via, exclusive).is_ok()))
}

/// V25: may this server seal a secret covered `via` to `node`, for a job
/// with this `exclusive`, given the node's live `offer`? `Ok(Some(seat))`
/// when it goes under a lane's rules, `Ok(None)` on today's path.
///
/// - A node this server owns: today's rules (`*` or a named grant).
/// - A lane grant: always the lane's rules for that seat
///   ([`lanes::lane_gate`]).
/// - A named grant on another owner's node: the lane's rules when the
///   node is on one of this server's seats or offers `lane-secrets` (it
///   would take the secret because of a lane); otherwise today's path,
///   where the node takes secrets only from a server it trusts `own`
///   (and an older node never from anyone else).
pub fn secret_gate(node: &NodeRow, offer: Option<&Offer>, via: &Via, exclusive: bool) -> Result<Option<String>, Refused> {
    let book = lanes::seat_book();
    let check = |seat: &SeatView| -> Result<(), Refused> {
        let consent = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            lanes::consent_for(&conn, seat)
        };
        let standing = book.node_standing(seat, node);
        lanes::lane_gate(&lanes::GateInput { node, offer, seat, standing, consent: consent.as_ref(), exclusive })
    };
    match via {
        Via::Lane(_) if !compute::ci_enabled() => Err((compute::CI_PREVIEW_OFF, compute::ci_preview_hint("A lane secret"))),
        Via::Lane(seat_id) => {
            let seat = book.seat(seat_id).ok_or_else(|| (scodes::UNTRUSTED_SERVER, format!("the seat {seat_id} this secret is granted to is gone")))?;
            check(&seat).map(|()| Some(seat.seat_id))
        }
        Via::Node | Via::Star if node.owned_for_secrets() => Ok(None),
        Via::Star => Err((scodes::NOT_GRANTED, "a * grant covers only nodes this server owns".into())),
        Via::Node => {
            let seats = book.seats_with_node(&node.id);
            let lane_offer = offer.is_some_and(|o| o.features.iter().any(|f| f == k2_core::compute::proto::ci::FEATURE_LANE_SECRETS));
            if seats.is_empty() && !lane_offer {
                return Ok(None);
            }
            let mut first: Option<Refused> = None;
            for s in &seats {
                match check(s) {
                    Ok(()) => return Ok(Some(s.seat_id.clone())),
                    Err(e) => {
                        first.get_or_insert(e);
                    }
                }
            }
            Err(first.unwrap_or_else(|| {
                (scodes::UNTRUSTED_SERVER, format!("'{}' offers secrets under a lane, but this server holds no seat on it naming that node", node.name))
            }))
        }
    }
}

/// [`secret_gate`] for every secret of a submit (`found` with coverage):
/// the first refusal as an HTTP answer (403 for trust, 409 for the node).
pub(crate) fn gate_all(ws_id: &str, node: &NodeRow, offer: Option<&Offer>, found: &[(Found, String, bool)], exclusive: bool) -> R<()> {
    for (f, _, _) in found {
        let Some((_, via)) = covering(ws_id, node, &f.secret_id) else { continue };
        if let Err((code, msg)) = secret_gate(node, offer, &via, exclusive) {
            use k2_core::compute::proto::ci::codes as ccodes;
            let trust = [scodes::UNTRUSTED_SERVER, scodes::NOT_GRANTED, ccodes::SECRET_OWNER_NOT_TRUSTED, ccodes::NODE_KEY_TAINTED];
            let status = if trust.contains(&code) { 403 } else { 409 };
            return Err(no(code, status, format!("--secret {}: {msg}", f.ask.name)));
        }
    }
    Ok(())
}

/// What `--plan` shows per secret (SE7): names and coverage, no value.
pub fn plan_view(found: &[(Found, String, bool)]) -> serde_json::Value {
    serde_json::Value::Array(
        found
            .iter()
            .map(|(f, env, covered)| serde_json::json!({"name": f.ask.name, "secret": f.secret_name, "env": env, "covered": covered}))
            .collect(),
    )
}

/// The env name each secret is delivered under on `node` (the ask's, else
/// the covering grant's, else the vault default), checked against both
/// deny lists and `--env` (SE2), with the node's coverage (SE4).
pub(crate) fn env_names(ws_id: &str, node: &NodeRow, found: &[Found], env: &BTreeMap<String, String>) -> R<Vec<(Found, String, bool)>> {
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for f in found {
        let g = covering_grant(ws_id, node, &f.secret_id);
        let name = f
            .ask
            .env
            .clone()
            .or_else(|| g.as_ref().and_then(|g| g.env_name.clone()))
            .unwrap_or_else(|| k2_vault::delivery::default_env_name(&f.secret_name));
        if let Err(e) = k2_vault::delivery::validate_env_name(&name) {
            return Err(no("env_refused", 400, format!("--secret {}: env name {name}: {}", f.ask.name, e.hint)));
        }
        if let Some(code) = psecret::name_refusal(&name) {
            return Err(no("env_refused", 400, format!("--secret {}: env name {name}: {code} (K2 names, the node's own names and loader names can't be set)", f.ask.name)));
        }
        if env.contains_key(&name) || !names.insert(name.clone()) {
            return Err(no("env_secret_collision", 400, format!("{name} is set twice (by --env or another --secret); a secret's env name must be its own")));
        }
        out.push((f.clone(), name, g.is_some()));
    }
    Ok(out)
}

/// `secret_not_granted` (403, exit 3) with the owner's exact command.
pub(crate) fn not_granted(found: &[(Found, String, bool)], ws_handle: &str, node: &NodeRow) -> Option<crate::compute_routes::Refusal> {
    let missing: Vec<&(Found, String, bool)> = found.iter().filter(|(_, _, c)| !c).collect();
    if missing.is_empty() {
        return None;
    }
    // On a seat: the lane form (one grant for every node of the seat).
    let on_seat = if compute::ci_enabled() { lanes::seat_book().seats_with_node(&node.id).into_iter().find(|s| s.active) } else { None };
    let target = match on_seat {
        Some(s) => format!("{LANE_TOKEN}{}/{}", s.owner_id, s.lane),
        None => node.name.clone(),
    };
    let cmds: Vec<String> = missing
        .iter()
        .map(|(f, env, _)| format!("k2 vault grant {} --scope {} --to compute:{ws_handle}/{target} --as env --env-name {env}", f.secret_name, f.scope_arg))
        .collect();
    Some(no(
        scodes::NOT_GRANTED,
        403,
        format!(
            "{} isn't granted to workspace {ws_handle}'s compute jobs on '{}'. Only a person can grant it; ask your owner to run: {}{}",
            missing.iter().map(|(f, _, _)| f.secret_name.as_str()).collect::<Vec<_>>().join(", "),
            node.name,
            cmds.join(" ; "),
            tenant_warning(node).map(|w| format!(". {w}")).unwrap_or_default()
        ),
    ))
}

/// A node this server doesn't own (review B-L3): its machine's admin
/// (root) and its k2-node user can read any value a job there gets.
pub fn tenant_warning(node: &NodeRow) -> Option<String> {
    (!node.owned_for_secrets()).then(|| {
        format!(
            "Warning: '{}' belongs to another K2 server ({}); that machine's admin (root) and its k2-node user can read the value",
            node.name,
            node.owner_label.as_deref().unwrap_or("its owner")
        )
    })
}

/// The warning a `compute:` grant answers with (`k2 vault grant`), when its
/// node isn't this server's own.
pub fn grant_warning(c: &Consumer) -> Option<String> {
    if let Some((_, seat_id)) = c.compute_lane_parts() {
        let s = lanes::seat_book().seat(seat_id)?;
        return Some(format!(
            "Warning: jobs on {}'s lane {} run on machines that server owns; their admin (root) can read the value. It is sealed only to a Linux node of your seat, \
             in a job with its own uid or --exclusive, and only while a person here consents for that owner ({})",
            s.owner_id,
            s.lane,
            lanes::consent_command(&s.owner_id)
        ));
    }
    let (_, node) = c.compute_parts()?;
    if node == "*" {
        return None;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = store::node_by_id(&conn, node)?;
    drop(conn);
    tenant_warning(&n)
}

/// The pins and the plan's env names (sorted, unique).
pub fn pins(found: &[(Found, String, bool)]) -> (Vec<SecretPin>, Vec<String>) {
    let pins: Vec<SecretPin> = found.iter().map(|(f, env, _)| SecretPin { name: f.ask.name.clone(), secret_id: f.secret_id.clone(), env: env.clone() }).collect();
    let mut names: Vec<String> = pins.iter().map(|p| p.env.clone()).collect();
    names.sort();
    names.dedup();
    (pins, names)
}

// ── release (SE9–SE12) ───────────────────────────────────────────────

/// A refusal to release: `(code, message)`.
pub type Refused = (&'static str, String);

/// Releases in flight by (job, generation), and attempts already answered
/// (bounded, oldest out first).
static NEEDS: Mutex<Option<(BTreeSet<(String, u32)>, std::collections::VecDeque<(String, u32, u32)>, BTreeSet<(String, u32, u32)>)>> = Mutex::new(None);
const NEED_LATCH_CAP: usize = 4096;

/// Is `job_id` a job this server placed on `node_id` (any attempt)? Checked
/// before a `secret_need` takes a place (round 2, item 3).
pub fn job_on_node(node_id: &str, job_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job_id).is_some_and(|j| j.node_id == node_id)
}

/// A taken in-flight place; dropping it (also on a panic) frees it.
pub struct NeedPlace(String, u32);

impl Drop for NeedPlace {
    fn drop(&mut self) {
        let mut g = NEEDS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((flight, _, _)) = g.as_mut() {
            flight.remove(&(std::mem::take(&mut self.0), self.1));
        }
    }
}

/// Take one `secret_need` (review A-L1/L2): `None` when a release for this
/// (job, generation) is in flight, or this attempt was already answered.
pub fn need_begin(n: &SecretNeed) -> Option<NeedPlace> {
    let mut g = NEEDS.lock().unwrap_or_else(|p| p.into_inner());
    let (flight, order, latched) = g.get_or_insert_with(Default::default);
    let attempt = (n.job_id.clone(), n.generation, n.attempt);
    if flight.contains(&(n.job_id.clone(), n.generation)) || latched.contains(&attempt) {
        return None;
    }
    flight.insert((n.job_id.clone(), n.generation));
    latched.insert(attempt.clone());
    order.push_back(attempt);
    while order.len() > NEED_LATCH_CAP {
        if let Some(old) = order.pop_front() {
            latched.remove(&old);
        }
    }
    Some(NeedPlace(n.job_id.clone(), n.generation))
}

/// Check everything for one `secret_need` and seal the values. On success:
/// the `secret_env` frame, the env names, and the belt's redactor.
/// `Err(None)`: not a job of this node, dropped without an answer or a
/// k2.db row (review A-L1).
pub fn release(node_id: &str, n: &SecretNeed) -> Result<(SecretEnv, Vec<String>, Redactor), Option<Refused>> {
    let stale = |m: &str| Some((scodes::ATTEMPT_STALE, m.to_string()));
    let (job, node) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::job_by_id(&conn, &n.job_id), store::node_by_id(&conn, node_id))
    };
    let (Some(job), Some(node)) = (job, node) else { return Err(None) };
    if job.node_id != node_id {
        return Err(None);
    }
    if job.generation != n.generation as i64 || job.attempt != n.attempt as i64 || !k2_core::compute::proto::crypto::ct_eq(job.plan_digest.as_bytes(), n.plan_digest.as_bytes()) {
        return Err(stale("not this job's current attempt on this node"));
    }
    match job.state.as_str() {
        "assigned" | "preparing" => {}
        "running" | "finishing" => return Err(Some((scodes::ALREADY_RUNNING, "the job already runs; a secret is released only before it starts".into()))),
        s => return Err(stale(&format!("the job is {s}"))),
    }
    let plan = job.plan().ok_or_else(|| stale("the job's plan is unreadable"))?;
    let want: BTreeSet<&str> = plan.secret_env.iter().map(String::as_str).collect();
    let have: BTreeSet<&str> = job.ctl.secrets.iter().map(|p| p.env.as_str()).collect();
    if want.is_empty() || want != have {
        return Err(stale("this job has no secret env"));
    }
    let key = compute::controller_key().map_err(|e| Some((scodes::UNAVAILABLE, format!("controller key: {e}"))))?;
    let vault = crate::vault::get().map_err(|e| Some((scodes::VAULT_LOCKED, format!("the vault isn't open ({})", e.code))))?;
    // V25: every secret's grant, and the lane's rules where they apply,
    // checked again now against the node's live offer, before any value
    // is resolved. The seat checked goes to the vault, so a lane grant
    // matches only that seat.
    let offer = crate::compute_ws::live_view(node_id).2;
    let mut seats: Vec<Option<String>> = Vec::new();
    for p in &job.ctl.secrets {
        let Some((_, via)) = covering(&job.workspace_id, &node, &p.secret_id) else {
            return Err(Some((scodes::NOT_GRANTED, format!("{}: no grant covers this node now", p.name))));
        };
        let seat = secret_gate(&node, offer.as_ref(), &via, plan.exclusive).map_err(|(code, m)| Some((code, format!("{}: {m}", p.name))))?;
        seats.push(seat);
    }
    let mut values: Vec<(String, Vec<u8>)> = Vec::new();
    let wipe_all = |v: &mut Vec<(String, Vec<u8>)>| {
        for (_, x) in v.iter_mut() {
            psecret::wipe(x);
        }
    };
    for (p, seat) in job.ctl.secrets.iter().zip(seats) {
        let who = Requester::Compute {
            workspace_id: job.workspace_id.clone(),
            node_id: node_id.to_string(),
            job_id: job.id.clone(),
            generation: n.generation,
            owned_node: node.owned_for_secrets(),
            lane_seat: seat,
        };
        match vault.resolve(&Selector::Id(p.secret_id.clone()), Use::Env, &who) {
            Ok(r) => values.push((p.env.clone(), r.value.expose().to_vec())),
            Err(e) => {
                wipe_all(&mut values);
                let code = match e.code {
                    "secret_not_active" | "secret_expired" => scodes::NOT_ACTIVE,
                    "not_granted" | "delivery_not_granted" | "not_found" => scodes::NOT_GRANTED,
                    "vault_not_open" | "vault_locked" => scodes::VAULT_LOCKED,
                    _ => scodes::UNAVAILABLE,
                };
                return Err(Some((code, format!("{}: {}", p.name, e.hint))));
            }
        }
    }
    let binding = psecret::Binding {
        controller_fp: key.fingerprint(),
        node_fp: node.fingerprint.clone(),
        job_id: job.id.clone(),
        generation: n.generation,
        attempt: n.attempt,
        plan_digest: job.plan_digest.clone(),
    };
    let sealed = psecret::seal(&values, &n.epk, &binding);
    // The belt matches what the node matches: values of 8+ bytes (SE23).
    let redactor = Redactor::new(values.iter().filter(|(_, v)| v.len() >= k2_vault::redact::SURFACE_MIN_LEN).map(|(k, v)| (k.as_str(), v.as_slice())));
    wipe_all(&mut values);
    let (epk, ct) = sealed.map_err(|code| {
        let m = if code == scodes::TOO_LARGE {
            format!("over the caps: at most {} secrets, {} KiB each (text, no NUL), {} KiB sealed", psecret::MAX_SECRETS, psecret::MAX_VALUE_BYTES / 1024, psecret::MAX_SEALED_BYTES / 1024)
        } else {
            "the node's key for this attempt is unusable".to_string()
        };
        Some((code, m))
    })?;
    let names: Vec<String> = plan.secret_env.clone();
    Ok((SecretEnv { job_id: job.id.clone(), generation: n.generation, epk, ct }, names, redactor))
}

/// Record the release or refusal on the job (SE12): names only.
pub fn record(node_id: &str, n: &SecretNeed, r: &Result<Vec<String>, Refused>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ws = store::job_by_id(&conn, &n.job_id).map(|j| j.workspace_id);
    let (kind, detail) = match r {
        Ok(names) => ("secret_release", serde_json::json!({"generation": n.generation, "attempt": n.attempt, "secrets": names})),
        Err((code, msg)) => ("secret_refused", serde_json::json!({"generation": n.generation, "attempt": n.attempt, "code": code, "detail": msg})),
    };
    store::record_event(&conn, "controller", kind, Some(node_id), ws.as_deref(), Some(&n.job_id), detail);
}

// ── the belt (SE24) ──────────────────────────────────────────────────

/// Released attempts' redactors, by job: `(generation, redactor, missed)`.
/// In memory only (a controller restart turns the belt off for that
/// attempt; the node stays the primary).
static BELT: Mutex<Option<HashMap<String, (u32, Arc<Redactor>, bool)>>> = Mutex::new(None);

pub fn belt_arm(job_id: &str, generation: u32, r: Redactor) {
    let mut g = BELT.lock().unwrap_or_else(|p| p.into_inner());
    g.get_or_insert_with(HashMap::new).insert(job_id.to_string(), (generation, Arc::new(r), false));
}

/// The attempt ended (or a newer one replaced it): drop its redactor.
pub fn belt_disarm(job_id: &str, generation: u32) {
    let mut g = BELT.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(m) = g.as_mut() {
        if m.get(job_id).is_some_and(|(gen, _, _)| *gen <= generation) {
            m.remove(job_id);
        }
    }
}

/// Is the belt armed for this attempt? (tests)
pub fn belt_armed(job_id: &str, generation: u32) -> bool {
    BELT.lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|m| m.get(job_id).is_some_and(|(g, _, _)| *g == generation))
}

/// One log chunk through the belt: `Some(redacted)` when it held a value
/// the node should have caught (recorded once per attempt as
/// `secret_redaction_miss`). Per chunk, no hold-back (SV9).
pub fn belt(job_id: &str, generation: u32, data: &[u8]) -> Option<Vec<u8>> {
    let (r, first) = {
        let mut g = BELT.lock().unwrap_or_else(|p| p.into_inner());
        let e = g.as_mut()?.get_mut(job_id)?;
        if e.0 != generation || !e.1.hits(data) {
            return None;
        }
        let first = !e.2;
        e.2 = true;
        (e.1.clone(), first)
    };
    let out = r.redact(data);
    if first {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ws = store::job_by_id(&conn, job_id).map(|j| j.workspace_id);
        let node = store::job_by_id(&conn, job_id).map(|j| j.node_id);
        store::record_event(
            &conn,
            "controller",
            scodes::REDACTION_MISS,
            node.as_deref(),
            ws.as_deref(),
            Some(job_id),
            serde_json::json!({"generation": generation, "detail": "a log chunk from the node held a secret value: redacted here; the stored log now differs from the receipt's log.sha256"}),
        );
    }
    Some(out)
}

// ── revoke (SE28) ────────────────────────────────────────────────────

/// Jobs still running that got `secret_id` under `consumer` (a compute
/// consumer). Revoking never stops them; this says which got it.
pub fn running_with(secret_id: &str, consumer: &Consumer) -> Vec<serde_json::Value> {
    // A lane grant: the nodes of its seat.
    let (ws, node, seat_nodes) = match (consumer.compute_parts(), consumer.compute_lane_parts()) {
        (Some((ws, node)), _) => (ws, node, None),
        (None, Some((ws, seat))) => (ws, "", Some(lanes::seat_book().seat(seat).map(|s| s.node_ids).unwrap_or_default())),
        (None, None) => return Vec::new(),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = Vec::new();
    for n in store::list_nodes(&conn, false) {
        if let Some(on_seat) = &seat_nodes {
            if !on_seat.contains(&n.id) {
                continue;
            }
        } else if node != "*" && n.id != node {
            continue;
        }
        if seat_nodes.is_none() && node == "*" && !n.owned_for_secrets() {
            continue;
        }
        for j in store::jobs_in_states(&conn, &n.id, &["preparing", "running", "finishing"]) {
            if j.workspace_id != ws {
                continue;
            }
            // Still covered by another grant (review item 8): not news.
            if covering_grant(ws, &n, secret_id).is_some() {
                continue;
            }
            for p in j.ctl.secrets.iter().filter(|p| p.secret_id == secret_id) {
                out.push(serde_json::json!({"job": j.id, "short": compute::message::short(&j.id), "node": n.name, "env": p.env, "state": j.state}));
            }
        }
    }
    out
}

/// Every live `compute:<ws>/*` env grant, as `(workspace, secret name)`,
/// sorted: what starts reaching a node the moment this server owns it
/// (Q2; the take-over's confirm names them, verification M1). `None` when
/// the vault is closed (they can't be listed, so nobody is told "none").
pub fn star_grants() -> Option<Vec<(String, String)>> {
    let vault = crate::vault::get().ok()?;
    star_grants_in(&vault)
}

/// [`star_grants`] in a given vault.
pub fn star_grants_in(vault: &k2_vault::LocalVault) -> Option<Vec<(String, String)>> {
    let ids: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut st = conn.prepare("SELECT id FROM projects").ok()?;
        let rows = st.query_map([], |r| r.get::<_, String>(0)).ok()?;
        rows.filter_map(Result::ok).collect()
    };
    let mut out = Vec::new();
    for ws in ids {
        let Ok(gs) = vault.grants_to(&Consumer::compute(&ws, "*")) else { return None };
        for (g, name) in gs {
            if g.delivery == k2_vault::Delivery::Env && !expired(&g) {
                out.push((crate::compute_routes::ws_handle(&ws), name));
            }
        }
    }
    out.sort();
    out.dedup();
    Some(out)
}

/// [`star_grants`] as one line for a person: `ws: A, B; ws2: C`.
pub fn star_grants_text(g: &Option<Vec<(String, String)>>) -> String {
    match g {
        None => "(the vault is closed, so they can't be listed: open it first and look again)".into(),
        Some(v) if v.is_empty() => "none today".into(),
        Some(v) => {
            let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (ws, s) in v {
                by.entry(ws.as_str()).or_default().push(s.as_str());
            }
            by.iter().map(|(ws, ss)| format!("{ws}: {}", ss.join(", "))).collect::<Vec<_>>().join("; ")
        }
    }
}

/// The env names a job gets (`get` / `status` / `receipt`, SE8).
pub fn job_secret_names(j: &JobRow) -> Vec<String> {
    j.plan().map(|p| p.secret_env).unwrap_or_default()
}
