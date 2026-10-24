//! `k2 vault systemd-cred` and placement records (`prd-vault-gaps-v1`
//! item 4, 0.46.4; hardened after two security reviews).
//!
//! - `POST /cli/vault/systemd-cred` (people: Owner/Admin + a vault confirm
//!   bound to the exact plan; no passport, `PASSPORT_DENIED`). Three ops:
//!   - place (`unit`): resolve a `user` secret on the person's behalf
//!     (`Use::System`), pipe it to the root helper's `creds put <cred>`
//!     (stdin), `creds bind <unit> <cred>`, and `creds restart <unit>` only
//!     with `restart`;
//!   - replace (`replace: true`): re-place the latest version into every
//!     systemd credential of the secret that is behind (after a rotate);
//!   - `dryRun`: the plan and the confirm text, no confirm, nothing sent.
//!   Linux only (`linux_only`); the helper must speak protocol 10 with
//!   `namespaces.creds` (`root_helper_missing` with the manual steps); the
//!   unit must be on the box's allow list (`unit_not_allowed` with the
//!   admin's step).
//! - `POST /cli/vault/systemd-cred/remove` (people, confirm bound to the
//!   plan): `creds unbind`, then `creds remove` when no other placement
//!   loads that cred.
//! - `POST /cli/vault/placement/add|done|remove`, `GET
//!   /cli/vault/placements`: manual placement records. Names and places
//!   only, never a value. An agent may keep them for its own workspace's
//!   secrets; `systemd_cred` rows are people-only.
//! - [`after_new_version`]: a new version (rotate, set, a handoff, …) never
//!   reaches a root unit by itself. Only a person with a fresh confirm
//!   (rollback, `--replace`) or a K2 subsystem in
//!   [`REPLACE_SYSTEM_ACTORS`] (none today) re-places; for anyone else the
//!   placement goes stale, the owner is told, and the doctor says
//!   `placement_stale` (security review A-C1 / B-H1).
//! - Every confirm is bound to the exact version and shows who wrote it; a
//!   value no person wrote needs `acceptAgentValue` (A2-N2 / B2-N1).
//!
//! An Owner or Admin (with a confirm) can place a value into any unit the
//! box's admin listed; that is the design, not a hole.
//! - [`doctor_findings`]: `placement_stale`, `placement_manual_due`,
//!   `placement_of_inactive_secret`, and rows a newer K2 wrote.
//!
//! Values travel daemon memory → sudo stdin only: never argv, a log, an
//! error or an audit line.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use k2_root_proto::creds::{cred_name_ok, cred_path, deny_reason, dropin_file, reserved_prefix, unit_name_ok, ALLOW_LIST};
use k2_root_proto::version::CREDS_MIN_PROTOCOL;
use k2_vault::vault::{NewPlacement, Placement, PlacementTarget};
use k2_vault::{Actor, Delivery, LocalVault, Namespace, Requester, ScopeKind, SecretMeta, SecretStatus, SecretStore, Selector, Use, VaultError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::root_helper::RootHelperState;
use crate::vault::actor::VaultCaller;
use crate::vault_routes::{self as vr, Sel};

pub const SYSTEMD_CRED: &str = "/cli/vault/systemd-cred";
pub const SYSTEMD_CRED_REMOVE: &str = "/cli/vault/systemd-cred/remove";
pub const PLACEMENT_ADD: &str = "/cli/vault/placement/add";
pub const PLACEMENT_DONE: &str = "/cli/vault/placement/done";
pub const PLACEMENT_REMOVE: &str = "/cli/vault/placement/remove";
pub const PLACEMENTS: &str = "/cli/vault/placements";

pub const READ_ROUTES: &[&str] = &[PLACEMENTS];
pub const POST_ROUTES: &[&str] = &[SYSTEMD_CRED, SYSTEMD_CRED_REMOVE, PLACEMENT_ADD, PLACEMENT_DONE, PLACEMENT_REMOVE];

/// The vault confirm action of every systemd-cred op. The token is bound
/// to the plan's digest ([`plan_digest`]), not just the secret.
pub const CONFIRM_ACTION: &str = "systemd-cred";
/// The audit stamp of K2's own re-puts.
pub const SYSTEM_ACTOR: &str = "system:systemd-cred";

/// One systemd-cred call at a time on this daemon: the cred-name check, the
/// pending row, the helper calls and the record are one step (review B-L3).
static PLACE_LOCK: Mutex<()> = Mutex::new(());

fn place_lock() -> std::sync::MutexGuard<'static, ()> {
    PLACE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn is_route(path: &str) -> bool {
    READ_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

fn err(code: &'static str, status: u16, hint: impl Into<String>) -> VaultError {
    VaultError::new(code, status, hint)
}

// ── the root door ───────────────────────────────────────────────────

/// What the routes need from the box, behind a trait so tests never run
/// sudo or read the real helper.
pub trait CredsDoor {
    fn linux(&self) -> bool;
    fn helper(&self) -> RootHelperState;
    /// `sudo -n k2-root-helper <argv…>`; the value only ever as `stdin`.
    fn call(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<Vec<u8>, String>;
}

struct RealDoor;

impl CredsDoor for RealDoor {
    fn linux(&self) -> bool {
        cfg!(target_os = "linux")
    }
    fn helper(&self) -> RootHelperState {
        crate::root_helper::state()
    }
    fn call(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        crate::root_helper::call(argv, stdin)
    }
}

#[cfg(test)]
pub mod test_seam {
    use std::cell::RefCell;
    use std::rc::Rc;
    thread_local! {
        pub static DOOR: RefCell<Option<Rc<dyn super::CredsDoor>>> = const { RefCell::new(None) };
    }
    pub fn set(d: Option<Rc<dyn super::CredsDoor>>) {
        DOOR.with(|s| *s.borrow_mut() = d);
    }
}

fn with_door<T>(f: impl FnOnce(&dyn CredsDoor) -> T) -> T {
    #[cfg(test)]
    {
        if let Some(d) = test_seam::DOOR.with(|s| s.borrow().clone()) {
            return f(d.as_ref());
        }
    }
    f(&RealDoor)
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The helper serves `creds`: protocol 10 AND `namespaces.creds`.
fn helper_ready(st: &RootHelperState) -> bool {
    st.usable(CREDS_MIN_PROTOCOL)
        && st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/creds")).and_then(Value::as_bool) == Some(true)
}

/// The daemon units `daemon.toml` names (from `helper version`), plus this
/// daemon's own unit: never a credential target.
fn daemon_units(st: &RootHelperState) -> Vec<String> {
    let mut out: Vec<String> = st
        .version_json
        .as_ref()
        .and_then(|v| v["daemons"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d["unit"].as_str().map(str::to_string))
        .collect();
    if cfg!(not(test)) {
        out.push(crate::root_helper::own_unit().1);
    }
    out
}

/// The box's allow list, as the helper reads it (`creds status`). `None`
/// when the helper can't say (it still enforces the list itself).
fn allowed_units(door: &dyn CredsDoor) -> Option<Vec<String>> {
    let out = door.call(&sv(&["creds", "status"]), None).ok()?;
    let v: Value = serde_json::from_slice(&out).ok()?;
    Some(v["allowedUnits"].as_array()?.iter().filter_map(|u| u.as_str().map(str::to_string)).collect())
}

/// How the box's admin lets K2 put credentials on a unit.
fn admin_step(unit: &str) -> String {
    format!(
        "the box's admin opts the unit in, as root over SSH (not from a K2 terminal): \
         echo {unit} >> {ALLOW_LIST} (root:root 0644), or install-root-helper.sh --creds-unit {unit}"
    )
}

/// The steps to do it by hand (no helper, or an old one).
fn manual_steps(cred: &str, unit: &str) -> String {
    format!(
        "Update the root helper first (k2 root-helper update; it needs protocol {CREDS_MIN_PROTOCOL}). By hand meanwhile, as root: \
         install -d -m 0700 /etc/k2/credstore.encrypted; systemd-creds encrypt --name={cred} --with-key=auto - {path} < <value file>; \
         a drop-in /etc/systemd/system/{unit}.d/creds.conf with [Service] LoadCredentialEncrypted={cred}:{path}; \
         systemctl daemon-reload && systemctl restart {unit}; then record it: k2 vault placement add NAME --where <host>:{path}",
        path = cred_path(cred)
    )
}

fn helper_err(step: &str, e: String) -> VaultError {
    // The helper's words never carry the value (stdin is never echoed).
    err("helper_failed", 502, format!("root helper {step}: {e}"))
}

// ── entry ───────────────────────────────────────────────────────────

pub fn handle(
    vault: &Arc<LocalVault>,
    path: &str,
    c: &VaultCaller,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
) -> CliResponse {
    let r = match path {
        SYSTEMD_CRED => systemd_cred(vault, c, body, ingress),
        SYSTEMD_CRED_REMOVE => systemd_cred_remove(vault, c, body, ingress),
        PLACEMENT_ADD => placement_add(vault, c, body),
        PLACEMENT_DONE => placement_done(vault, c, body),
        PLACEMENT_REMOVE => placement_remove(vault, c, body),
        PLACEMENTS => placements(vault, c, params),
        _ => Err(VaultError::not_found("no such vault route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => vr::refuse(&e),
    }
}

// ── plans and their confirm ─────────────────────────────────────────

/// A systemd-cred request: the route body, and the plan a confirm is
/// bound to (`/cli/vault/confirm/start|finish` with `systemdCred`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredPlan {
    #[serde(flatten)]
    pub sel: Sel,
    pub unit: Option<String>,
    pub cred_name: Option<String>,
    #[serde(default)]
    pub restart: bool,
    /// Re-place the latest version into every systemd credential of the
    /// secret that is behind (no unit).
    #[serde(default)]
    pub replace: bool,
    /// `place` | `replace` | `remove`. The routes set it from the path and
    /// `replace`; a confirm plan names it.
    pub op: Option<String>,
    /// The exact version the person approves (from the dry run). Place and
    /// replace refuse when the latest has moved (`version_changed`).
    pub version: Option<u32>,
    /// Replace only: the placement rows the person approves (from the dry
    /// run). Refused when they differ now (`plan_changed`).
    pub rows: Option<Vec<String>>,
    /// The value was not written by a person (an agent, a shell tab, K2
    /// itself, a guest): the person says so explicitly, after reviewing it.
    #[serde(default)]
    pub accept_agent_value: bool,
    #[serde(default)]
    pub dry_run: bool,
    pub confirm_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Place,
    Replace,
    Remove,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Place => "place",
            Op::Replace => "replace",
            Op::Remove => "remove",
        }
    }
}

/// Everything a confirm is bound to (reviews A-M2, A2-N2, B2-N1): the op,
/// the secret, the exact version, the unit, the cred name, the restart
/// flag, the "an agent wrote it" acknowledgement and, for a replace, the
/// rows. Any change needs a new confirm.
#[allow(clippy::too_many_arguments)]
pub fn plan_digest(op: Op, secret_id: &str, version: u32, unit: &str, cred: &str, restart: bool, accept_agent_value: bool, rows: &[String]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let v = version.to_string();
    let mut sorted = rows.to_vec();
    sorted.sort();
    let rows = sorted.join(",");
    for part in [op.as_str(), secret_id, &v, unit, cred, if restart { "1" } else { "0" }, if accept_agent_value { "1" } else { "0" }, &rows] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("systemd-cred:{hex}")
}

/// A person on Linux, and the `user` secret they name.
fn person_secret(vault: &LocalVault, c: &VaultCaller, sel: &Sel, door: &dyn CredsDoor) -> Result<SecretMeta, VaultError> {
    vr::human_admin(c)?;
    if !door.linux() {
        return Err(err(
            "linux_only",
            400,
            "systemd credentials are Linux only (systemd 250+); on this machine use k2 vault fetch, file or env delivery",
        ));
    }
    if sel.system {
        return Err(VaultError::forbidden("vault_system_namespace", "K2's own secrets are used only by K2"));
    }
    let meta = vault.meta(&vr::selector(c, sel)?, &c.actor)?;
    if meta.namespace != Namespace::User {
        return Err(VaultError::forbidden("vault_system_namespace", "K2's own secrets are used only by K2"));
    }
    Ok(meta)
}

/// The unit by its name rule (no deny list: `remove` may take a credential
/// off a unit that left the allow list later).
fn unit_of(p: &CredPlan) -> Result<String, VaultError> {
    let unit = p.unit.as_deref().map(str::trim).unwrap_or("");
    if unit.is_empty() {
        return Err(err("unit_required", 400, "name the unit: --unit <name>.service"));
    }
    if !unit_name_ok(unit) {
        return Err(err(
            "bad_unit",
            400,
            "a unit is <name>.service: letters, digits, _ . -, at most 128, not starting with . or -, no @ (templates and instances) and not ending in -",
        ));
    }
    Ok(unit.to_string())
}

/// The cred name a place asks for (default: the secret's name), by its rule.
fn cred_of(p: &CredPlan, m: &SecretMeta) -> Result<String, VaultError> {
    let cred = p.cred_name.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| m.name.clone());
    if let Some(pre) = reserved_prefix(&cred) {
        return Err(err(
            "bad_cred_name",
            400,
            format!("{pre}* is a systemd credential namespace (stock units act on it as root); pick another with --cred-name"),
        ));
    }
    if !cred_name_ok(&cred) {
        return Err(err(
            "bad_cred_name",
            400,
            "a cred name is letters, digits, _ . -, at most 64, not starting with . or - (pick one with --cred-name)",
        ));
    }
    Ok(cred)
}

fn op_of(path_remove: bool, p: &CredPlan) -> Result<Op, VaultError> {
    let op = if path_remove {
        Op::Remove
    } else if p.replace {
        Op::Replace
    } else {
        Op::Place
    };
    if let Some(named) = p.op.as_deref() {
        if named != op.as_str() {
            return Err(err("bad_op", 400, "op is place, replace or remove"));
        }
    }
    Ok(op)
}

/// The systemd_cred rows `remove` takes off (the given cred, or the unit's).
fn remove_rows(vault: &LocalVault, m: &SecretMeta, unit: &str, cred: Option<&str>) -> Result<Vec<Placement>, VaultError> {
    Ok(vault
        .placements(Some(&m.id))?
        .rows
        .into_iter()
        .filter(|p| {
            p.target == PlacementTarget::SystemdCred
                && p.unit.as_deref() == Some(unit)
                && cred.filter(|s| !s.is_empty()).is_none_or(|cn| p.cred_name.as_deref() == Some(cn))
        })
        .collect())
}

fn behind_rows(vault: &LocalVault, m: &SecretMeta) -> Result<Vec<Placement>, VaultError> {
    Ok(vault
        .placements(Some(&m.id))?
        .rows
        .into_iter()
        .filter(|p| p.target == PlacementTarget::SystemdCred && p.version > 0 && p.behind(m.latest_version))
        .collect())
}

fn row_ids(rows: &[Placement]) -> Vec<String> {
    let mut ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    ids.sort();
    ids
}

/// Who wrote a version, for the confirm text.
#[derive(Debug, Clone)]
pub struct Author {
    pub stamp: String,
    pub at: String,
    /// The owner or a Connect login typed it (not an agent, a K2 shell tab,
    /// K2 itself or an App guest).
    pub person: bool,
    pub text: String,
}

/// A person: the owner token or a Connect login. Agents, K2 shell tabs
/// (agent-level, KV15), K2 subsystems and App guests are not.
pub fn stamp_is_person(stamp: &str) -> bool {
    // "owner (rollback of v2 by agent:…)" is a rollback to a version no
    // person wrote (A3-P3): not a person's value.
    !stamp.contains(' ') && (stamp == "owner" || stamp.starts_with("connect:"))
}

fn stamp_words(stamp: &str) -> String {
    if let Some((who, rest)) = stamp.split_once(" (rollback of ") {
        let rest = rest.trim_end_matches(')');
        if let Some((v, orig)) = rest.split_once(" by ") {
            return format!("{} (a rollback to {v}, first written by {})", stamp_words(who), stamp_words(orig));
        }
    }
    if stamp == "owner" {
        return "the owner".into();
    }
    if let Some(u) = stamp.strip_prefix("connect:") {
        return format!("{u} (a person)");
    }
    if let Some(rest) = stamp.strip_prefix("agent:") {
        let ws = rest.split('/').next().unwrap_or(rest);
        return format!("an agent of workspace {}", crate::vault::workspace_label(ws));
    }
    if let Some(ws) = stamp.strip_prefix("shell:") {
        return format!("a K2 shell tab of workspace {}", crate::vault::workspace_label(ws));
    }
    if let Some(s) = stamp.strip_prefix("system:") {
        return format!("K2 itself ({s})");
    }
    if let Some(g) = stamp.strip_prefix("guest:") {
        return format!("App guest {g}");
    }
    stamp.to_string()
}

fn ago(at: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(at) else {
        return at.to_string();
    };
    let s = chrono::Utc::now().signed_duration_since(t.with_timezone(&chrono::Utc)).num_seconds().max(0);
    match s {
        0..=89 => "just now".into(),
        90..=5399 => format!("{} min ago", (s + 30) / 60),
        5400..=172_799 => format!("{} h ago", (s + 1800) / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

pub fn author_of(vault: &LocalVault, secret_id: &str, version: u32) -> Result<Author, VaultError> {
    let (stamp, at) = vault.version_author(secret_id, version)?;
    let person = stamp_is_person(&stamp);
    let text = format!("written by {}, {}", stamp_words(&stamp), ago(&at));
    Ok(Author { stamp, at, person, text })
}

/// (digest, text) for a plan, both derived by the daemon from the request
/// it will run. Place and replace name the version and who wrote it; a
/// value no person wrote leads with a warning.
fn plan_text(vault: &LocalVault, c: &VaultCaller, op: Op, p: &CredPlan, door: &dyn CredsDoor) -> Result<(String, String), VaultError> {
    let m = person_secret(vault, c, &p.sel, door)?;
    let version = p.version.unwrap_or(m.latest_version);
    // (what, warning): the warning leads the whole text.
    let who = |v: u32| -> Result<(String, &'static str), VaultError> {
        let a = author_of(vault, &m.id, v)?;
        let warn = if a.person { "" } else { "WARNING: no person wrote this value. " };
        Ok((format!("{} v{v} ({})", m.name, a.text), warn))
    };
    match op {
        Op::Place => {
            let unit = unit_of(p)?;
            let cred = cred_of(p, &m)?;
            let (what, warn) = who(version)?;
            let text = format!(
                "{warn}put {what} into {unit} as systemd credential {cred}{} (root reads it; only people may change it there)",
                if p.restart { ", then restart the unit" } else { "" }
            );
            Ok((plan_digest(op, &m.id, version, &unit, &cred, p.restart, p.accept_agent_value, &[]), text))
        }
        Op::Remove => {
            let unit = unit_of(p)?;
            let cred = p.cred_name.clone().unwrap_or_default();
            let shown = remove_rows(vault, &m, &unit, Some(&cred))?
                .first()
                .and_then(|r| r.cred_name.clone())
                .unwrap_or_else(|| cred.clone());
            let text = format!("take {}'s systemd credential {shown} off {unit}", m.name);
            Ok((plan_digest(op, &m.id, 0, &unit, &cred, false, false, &[]), text))
        }
        Op::Replace => {
            let rows = behind_rows(vault, &m)?;
            let ids = p.rows.clone().unwrap_or_else(|| row_ids(&rows));
            let units: Vec<String> = rows
                .iter()
                .map(|r| format!("{}{}", r.unit.clone().unwrap_or_default(), if r.restart { " (restart)" } else { "" }))
                .collect();
            let text = if units.is_empty() {
                format!("re-place {}: nothing is behind", m.name)
            } else {
                let (what, warn) = who(version)?;
                format!("{warn}re-place {what} into {}", units.join(", "))
            };
            Ok((plan_digest(op, &m.id, version, "", "", false, p.accept_agent_value, &ids), text))
        }
    }
}

/// For `/cli/vault/confirm/start|finish`: the digest the token binds to
/// and the daemon's own text. Place and replace must name their version.
pub fn confirm_binding(vault: &LocalVault, c: &VaultCaller, plan: &CredPlan) -> Result<(String, String), VaultError> {
    let op = match plan.op.as_deref() {
        Some("remove") => Op::Remove,
        Some("replace") => Op::Replace,
        Some("place") | None if !plan.replace => Op::Place,
        None => Op::Replace,
        Some(_) => return Err(err("bad_op", 400, "op is place, replace or remove")),
    };
    if op != Op::Remove && plan.version.is_none() {
        return Err(err("version_required", 400, "the plan names the version it places (run the dry run first)"));
    }
    with_door(|door| plan_text(vault, c, op, plan, door))
}

fn take_plan_proof(vault: &LocalVault, c: &VaultCaller, digest: &str, token: Option<&str>, name: &str, ingress: &str) -> Result<(), VaultError> {
    match vr::take_proof(vault, c, CONFIRM_ACTION, Some(digest), token) {
        Ok(_) => Ok(()),
        Err(e) => {
            crate::vault::auth_audit("vault-systemd-cred", &c.actor.stamp(), &format!("deny {name}: {}", e.code), ingress);
            Err(e)
        }
    }
}

/// Re-checks before any value goes onto a box: active, ceiling ≥ file.
fn placeable(m: &SecretMeta) -> Result<(), VaultError> {
    if m.status != SecretStatus::Active {
        return Err(VaultError::forbidden("secret_not_active", "that secret is disabled"));
    }
    if m.delivery_ceiling.level() < Delivery::File.level() {
        return Err(err(
            "delivery_above_ceiling",
            400,
            format!(
                "{} may go no further than {} delivery; a systemd credential is a file on the box. Raise it first: k2 vault delivery {} {} --ceiling file",
                m.name,
                m.delivery_ceiling.as_str(),
                m.name,
                m.default_delivery.as_str()
            ),
        ));
    }
    Ok(())
}

/// Place and replace (not a dry run): the person approved exactly this
/// version, and said so when no person wrote it.
fn approved_version(vault: &LocalVault, m: &SecretMeta, p: &CredPlan) -> Result<u32, VaultError> {
    let v = p.version.ok_or_else(|| err("version_required", 400, "name the version you approve (run the dry run first)"))?;
    if v != m.latest_version {
        return Err(VaultError::conflict(
            "version_changed",
            format!("{} is at v{} now, not the v{v} you approved; look again and confirm the new version", m.name, m.latest_version),
        ));
    }
    let a = author_of(vault, &m.id, v)?;
    if !a.person && !p.accept_agent_value {
        return Err(VaultError::conflict(
            "agent_written_value",
            format!(
                "{} v{v} was {}: no person wrote it, and it would run in a root unit. Review it first (k2 vault reveal {}), then add --accept-agent-value",
                m.name, a.text, m.name
            ),
        ));
    }
    Ok(v)
}

// ── systemd-cred ────────────────────────────────────────────────────

fn systemd_cred(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    let p: CredPlan = vr::parse(body)?;
    let op = op_of(false, &p)?;
    with_door(|door| {
        let (digest, text) = plan_text(vault, c, op, &p, door)?;
        let m = person_secret(vault, c, &p.sel, door)?;
        placeable(&m)?;
        let st = door.helper();
        let ready = helper_ready(&st);
        let author = author_of(vault, &m.id, p.version.unwrap_or(m.latest_version))?;
        if op == Op::Replace {
            return replace(vault, c, &p, door, &m, &digest, &text, ready, &author, ingress);
        }
        let unit = unit_of(&p)?;
        let cred = cred_of(&p, &m)?;
        if let Some(why) = deny_reason(&unit, &daemon_units(&st)) {
            return Err(err("unit_not_allowed", 400, format!("K2 won't put credentials on {unit}: it is {why}")));
        }
        let allowed = if ready { allowed_units(door) } else { None };
        if allowed.as_ref().is_some_and(|a| !a.iter().any(|u| *u == unit)) {
            return Err(err("unit_not_allowed", 400, format!("{unit} is not on this box's systemd-cred allow list: {}", admin_step(&unit))));
        }
        let plan = json!({
            "secret": m.name,
            "unit": unit,
            "credName": cred,
            "credPath": cred_path(&cred),
            "dropin": format!("/etc/systemd/system/{unit}.d/{}", dropin_file(&cred)),
            "dropinLine": format!("LoadCredentialEncrypted={cred}:{}", cred_path(&cred)),
            "restart": p.restart,
            "readAs": format!("$CREDENTIALS_DIRECTORY/{cred} (or %d/{cred} in the unit)"),
        });
        if p.dry_run {
            return Ok(json!({
                "ok": true, "dryRun": true, "plan": plan, "helperReady": ready,
                "allowed": allowed.as_ref().map(|a| a.iter().any(|u| *u == unit)),
                "confirmText": text, "version": m.latest_version,
                "author": author.stamp, "authorText": author.text, "authorIsPerson": author.person,
            }));
        }
        let version = approved_version(vault, &m, &p)?;
        if !ready {
            return Err(err("root_helper_missing", 503, manual_steps(&cred, &unit)));
        }
        let _g = place_lock();
        let all = vault.placements(None)?;
        if let Some(q) = all.rows.iter().find(|q| {
            q.target == PlacementTarget::SystemdCred && q.cred_name.as_deref() == Some(cred.as_str()) && q.secret_id != m.id
        }) {
            let other = vault.meta(&Selector::Id(q.secret_id.clone()), &c.actor).map(|o| o.name).unwrap_or_else(|_| "another secret".into());
            return Err(err("cred_name_taken", 409, format!("the cred name {cred} already holds {other}; pick another with --cred-name")));
        }
        take_plan_proof(vault, c, &digest, p.confirm_token.as_deref(), &m.name, ingress)?;
        let stamp = c.actor.stamp();
        let had_cred = all.rows.iter().any(|q| q.target == PlacementTarget::SystemdCred && q.cred_name.as_deref() == Some(cred.as_str()));
        let existing = all.rows.iter().any(|q| {
            q.target == PlacementTarget::SystemdCred && q.secret_id == m.id && q.unit.as_deref() == Some(unit.as_str()) && q.cred_name.as_deref() == Some(cred.as_str())
        });
        // The row first (pending, v0), so a failure half-way stays visible
        // to the doctor and removable (review A-L3 / B-M2).
        let pending = if existing { None } else { Some(vault.placement_record_cred(&m.id, &unit, &cred, p.restart, 0, &stamp)?) };
        let audit_fail = |e: &VaultError| {
            crate::vault::auth_audit("vault-systemd-cred", &stamp, &format!("failed {} -> {unit}: {}", m.name, e.code), ingress);
        };
        if let Err(e) = put_value(vault, door, &m, &cred, version) {
            // Nothing reached the box (the helper records after a write).
            if let Some(row) = &pending {
                let _ = vault.placement_remove(&row.id, &stamp);
            }
            audit_fail(&e);
            return Err(e);
        }
        if let Err(e) = door.call(&sv(&["creds", "bind", &unit, &cred]), None) {
            let mut hint = format!("root helper bind: {e}");
            if !had_cred {
                match door.call(&sv(&["creds", "remove", &cred]), None) {
                    Ok(_) => {
                        if let Some(row) = &pending {
                            let _ = vault.placement_remove(&row.id, &stamp);
                        }
                    }
                    Err(e2) => hint.push_str(&format!(
                        ". Taking the sealed credential back off also failed ({e2}); its placement row stays (pending) so k2 vault systemd-cred {} --unit {unit} --remove can clean it up",
                        m.name
                    )),
                }
            }
            let e = err("helper_failed", 502, hint);
            audit_fail(&e);
            return Err(e);
        }
        let row = vault.placement_record_cred(&m.id, &unit, &cred, p.restart, version, &stamp)?;
        let mut restarted = false;
        if p.restart {
            door.call(&sv(&["creds", "restart", &unit]), None).map_err(|e| {
                err(
                    "restart_failed",
                    502,
                    format!("the credential is in place and bound to {unit}, but its restart failed: {e}. Restart it yourself: systemctl restart {unit}"),
                )
            })?;
            restarted = true;
        }
        crate::vault::auth_audit(
            "vault-systemd-cred",
            &stamp,
            &format!("ok {} v{version} ({}) -> {unit} as {cred}{}", m.name, author.stamp, if restarted { " (restart queued)" } else { "" }),
            ingress,
        );
        Ok(json!({
            "ok": true,
            "plan": plan,
            "version": version,
            "restarted": restarted,
            "placement": row,
            "next": if restarted { Value::Null } else { json!(format!("the unit reads it at its next start: systemctl restart {unit}")) },
        }))
    })
}

#[allow(clippy::too_many_arguments)]
fn replace(
    vault: &LocalVault,
    c: &VaultCaller,
    p: &CredPlan,
    door: &dyn CredsDoor,
    m: &SecretMeta,
    digest: &str,
    text: &str,
    ready: bool,
    author: &Author,
    ingress: &str,
) -> Result<Value, VaultError> {
    let rows = behind_rows(vault, m)?;
    let ids = row_ids(&rows);
    if p.dry_run {
        return Ok(json!({
            "ok": true, "dryRun": true, "helperReady": ready, "confirmText": text, "behind": rows.len(),
            "rows": ids, "version": m.latest_version,
            "author": author.stamp, "authorText": author.text, "authorIsPerson": author.person,
        }));
    }
    if rows.is_empty() {
        return Ok(json!({ "ok": true, "replaced": [] }));
    }
    let version = approved_version(vault, m, p)?;
    let mut asked = p.rows.clone().unwrap_or_default();
    asked.sort();
    if asked != ids {
        return Err(VaultError::conflict("plan_changed", "the placements behind changed since you looked; run it again and confirm the new list"));
    }
    if !ready {
        return Err(err("root_helper_missing", 503, format!("update the root helper first (k2 root-helper update; protocol {CREDS_MIN_PROTOCOL})")));
    }
    take_plan_proof(vault, c, digest, p.confirm_token.as_deref(), &m.name, ingress)?;
    let _g = place_lock();
    let out = re_place(vault, door, m, &rows, &c.actor.stamp(), version);
    Ok(json!({ "ok": out.iter().all(|r| r["ok"] == true), "replaced": out }))
}

/// Resolve the value and `creds put` it (stdin), only if it is exactly
/// version `expect` (refused as `version_changed` otherwise). Re-checks
/// the secret first (active, ceiling ≥ file).
fn put_value(vault: &LocalVault, door: &dyn CredsDoor, m: &SecretMeta, cred: &str, expect: u32) -> Result<u32, VaultError> {
    let fresh = vault.meta(&Selector::Id(m.id.clone()), &Actor::System("systemd-cred".into())).unwrap_or_else(|_| m.clone());
    placeable(&fresh)?;
    let r = vault.resolve(&Selector::Id(m.id.clone()), Use::System, &Requester::System("systemd-cred".into()))?;
    if r.version != expect {
        drop(r);
        return Err(VaultError::conflict(
            "version_changed",
            format!("{} moved past v{expect} while placing; nothing was placed; look again", m.name),
        ));
    }
    let res = door.call(&sv(&["creds", "put", cred]), Some(r.value.expose()));
    let version = r.version;
    drop(r);
    res.map_err(|e| helper_err("put", e))?;
    Ok(version)
}

/// Re-put each row's credential at version `expect` (and queue a restart
/// for rows placed with `restart`). Only for a confirmed person or a listed
/// system actor.
fn re_place(vault: &LocalVault, door: &dyn CredsDoor, m: &SecretMeta, rows: &[Placement], who: &str, expect: u32) -> Vec<Value> {
    let mut out = vec![];
    for p in rows {
        let unit = p.unit.clone().unwrap_or_default();
        let cred = p.cred_name.clone().unwrap_or_default();
        match put_value(vault, door, m, &cred, expect).and_then(|v| vault.placement_mark(&p.id, v, p.restart, who).map(|_| v)) {
            Ok(v) => {
                let restart_error = if p.restart {
                    door.call(&sv(&["creds", "restart", &unit]), None).err()
                } else {
                    None
                };
                crate::vault::auth_audit("vault-systemd-cred", who, &format!("ok re-put {} v{v} -> {unit}", m.name), "daemon");
                out.push(json!({
                    "id": p.id, "unit": unit, "credName": cred, "ok": true, "version": v,
                    "restarted": p.restart && restart_error.is_none(), "restartError": restart_error,
                }));
            }
            Err(e) => {
                crate::vault::auth_audit("vault-systemd-cred", who, &format!("failed re-put {} -> {unit}: {}", m.name, e.code), "daemon");
                out.push(json!({ "id": p.id, "unit": unit, "credName": cred, "ok": false, "error": { "code": e.code, "hint": e.hint } }));
            }
        }
    }
    out
}

fn systemd_cred_remove(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    let p: CredPlan = vr::parse(body)?;
    let op = op_of(true, &p)?;
    with_door(|door| {
        let (digest, text) = plan_text(vault, c, op, &p, door)?;
        let m = person_secret(vault, c, &p.sel, door)?;
        let unit = unit_of(&p)?;
        let _g = place_lock();
        let rows = remove_rows(vault, &m, &unit, p.cred_name.as_deref())?;
        let Some(row) = rows.first().cloned() else {
            return Err(VaultError::not_found(format!("{} has no systemd credential on {unit}", m.name)));
        };
        let cred = row.cred_name.clone().unwrap_or_default();
        if !cred_name_ok(&cred) {
            return Err(VaultError::internal("that placement's cred name is not valid"));
        }
        if p.dry_run {
            return Ok(json!({ "ok": true, "dryRun": true, "confirmText": text, "unit": unit, "credName": cred }));
        }
        let st = door.helper();
        if !helper_ready(&st) {
            return Err(err(
                "root_helper_missing",
                503,
                format!(
                    "Update the root helper first (k2 root-helper update; protocol {CREDS_MIN_PROTOCOL}). By hand, as root: \
                     rm /etc/systemd/system/{unit}.d/{}; systemctl daemon-reload; rm {}. K2 keeps its record until it can remove it itself.",
                    dropin_file(&cred),
                    cred_path(&cred)
                ),
            ));
        }
        take_plan_proof(vault, c, &digest, p.confirm_token.as_deref(), &m.name, ingress)?;
        let stamp = c.actor.stamp();
        door.call(&sv(&["creds", "unbind", &unit, &cred]), None).map_err(|e| helper_err("unbind", e))?;
        let still_used = vault
            .placements(None)?
            .rows
            .iter()
            .any(|q| q.id != row.id && q.target == PlacementTarget::SystemdCred && q.cred_name.as_deref() == Some(cred.as_str()));
        if !still_used {
            door.call(&sv(&["creds", "remove", &cred]), None).map_err(|e| helper_err("remove", e))?;
        }
        vault.placement_remove(&row.id, &stamp)?;
        crate::vault::auth_audit("vault-systemd-cred", &stamp, &format!("ok remove {} from {unit} ({cred})", m.name), ingress);
        Ok(json!({
            "ok": true,
            "unit": unit,
            "credName": cred,
            "credRemoved": !still_used,
            "next": format!("the unit stops loading it at its next start: systemctl restart {unit}"),
        }))
    })
}

// ── rotation and rollback ───────────────────────────────────────────

/// K2 subsystems that may re-place a new version without a person (review
/// B2-N4). Empty: today no K2 subsystem rotates a secret that a root unit
/// reads. `carry`, `handoff`, `clone` and `adopt` write versions as System
/// and must never be here.
pub const REPLACE_SYSTEM_ACTORS: &[&str] = &[];

/// Who made a new version, for [`after_new_version`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replacer {
    /// A person with a fresh confirm for this change (rollback, bound to
    /// [`rollback_binding`]'s plan).
    ConfirmedPerson,
    /// Anyone else (rotate, set, a handoff): never re-places by itself.
    NotConfirmed,
}

fn system_may_replace(actor: &Actor) -> bool {
    matches!(actor, Actor::System(s) if REPLACE_SYSTEM_ACTORS.contains(&s.as_str()))
}

/// After a new version of `m`. Re-puts its behind systemd credentials only
/// for a non-passport Owner/Admin with a fresh confirm, or a K2 subsystem
/// named in [`REPLACE_SYSTEM_ACTORS`]. For anyone else the placements stay
/// stale (no helper call), the owner is told, and the answer says how a
/// person re-places them.
pub fn after_new_version(vault: &LocalVault, m: &SecretMeta, actor: &Actor, who: Replacer) -> Vec<Value> {
    let Ok(rows) = behind_rows(vault, m) else {
        return vec![];
    };
    if rows.is_empty() {
        return vec![];
    }
    let may = system_may_replace(actor) || (who == Replacer::ConfirmedPerson && actor.is_admin_human());
    if !may {
        let units: Vec<String> = rows.iter().filter_map(|r| r.unit.clone()).collect();
        notify_stale(m, actor, &units);
        return rows
            .iter()
            .map(|r| {
                json!({
                    "id": r.id, "unit": r.unit, "credName": r.cred_name, "ok": false, "stale": true,
                    "error": { "code": "placement_stale", "hint": format!(
                        "a new version reaches a root unit only with a person's confirm: k2 vault systemd-cred {} --replace (from your own terminal)", m.name) },
                })
            })
            .collect();
    }
    with_door(|door| {
        let st = if door.linux() { door.helper() } else { RootHelperState::missing() };
        if !(door.linux() && helper_ready(&st)) {
            return rows
                .iter()
                .map(|r| json!({ "id": r.id, "unit": r.unit, "credName": r.cred_name, "ok": false,
                    "error": { "code": "root_helper_missing", "hint": "the root helper can't re-place it (k2 root-helper update)" } }))
                .collect();
        }
        let _g = place_lock();
        let stamp = if matches!(actor, Actor::System(_)) { SYSTEM_ACTOR.to_string() } else { actor.stamp() };
        re_place(vault, door, m, &rows, &stamp, m.latest_version)
    })
}

/// A rollback body, as the confirm plan (`rollbackPlan`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackPlan {
    #[serde(flatten)]
    pub sel: Sel,
    pub version: Option<u32>,
    /// The target version was not written by a person: required for a
    /// secret root units load (A3-P3), and part of the confirm.
    #[serde(default)]
    pub accept_agent_value: bool,
}

/// A rollback of a secret that root units load (review A2-N3 / B2-N2):
/// the confirm is bound to the target version and the units, and its text
/// lists the units it re-places (and restarts) and who wrote the target
/// version. `None` when the secret has no systemd credential (the plain
/// rollback confirm, bound to the secret, stays).
pub fn rollback_binding(vault: &LocalVault, c: &VaultCaller, plan: &RollbackPlan) -> Result<Option<(String, String)>, VaultError> {
    vr::human_admin(c)?;
    let m = vault.meta(&vr::selector(c, &plan.sel)?, &c.actor)?;
    let target = plan.version.ok_or_else(|| err("version_required", 400, "--to <version>"))?;
    let rows: Vec<Placement> = vault
        .placements(Some(&m.id))?
        .rows
        .into_iter()
        .filter(|p| p.target == PlacementTarget::SystemdCred && p.version > 0)
        .collect();
    if rows.is_empty() {
        return Ok(None);
    }
    let a = author_of(vault, &m.id, target)?;
    let units: Vec<String> = rows
        .iter()
        .map(|r| format!("{}{}", r.unit.clone().unwrap_or_default(), if r.restart { " (restart)" } else { "" }))
        .collect();
    if !a.person && !plan.accept_agent_value {
        return Err(VaultError::conflict(
            "agent_written_value",
            format!(
                "v{target} of {} was {}: no person wrote it, and the rollback would put it into a root unit. Check who wrote it first (k2 vault audit {}), then add --accept-agent-value",
                m.name, a.text, m.name
            ),
        ));
    }
    let warn = if a.person { "" } else { "WARNING: no person wrote that version. " };
    let text = format!("{warn}roll {} back to v{target} ({}) and re-place it into {}", m.name, a.text, units.join(", "));
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for part in ["rollback", m.id.as_str(), &target.to_string(), &row_ids(&rows).join(","), if plan.accept_agent_value { "1" } else { "0" }] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(Some((format!("rollback:{hex}"), text)))
}

/// The owner hears that a new version did NOT reach a root unit.
fn notify_stale(m: &SecretMeta, actor: &Actor, units: &[String]) {
    let by = actor.stamp();
    crate::vault::auth_audit(
        "vault-systemd-cred",
        &by,
        &format!("stale {} v{} not re-placed into {}: needs a person (k2 vault systemd-cred {} --replace)", m.name, m.latest_version, units.join(", "), m.name),
        "daemon",
    );
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncProjects,
        json!({ "vaultNotice": { "secret": m.id, "name": m.name, "by": by, "version": m.latest_version, "placementsStale": units } }),
    );
}

// ── manual placements ───────────────────────────────────────────────

/// Bookkeeping rights on a secret's manual placements: an agent for its own
/// workspace's secrets only; a person for any secret they can see
/// (`vault.meta` already checked that).
fn may_book(c: &VaultCaller, m: &SecretMeta) -> Result<(), VaultError> {
    if m.namespace != Namespace::User {
        return Err(VaultError::forbidden("vault_system_namespace", "K2's own secrets are used only by K2"));
    }
    if let Some(a) = c.actor.agent() {
        if m.scope.kind == ScopeKind::Workspace && m.scope.id == a.workspace_id {
            return Ok(());
        }
        return Err(VaultError::forbidden(
            "not_your_workspace",
            "an agent keeps placement records only for its own workspace's secrets; ask the owner",
        ));
    }
    if c.actor.is_admin_human() || c.actor.is_member() {
        return Ok(());
    }
    Err(VaultError::forbidden("forbidden", "not allowed"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddBody {
    #[serde(flatten)]
    sel: Sel,
    #[serde(rename = "where")]
    where_text: Option<String>,
    note: Option<String>,
    /// Only `manual` here; `systemd_cred` rows come from systemd-cred.
    target: Option<String>,
}

fn placement_add(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: AddBody = vr::parse(body)?;
    if b.target.as_deref().is_some_and(|t| t != "manual") {
        return Err(VaultError::forbidden(
            "owner_only",
            "systemd credential placements are written by k2 vault systemd-cred (people only); placement add records manual ones",
        ));
    }
    let sel = vr::selector(c, &b.sel)?;
    let m = vault.meta(&sel, &c.actor)?;
    may_book(c, &m)?;
    let p = vault.placement_add(
        NewPlacement {
            secret_id: m.id.clone(),
            target: PlacementTarget::Manual,
            unit: None,
            cred_name: None,
            where_text: b.where_text.clone(),
            note: b.note.clone().unwrap_or_default(),
            restart: false,
        },
        &c.actor.stamp(),
    )?;
    Ok(json!({ "ok": true, "placement": placement_json(&p, &m) }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdBody {
    id: Option<String>,
}

fn row_for(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<(Placement, SecretMeta), VaultError> {
    let b: IdBody = vr::parse(body)?;
    let id = b.id.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| err("id_required", 400, "name the placement id (k2 vault placements)"))?;
    let p = vault.placement_get(id)?;
    // A placement of a secret the caller can't see is "not found".
    let m = vault
        .meta(&Selector::Id(p.secret_id.clone()), &c.actor)
        .map_err(|_| VaultError::not_found("no such placement"))?;
    may_book(c, &m)?;
    Ok((p, m))
}

fn placement_done(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let (p, m) = row_for(vault, c, body)?;
    if p.target != PlacementTarget::Manual {
        return Err(err(
            "placement_not_manual",
            400,
            format!("K2 re-places systemd credentials itself; to retry: k2 vault systemd-cred {} --unit {}", m.name, p.unit.clone().unwrap_or_default()),
        ));
    }
    let q = vault.placement_mark(&p.id, m.latest_version, false, &c.actor.stamp())?;
    Ok(json!({ "ok": true, "placement": placement_json(&q, &m) }))
}

fn placement_remove(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let (p, m) = row_for(vault, c, body)?;
    if p.target != PlacementTarget::Manual {
        return Err(err(
            "use_systemd_cred_remove",
            400,
            format!(
                "a systemd credential comes off the box with: k2 vault systemd-cred {} --unit {} --remove (people only)",
                m.name,
                p.unit.clone().unwrap_or_default()
            ),
        ));
    }
    vault.placement_remove(&p.id, &c.actor.stamp())?;
    Ok(json!({ "ok": true, "removed": p.id }))
}

// ── reads ───────────────────────────────────────────────────────────

/// `current` | `stale` (a systemd credential behind) | `due` (a manual one
/// behind) | `inactive` (the secret is disabled or destroyed).
fn state_of(p: &Placement, m: &SecretMeta) -> &'static str {
    if m.status != SecretStatus::Active {
        "inactive"
    } else if p.target == PlacementTarget::SystemdCred && p.version == 0 {
        "pending"
    } else if !p.behind(m.latest_version) {
        "current"
    } else if p.target == PlacementTarget::SystemdCred {
        "stale"
    } else {
        "due"
    }
}

pub fn placement_json(p: &Placement, m: &SecretMeta) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        o.insert("name".into(), json!(m.name));
        o.insert("latestVersion".into(), json!(m.latest_version));
        o.insert("secretStatus".into(), json!(m.status.as_str()));
        o.insert("state".into(), json!(state_of(p, m)));
    }
    v
}

/// One secret's placements for `k2 vault show`.
pub fn placements_for(vault: &LocalVault, m: &SecretMeta) -> Vec<Value> {
    vault.placements(Some(&m.id)).map(|l| l.rows.iter().map(|p| placement_json(p, m)).collect()).unwrap_or_default()
}

/// Every placement the caller may see, with its secret.
fn visible(vault: &LocalVault, c: &VaultCaller, secret_id: Option<&str>) -> Result<(Vec<(Placement, SecretMeta)>, usize), VaultError> {
    let l = vault.placements(secret_id)?;
    let mut metas: HashMap<String, Option<SecretMeta>> = HashMap::new();
    let mut out = vec![];
    for p in l.rows {
        let m = metas
            .entry(p.secret_id.clone())
            .or_insert_with(|| vault.meta(&Selector::Id(p.secret_id.clone()), &c.actor).ok())
            .clone();
        if let Some(m) = m {
            if c.actor.agent().is_some() && may_book(c, &m).is_err() {
                continue;
            }
            out.push((p, m));
        }
    }
    Ok((out, l.unknown))
}

fn placements(vault: &LocalVault, c: &VaultCaller, params: &HashMap<String, String>) -> Result<Value, VaultError> {
    let secret_id = if params.contains_key("name") || params.contains_key("id") || params.contains_key("ref") {
        let sel = vr::selector(c, &vr::sel_from_params(params))?;
        Some(vault.meta(&sel, &c.actor)?.id)
    } else {
        None
    };
    let (rows, unknown) = visible(vault, c, secret_id.as_deref())?;
    Ok(json!({
        "placements": rows.iter().map(|(p, m)| placement_json(p, m)).collect::<Vec<_>>(),
        "needNewerK2": unknown,
    }))
}

/// The doctor's placement findings for this caller.
pub fn doctor_findings(vault: &LocalVault, c: &VaultCaller) -> Vec<Value> {
    let Ok((rows, unknown)) = visible(vault, c, None) else {
        return vec![];
    };
    let mut out = vec![];
    for (p, m) in rows {
        let unit = p.unit.clone().unwrap_or_default();
        let place = p.unit.clone().or_else(|| p.where_text.clone()).unwrap_or_default();
        match state_of(&p, &m) {
            "inactive" => out.push(json!({
                "level": "warning",
                "code": "placement_of_inactive_secret",
                "secret": m.id,
                "placement": p.id,
                "message": if p.target == PlacementTarget::SystemdCred {
                    format!("{} is {} but its systemd credential is still on {unit}; take it off: k2 vault systemd-cred {} --unit {unit} --remove", m.name, m.status.as_str(), m.name)
                } else {
                    format!("{} is {} but is still placed at {place}; remove it there, then: k2 vault placement rm {}", m.name, m.status.as_str(), p.id)
                },
            })),
            "pending" => out.push(json!({
                "level": "warning",
                "code": "placement_stale",
                "secret": m.id,
                "placement": p.id,
                "message": format!("{}'s systemd credential on {unit} was never fully written (a failed placement); place it again or take it off: k2 vault systemd-cred {} --unit {unit} [--remove]", m.name, m.name),
            })),
            "stale" => out.push(json!({
                "level": "warning",
                "code": "placement_stale",
                "secret": m.id,
                "placement": p.id,
                "message": format!("{}'s systemd credential on {unit} holds v{} (latest v{}); a person re-places it: k2 vault systemd-cred {} --replace", m.name, p.version, m.latest_version, m.name),
            })),
            "due" => out.push(json!({
                "level": "warning",
                "code": "placement_manual_due",
                "secret": m.id,
                "placement": p.id,
                "message": format!("{} was rotated to v{}; put the new value at {place} by hand, then: k2 vault placement done {}", m.name, m.latest_version, p.id),
            })),
            _ => {}
        }
    }
    // Files a crash left in K2's places (review B2-N3): people only (the
    // list names root units), Linux with a current helper.
    if c.actor.is_admin_human() {
        with_door(|door| {
            if !door.linux() || !helper_ready(&door.helper()) {
                return;
            }
            let Some(v) = door.call(&sv(&["creds", "status"]), None).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
                return;
            };
            for o in v["orphans"].as_array().cloned().unwrap_or_default() {
                out.push(json!({
                    "level": "warning",
                    "code": "creds_orphan",
                    "message": format!(
                        "{} is in K2's systemd-credential places but no record names it (a crash between a write and its record); K2 won't touch it. As root: {}",
                        o["path"].as_str().unwrap_or("?"), o["cleanup"].as_str().unwrap_or("?")
                    ),
                }));
            }
        });
    }
    if unknown > 0 {
        out.push(json!({
            "level": "info",
            "code": "placements_need_newer_k2",
            "message": format!("{unknown} placement record(s) need a newer K2 to read"),
        }));
    }
    out
}

#[cfg(test)]
#[path = "vault_systemd_routes_tests.rs"]
mod tests;
