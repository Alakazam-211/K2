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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use k2_core::compute::proto::frames::{SecretEnv, SecretNeed};
use k2_core::compute::proto::secret::{self as psecret, codes as scodes};
use k2_core::compute::store::{self, JobRow, NodeRow, SecretPin};
use k2_core::compute::{self as compute};
use k2_vault::redact::Redactor;
use k2_vault::{Actor, Consumer, Requester, Scope, ScopeKind, SecretStore, Selector, Use, VaultError};

use crate::compute_routes::{no, R};

// ── consumers (`k2 vault grant --to compute:…`) ──────────────────────

/// `compute:[<ws>/]<node|*>` → the vault consumer
/// `compute:<workspace id>/<node id or *>`. `caller_ws` is the caller's own
/// workspace, used when no workspace is named.
pub fn consumer(spec: &str, caller_ws: Option<&str>) -> Result<Consumer, VaultError> {
    let bad = |m: &str| VaultError::bad_request("bad_consumer", format!("{m} (use compute:<workspace>/<node>, or compute:<workspace>/* for every node this server owns)"));
    let (ws_tok, node_tok) = match spec.split_once('/') {
        Some((w, n)) => (Some(w), n),
        None => (None, spec),
    };
    let ws_id = match ws_tok {
        Some(w) if !w.is_empty() => crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?,
        Some(_) => return Err(bad("empty workspace")),
        None => caller_ws.map(str::to_string).ok_or_else(|| bad("name the workspace"))?,
    };
    if node_tok.is_empty() {
        return Err(bad("name a node or *"));
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

/// How people see a compute consumer: `compute:<ws handle>/<node name>`.
pub fn label(c: &Consumer) -> String {
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
pub fn asks_from(v: &serde_json::Value) -> R<Vec<Ask>> {
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

/// Each ask → its secret id: `NAME` in the job's workspace scope, or a
/// `k2v://` reference. Missing → 404 (exit 3).
pub fn find(ws_id: &str, ws_handle: &str, asks: &[Ask]) -> R<Vec<Found>> {
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
        let m = vault
            .meta(&sel, &Actor::System("compute".into()))
            .map_err(|_| no("secret_not_found", 404, format!("--secret {}: no such secret in workspace {ws_handle}'s vault (k2 vault list)", a.name)))?;
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

/// The env grant that lets `ws_id`'s jobs on `node` have `secret_id`: one
/// naming the node, or `*` on a node this server owns (Q2). No value is
/// resolved (SV15).
pub fn covering_grant(ws_id: &str, node: &NodeRow, secret_id: &str) -> Option<k2_vault::Grant> {
    let vault = crate::vault::get().ok()?;
    let mut consumers = vec![Consumer::compute(ws_id, &node.id)];
    if !node.is_tenant() {
        consumers.push(Consumer::compute(ws_id, "*"));
    }
    consumers.iter().find_map(|c| {
        vault
            .grants_to(c)
            .ok()?
            .into_iter()
            .map(|(g, _)| g)
            .find(|g| g.secret_id == secret_id && g.delivery == k2_vault::Delivery::Env && !expired(g))
    })
}

/// Every one of `secret_ids` is covered on `node` (`--any` placement).
pub fn covers_all(ws_id: &str, node: &NodeRow, secret_ids: &[String]) -> bool {
    secret_ids.iter().all(|id| covering_grant(ws_id, node, id).is_some())
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
pub fn env_names(ws_id: &str, node: &NodeRow, found: &[Found], env: &BTreeMap<String, String>) -> R<Vec<(Found, String, bool)>> {
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
pub fn not_granted(found: &[(Found, String, bool)], ws_handle: &str, node: &NodeRow) -> Option<crate::compute_routes::Refusal> {
    let missing: Vec<&(Found, String, bool)> = found.iter().filter(|(_, _, c)| !c).collect();
    if missing.is_empty() {
        return None;
    }
    let cmds: Vec<String> = missing
        .iter()
        .map(|(f, env, _)| format!("k2 vault grant {} --scope {} --to compute:{ws_handle}/{} --as env --env-name {env}", f.secret_name, f.scope_arg, node.name))
        .collect();
    Some(no(
        scodes::NOT_GRANTED,
        403,
        format!(
            "{} isn't granted to workspace {ws_handle}'s compute jobs on '{}'. Only a person can grant it; ask your owner to run: {}",
            missing.iter().map(|(f, _, _)| f.secret_name.as_str()).collect::<Vec<_>>().join(", "),
            node.name,
            cmds.join(" ; ")
        ),
    ))
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

/// Check everything for one `secret_need` and seal the values. On success:
/// the `secret_env` frame, the env names, and the belt's redactor.
pub fn release(node_id: &str, n: &SecretNeed) -> Result<(SecretEnv, Vec<String>, Redactor), Refused> {
    let stale = |m: &str| (scodes::ATTEMPT_STALE, m.to_string());
    let (job, node) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::job_by_id(&conn, &n.job_id), store::node_by_id(&conn, node_id))
    };
    let job = job.ok_or_else(|| stale("no such job"))?;
    let node = node.ok_or_else(|| stale("no such node"))?;
    if job.node_id != node_id || job.generation != n.generation as i64 || job.attempt != n.attempt as i64 || !k2_core::compute::proto::crypto::ct_eq(job.plan_digest.as_bytes(), n.plan_digest.as_bytes()) {
        return Err(stale("not this job's current attempt on this node"));
    }
    match job.state.as_str() {
        "assigned" | "preparing" => {}
        "running" | "finishing" => return Err((scodes::ALREADY_RUNNING, "the job already runs; a secret is released only before it starts".into())),
        s => return Err(stale(&format!("the job is {s}"))),
    }
    let plan = job.plan().ok_or_else(|| stale("the job's plan is unreadable"))?;
    let want: BTreeSet<&str> = plan.secret_env.iter().map(String::as_str).collect();
    let have: BTreeSet<&str> = job.ctl.secrets.iter().map(|p| p.env.as_str()).collect();
    if want.is_empty() || want != have {
        return Err(stale("this job has no secret env"));
    }
    let key = compute::controller_key().map_err(|e| (scodes::UNAVAILABLE, format!("controller key: {e}")))?;
    let vault = crate::vault::get().map_err(|e| (scodes::VAULT_LOCKED, format!("the vault isn't open ({})", e.code)))?;
    let who = Requester::Compute {
        workspace_id: job.workspace_id.clone(),
        node_id: node_id.to_string(),
        job_id: job.id.clone(),
        generation: n.generation,
        owned_node: !node.is_tenant(),
    };
    let mut values: Vec<(String, Vec<u8>)> = Vec::new();
    let wipe_all = |v: &mut Vec<(String, Vec<u8>)>| {
        for (_, x) in v.iter_mut() {
            psecret::wipe(x);
        }
    };
    for p in &job.ctl.secrets {
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
                return Err((code, format!("{}: {}", p.name, e.hint)));
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
        (code, m)
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
    let Some((ws, node)) = consumer.compute_parts() else { return Vec::new() };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = Vec::new();
    for n in store::list_nodes(&conn, false) {
        if node != "*" && n.id != node {
            continue;
        }
        if node == "*" && n.is_tenant() {
            continue;
        }
        for j in store::jobs_in_states(&conn, &n.id, &["preparing", "running", "finishing"]) {
            if j.workspace_id != ws {
                continue;
            }
            for p in j.ctl.secrets.iter().filter(|p| p.secret_id == secret_id) {
                out.push(serde_json::json!({"job": j.id, "short": compute::message::short(&j.id), "node": n.name, "env": p.env, "state": j.state}));
            }
        }
    }
    out
}

/// The env names a job gets (`get` / `status` / `receipt`, SE8).
pub fn job_secret_names(j: &JobRow) -> Vec<String> {
    j.plan().map(|p| p.secret_env).unwrap_or_default()
}
