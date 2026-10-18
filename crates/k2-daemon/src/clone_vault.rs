//! Vault secrets that ride a clone (0.46.0, `prd-clone-carries-threads-and-vault-v1`
//! §6, slice A; amendments CL12–CL18, CL26, CL27).
//!
//! The source daemon builds a **plan**: every secret the agent created
//! (workspace scope) or uses (server secrets, or another workspace's,
//! granted to it or to one of its sessions, or named in its `.env.k2`),
//! each with a choice: **copy**, **blank** (a secret card on the clone's
//! Thread) or **skip**. Defaults (§6.2): sign-ins skip, API keys copy,
//! box-specific keys and shared secrets blank. Never offered: K2 logins,
//! App passes, engine credentials, chat (session) secrets, App-only
//! bindings, the system namespace (CL13).
//!
//! Any copy (and sending `.env` files as files, D4) needs a person's
//! **confirm** (`clone-carry`): Owner or Admin only (D1), bound to the
//! digest of the exact plan (`secretId = clone:<sha256>`, CL14), with the
//! Touch ID reason written by this daemon from the plan.
//!
//! **Export** seals the copy rows (plus names-only blank rows) into a
//! `k2-vault-carry/2` bound to the target vault, the clone id and a 1 h
//! expiry (CL26). **Import** opens it on the target, checks the clone id
//! against the unpacked manifest, writes each copied secret into the NEW
//! project's workspace scope by id (never a name, CL12), re-creates the
//! grants with their env/file names, rewrites `.env.k2` references, and
//! posts leave-blank cards (≤ 10, CL17; deferred while the clone has no
//! Thread of its own yet, CL18).
//!
//! Values never appear in a response, a log, the Thread or an audit line.
//! Audit lines both sides (`vault-carry`, names and counts only).
//!
//! Routes (`/cli/clone/vault/*`, POST, people only; agent passports never
//! reach `/cli/clone/`): `plan`, `confirm`, `export`, `import`, `copy`
//! (same box). `clone/bundle`, `clone/pack` and `clone/unpack` call the
//! same functions (see `clone_routes`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use k2_vault::carry::{CarryBlank, CarryGrant, CarryItem, CarryPayload};
use k2_vault::{
    Actor, Consumer, ConsumerKind, Grant, GrantRequest, Kind, ListFilter, LocalVault, Namespace, PutMode, PutRequest,
    Requester, Scope, ScopeKind, SecretMeta, SecretStatus, SecretStore, Selector, Use, VaultError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;

pub const PLAN: &str = "/cli/clone/vault/plan";
pub const CONFIRM: &str = "/cli/clone/vault/confirm";
pub const EXPORT: &str = "/cli/clone/vault/export";
pub const IMPORT: &str = "/cli/clone/vault/import";
pub const COPY: &str = "/cli/clone/vault/copy";
/// Every route here is POST-only.
pub const ROUTES: &[&str] = &[PLAN, CONFIRM, EXPORT, IMPORT, COPY];

/// The confirm action (`CONFIRM_ACTIONS`, CL14).
pub const CONFIRM_ACTION: &str = "clone-carry";
/// Leave-blank cards one clone may post (CL17); the rest go in one line.
pub const MAX_BLANK_CARDS: usize = 10;
/// Request bodies here are small (names and an opaque carry).
pub const MAX_BODY: usize = 8 * 1024 * 1024;

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

// ── plan ────────────────────────────────────────────────────────────

/// One secret's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Copy,
    Blank,
    Skip,
}

impl Choice {
    pub fn as_str(&self) -> &'static str {
        match self {
            Choice::Copy => "copy",
            Choice::Blank => "blank",
            Choice::Skip => "skip",
        }
    }
}

/// The destination vault, as its `GET /cli/vault/recipient` answered.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct VaultTarget {
    /// `age1…` carry recipient.
    pub recipient: String,
    /// The destination vault id the carry is bound to.
    pub vault_id: String,
    /// What people call the destination ("luzz.k2.dev", "this computer"):
    /// shown in the confirm reason, part of the digest.
    #[serde(default)]
    pub label: Option<String>,
}

/// What a clone asks of the vault: the body of `plan` / `confirm` /
/// `export` / `copy`, and the vault half of `clone/bundle` and `clone/pack`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CarryAsk {
    /// The source workspace (path on this box) …
    #[serde(default)]
    pub project_path: Option<String>,
    /// … or its id.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub vault_target: Option<VaultTarget>,
    /// The clone's id (the bundle manifest's `clone_id`). Required for any
    /// copy or blank row.
    #[serde(default)]
    pub clone_id: Option<String>,
    /// Per-secret choices by name; unnamed rows keep their default.
    #[serde(default)]
    pub secrets: BTreeMap<String, Choice>,
    /// "This is my own server/computer": keeps "given by a person" marks
    /// (VS20).
    #[serde(default)]
    pub own_box_move: bool,
    /// D4: the legacy "Include secrets (.env files)" tick (`carry_secrets`).
    #[serde(default, alias = "carry_secrets")]
    pub send_env_files: bool,
    #[serde(default)]
    pub confirm_token: Option<String>,
}

/// One secret the clone could take.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRow {
    pub name: String,
    pub kind: String,
    /// `workspace` (this agent's own), `server` (a server secret it uses)
    /// or `shared` (another workspace's secret granted to it).
    pub group: &'static str,
    pub default: Choice,
    pub choice: Choice,
    /// False for a Member caller (D1).
    pub copy_allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Env / file names the agent gets it under (grants).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_names: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub file_names: Vec<String>,
    #[serde(skip)]
    meta: SecretMeta,
    #[serde(skip)]
    grants: Vec<Grant>,
}

/// A secret the agent has that never travels, and why.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotCarried {
    pub name: String,
    pub kind: String,
    /// `app` | `login` | `session` | `shadowed`
    pub reason: &'static str,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub source_workspace_id: String,
    pub source_workspace: String,
    pub rows: Vec<PlanRow>,
    pub not_carried: Vec<NotCarried>,
    /// The caller can't copy (a Member, D1).
    pub member: bool,
    /// D4: `.env` files would travel as files (plaintext in the bundle).
    pub send_env_files: bool,
}

impl Plan {
    pub fn copies(&self) -> impl Iterator<Item = &PlanRow> {
        self.rows.iter().filter(|r| r.choice == Choice::Copy)
    }
    pub fn blanks(&self) -> impl Iterator<Item = &PlanRow> {
        self.rows.iter().filter(|r| r.choice == Choice::Blank)
    }
    /// Copying any value, or sending `.env` files, needs a person (§6.4, D4).
    pub fn needs_confirm(&self) -> bool {
        self.send_env_files || self.copies().next().is_some()
    }
    /// Something for the target vault (a copy or a blank card).
    pub fn has_carry(&self) -> bool {
        self.rows.iter().any(|r| r.choice != Choice::Skip)
    }
    fn names(&self, c: Choice) -> Vec<String> {
        self.rows.iter().filter(|r| r.choice == c).map(|r| r.name.clone()).collect()
    }
}

/// Kinds that never travel (CL13): K2 logins, App passes, engine creds.
pub fn never_carried_kind(k: Kind) -> bool {
    k2_vault::carry::never_carried_kind(k.as_str())
}

/// §6.2 defaults for one row.
fn default_for(group: &str, kind: Kind) -> (Choice, Option<&'static str>) {
    if group != "workspace" {
        return (Choice::Blank, Some("shared with other agents; copy lands it in the clone's own workspace scope"));
    }
    match kind {
        Kind::OauthToken => (Choice::Skip, Some("sign-ins skip by default; sign in again on the clone")),
        Kind::TlsKey | Kind::SigningKey => (Choice::Blank, Some("box-specific; left blank by default")),
        _ => (Choice::Copy, None),
    }
}

/// projects.id of a source workspace named by path or id.
pub fn source_project_id(ask: &CarryAsk) -> Result<String, VaultError> {
    if let Some(id) = ask.project_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return crate::vault::workspace_path(id)
            .map(|_| id.to_string())
            .ok_or_else(|| VaultError::not_found(format!("no workspace with id {id}")));
    }
    let p = ask.project_path.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let p = p.ok_or_else(|| VaultError::bad_request("project_required", "name the workspace (project_path or project_id)"))?;
    project_id_for_path(p).ok_or_else(|| VaultError::not_found("that folder isn't a registered workspace"))
}

/// projects.id of the registered workspace at `path` (canonical paths
/// compared, so a symlinked spelling matches).
pub fn project_id_for_path(path: &str) -> Option<String> {
    if let Some(id) = crate::canonical_session::lookup_project_id(path) {
        return Some(id);
    }
    let want = std::fs::canonicalize(path).ok()?;
    k2_core::projects_ops::projects_list()
        .ok()?
        .into_iter()
        .find(|p| std::fs::canonicalize(&p.path).map(|c| c == want).unwrap_or(false))
        .map(|p| p.id)
}

/// The workspace's published services (their grants are App bindings).
fn published_services(project_id: &str) -> HashSet<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = HashSet::new();
    if let Ok(mut st) = conn.prepare("SELECT id FROM published_services WHERE project_id = ?1") {
        if let Ok(rows) = st.query_map(rusqlite::params![project_id], |r| r.get::<_, String>(0)) {
            out.extend(rows.flatten());
        }
    }
    out
}

fn session_project(session_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace_session_handles::project_id_for_session_id(&conn, session_id).ok().flatten()
}

/// `k2v://` references in the workspace's `.env.k2` → secret ids.
fn env_k2_refs(vault: &LocalVault, project_id: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(dir) = crate::vault::workspace_path(project_id) else { return out };
    let Ok(text) = std::fs::read_to_string(Path::new(&dir).join(".env.k2")) else { return out };
    for (_, v) in crate::vault::publish_env::parse_env_k2(&text) {
        if !k2_vault::refs::VaultRef::is_ref(&v) {
            continue;
        }
        let Ok(sel) = crate::vault::selector_for_ref(&v) else { continue };
        if let Ok(m) = vault.meta(&sel, &Actor::System("clone".into())) {
            out.insert(m.id);
        }
    }
    out
}

/// Build the plan for the workspace `project_id`, as `caller` sees it,
/// with `ask.secrets` applied. Names only; no value is read.
pub fn build_plan(vault: &LocalVault, caller: &VaultCaller, project_id: &str, ask: &CarryAsk) -> Result<Plan, VaultError> {
    let sys = Actor::System("clone".into());
    let member = !caller.actor.is_admin_human();
    let services = published_services(project_id);
    let refs = env_k2_refs(vault, project_id);
    let all = vault.list(&ListFilter { include_system: false, scope: None, include_destroyed: false }, &sys)?;
    let mut rows: Vec<PlanRow> = vec![];
    let mut not_carried: Vec<NotCarried> = vec![];
    let mut later: Vec<(SecretMeta, Vec<Grant>, &'static str)> = vec![];
    for m in all {
        if m.namespace != Namespace::User || m.status != SecretStatus::Active {
            continue;
        }
        let grants = vault.grants(&Selector::Id(m.id.clone()), &sys)?;
        let to_ws = |g: &Grant| match g.consumer.kind {
            ConsumerKind::Workspace => g.consumer.id == project_id,
            ConsumerKind::Session => session_project(&g.consumer.id).as_deref() == Some(project_id),
            _ => false,
        };
        let ws_grants: Vec<Grant> = grants.iter().filter(|g| to_ws(g)).cloned().collect();
        let app_bound = grants.iter().any(|g| g.consumer.kind == ConsumerKind::PublishedService && services.contains(&g.consumer.id));
        let own = m.scope.kind == ScopeKind::Workspace && m.scope.id == project_id;
        let used = !ws_grants.is_empty() || refs.contains(&m.id);
        let group: &'static str = match m.scope.kind {
            _ if own => "workspace",
            ScopeKind::Server if used => "server",
            ScopeKind::Workspace if used => "shared",
            ScopeKind::Session if session_project(&m.scope.id).as_deref() == Some(project_id) => {
                not_carried.push(NotCarried {
                    name: m.name.clone(),
                    kind: m.kind.as_str().into(),
                    reason: "session",
                    note: "a chat's own secret; it ends with the chat".into(),
                });
                continue;
            }
            _ if app_bound => {
                not_carried.push(NotCarried {
                    name: m.name.clone(),
                    kind: m.kind.as_str().into(),
                    reason: "app",
                    note: "used by a published App; Apps don't travel with a clone yet".into(),
                });
                continue;
            }
            _ => continue,
        };
        if never_carried_kind(m.kind) {
            not_carried.push(NotCarried {
                name: m.name.clone(),
                kind: m.kind.as_str().into(),
                reason: "login",
                note: "K2 logins, App passes and engine credentials are made again on the target".into(),
            });
            continue;
        }
        if group == "workspace" {
            push_row(&mut rows, m, ws_grants, group, member);
        } else {
            later.push((m, ws_grants, group));
        }
    }
    // A used secret with the same name as one of the agent's own is
    // shadowed (both would land as the clone's workspace secret).
    for (m, g, group) in later {
        if rows.iter().any(|r| r.name == m.name) {
            not_carried.push(NotCarried {
                name: m.name.clone(),
                kind: m.kind.as_str().into(),
                reason: "shadowed",
                note: "the agent's own secret of this name travels instead".into(),
            });
            continue;
        }
        push_row(&mut rows, m, g, group, member);
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    for (name, choice) in &ask.secrets {
        let row = rows
            .iter_mut()
            .find(|r| &r.name == name)
            .ok_or_else(|| VaultError::bad_request("unknown_secret", format!("{name} isn't one of this agent's secrets (k2 clone preview lists them)")))?;
        if *choice == Choice::Copy && !row.copy_allowed {
            return Err(VaultError::forbidden("copy_needs_admin", "copying secrets needs an Owner or Admin; leave them blank or skip"));
        }
        row.choice = *choice;
    }
    if ask.send_env_files && member {
        return Err(VaultError::forbidden("copy_needs_admin", "sending .env files needs an Owner or Admin"));
    }
    Ok(Plan {
        source_workspace_id: project_id.to_string(),
        source_workspace: crate::vault::workspace_label(project_id),
        rows,
        not_carried,
        member,
        send_env_files: ask.send_env_files,
    })
}

fn push_row(rows: &mut Vec<PlanRow>, m: SecretMeta, grants: Vec<Grant>, group: &'static str, member: bool) {
    let (mut default, note) = default_for(group, m.kind);
    if member && default == Choice::Copy {
        default = Choice::Blank;
    }
    rows.push(PlanRow {
        name: m.name.clone(),
        kind: m.kind.as_str().into(),
        group,
        default,
        choice: default,
        copy_allowed: !member,
        note: if member { Some("copying secrets needs an Owner or Admin".into()) } else { note.map(str::to_string) },
        env_names: grants.iter().filter_map(|g| g.env_name.clone()).collect(),
        file_names: grants.iter().filter_map(|g| g.file_name.clone()).collect(),
        meta: m,
        grants,
    });
}

// ── confirm (§6.4, CL14) ────────────────────────────────────────────

fn target_fingerprint(t: Option<&VaultTarget>) -> Result<String, VaultError> {
    match t {
        Some(t) => Ok(k2_vault::agefmt::Recipient::parse(&t.recipient)?.fingerprint()),
        None => Ok(String::new()),
    }
}

/// `sha256` of the exact plan a confirm is for: source, target vault and
/// key, clone id, `.env` files, and every row's (name, choice).
pub fn plan_digest(plan: &Plan, target: Option<&VaultTarget>, clone_id: Option<&str>) -> Result<String, VaultError> {
    let mut choices: Vec<(String, &str)> = plan.rows.iter().map(|r| (r.name.clone(), r.choice.as_str())).collect();
    choices.sort();
    let canon = json!({
        "v": 1,
        "sourceProjectId": plan.source_workspace_id,
        "targetVaultId": target.map(|t| t.vault_id.as_str()).unwrap_or(""),
        "targetFingerprint": target_fingerprint(target)?,
        "targetLabel": target.and_then(|t| t.label.as_deref()).unwrap_or(""),
        "cloneId": clone_id.unwrap_or(""),
        "sendEnvFiles": plan.send_env_files,
        "choices": choices,
    });
    Ok(k2_vault::crypto::hex(&k2_vault::crypto::sha256(canon.to_string().as_bytes())))
}

/// `clone:<digest>`: the confirm's `secretId`.
pub fn confirm_secret_id(digest: &str) -> String {
    format!("clone:{digest}")
}

/// The Touch ID reason, written by the daemon from the plan (never the
/// client's label).
pub fn confirm_reason(plan: &Plan, target: Option<&VaultTarget>) -> String {
    let n = plan.copies().count();
    let to = target
        .and_then(|t| t.label.as_deref())
        .map(|l| l.chars().filter(|c| !c.is_control()).take(48).collect::<String>())
        .unwrap_or_else(|| "the clone".into());
    let mut parts = vec![];
    if n > 0 {
        parts.push(format!("copy {n} secret{} from {} to {to}", if n == 1 { "" } else { "s" }, plan.source_workspace));
    }
    if plan.send_env_files {
        parts.push(format!("send {}'s .env files as files", plan.source_workspace));
    }
    parts.join(" and ")
}

/// The plan for an ask. A folder that isn't a registered workspace has no
/// vault secrets; it can still send `.env` files (D4), so it gets a plan
/// with no rows, keyed by its canonical path.
///
/// Without a destination vault (`vault_target`) no secret can travel, so
/// every row not named in `ask.secrets` is `skip` (its `default` still
/// shows what a carry would do); a named copy is refused later
/// (`target_required`).
pub fn plan_for(vault: &LocalVault, caller: &VaultCaller, ask: &CarryAsk) -> Result<Plan, VaultError> {
    match source_project_id(ask) {
        Ok(pid) => {
            let mut plan = build_plan(vault, caller, &pid, ask)?;
            if ask.vault_target.is_none() {
                for r in plan.rows.iter_mut().filter(|r| !ask.secrets.contains_key(&r.name)) {
                    r.choice = Choice::Skip;
                }
            }
            Ok(plan)
        }
        Err(e) if ask.vault_target.is_none() && ask.secrets.is_empty() && ask.project_id.is_none() => {
            let Some(path) = ask.project_path.as_deref().and_then(|p| std::fs::canonicalize(p).ok()) else { return Err(e) };
            let member = !caller.actor.is_admin_human();
            if ask.send_env_files && member {
                return Err(VaultError::forbidden("copy_needs_admin", "sending .env files needs an Owner or Admin"));
            }
            Ok(Plan {
                source_workspace_id: format!("path:{}", path.display()),
                source_workspace: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                rows: vec![],
                not_carried: vec![],
                member,
                send_env_files: ask.send_env_files,
            })
        }
        Err(e) => Err(e),
    }
}

/// The confirm binding for an ask: `(secretId, reason)`. Used by
/// `/cli/vault/confirm/finish` (action `clone-carry` with `clonePlan`)
/// and `/cli/clone/vault/confirm`.
pub fn confirm_binding(vault: &LocalVault, caller: &VaultCaller, ask: &CarryAsk) -> Result<(String, String, Plan), VaultError> {
    let plan = plan_for(vault, caller, ask)?;
    if !plan.needs_confirm() {
        return Err(VaultError::bad_request("nothing_to_confirm", "no secret is set to copy and no .env files are sent"));
    }
    let digest = plan_digest(&plan, ask.vault_target.as_ref(), ask.clone_id.as_deref())?;
    let reason = confirm_reason(&plan, ask.vault_target.as_ref());
    Ok((confirm_secret_id(&digest), reason, plan))
}

/// Consume the confirm for `plan` when it needs one (any copy, or `.env`
/// files). Owner or Admin people only (D1); passports never.
pub fn take_confirm(vault: &LocalVault, caller: &VaultCaller, plan: &Plan, ask: &CarryAsk) -> Result<Option<String>, VaultError> {
    if !plan.needs_confirm() {
        return Ok(None);
    }
    if caller.is_passport() || !caller.actor.is_admin_human() {
        return Err(VaultError::forbidden("copy_needs_admin", "copying secrets needs an Owner or Admin"));
    }
    let digest = plan_digest(plan, ask.vault_target.as_ref(), ask.clone_id.as_deref())?;
    let token = ask
        .confirm_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| VaultError::forbidden("vault_confirm_required", "copying secrets needs a confirm first (Touch ID, the vault password or your Connect password)"))?;
    match vault.confirm.take(token, CONFIRM_ACTION, Some(&confirm_secret_id(&digest)), &caller.actor.stamp()) {
        Ok(k2_vault::HumanProof::Confirmed { method }) => Ok(Some(method)),
        Ok(k2_vault::HumanProof::None) => Err(VaultError::forbidden("vault_confirm_required", "confirm again")),
        Err(e) => {
            crate::vault::auth_audit("vault-carry", &caller.actor.stamp(), &format!("deny clone export: {}", e.code), "-");
            Err(e)
        }
    }
}

// ── export (source side) ────────────────────────────────────────────

/// The source side's answer: the opaque carry (when one was sealed) and a
/// names-only report.
#[derive(Debug)]
pub struct Exported {
    pub carry_b64: Option<String>,
    pub report: Value,
}

fn report_base(plan: &Plan) -> Value {
    json!({
        "copied": plan.names(Choice::Copy),
        "blank": plan.names(Choice::Blank),
        "skipped": plan.names(Choice::Skip),
        "notCarried": plan.not_carried,
    })
}

/// Seal the plan's copy rows (values) and blank rows (names only) for the
/// target: `k2-vault-carry/2`, bound to the target vault, `clone_id` and
/// a 1 h expiry. The confirm must already be taken (`take_confirm`).
pub fn export(
    vault: &LocalVault,
    caller: &VaultCaller,
    plan: &Plan,
    target: &VaultTarget,
    clone_id: &str,
    own_box_move: bool,
    method: Option<&str>,
) -> Result<Exported, VaultError> {
    if plan.needs_confirm() && plan.copies().next().is_some() && method.is_none() {
        return Err(VaultError::forbidden("vault_confirm_required", "copying secrets needs a confirm first"));
    }
    if clone_id.trim().is_empty() {
        return Err(VaultError::bad_request("clone_id_required", "the clone id (the bundle manifest's clone_id)"));
    }
    let recipient = k2_vault::agefmt::Recipient::parse(&target.recipient)?;
    if target.vault_id.trim().is_empty() {
        return Err(VaultError::bad_request("target_required", "the target vault id (k2 vault recipient on the target)"));
    }
    let mut report = report_base(plan);
    if !plan.has_carry() {
        report["sealed"] = json!(false);
        report["reason"] = json!("no_secrets");
        return Ok(Exported { carry_b64: None, report });
    }
    let mut items = vec![];
    for r in plan.copies() {
        let v = vault.resolve(&Selector::Id(r.meta.id.clone()), Use::System, &Requester::System("clone".into()))?;
        items.push(CarryItem {
            scope_kind: "workspace".into(),
            scope_label: plan.source_workspace.clone(),
            scope_id: Some(plan.source_workspace_id.clone()),
            name: r.name.clone(),
            kind: r.kind.clone(),
            hosts: r.meta.hosts.clone(),
            inject: r.meta.inject.clone(),
            default_delivery: r.meta.default_delivery.as_str().into(),
            delivery_ceiling: r.meta.delivery_ceiling.as_str().into(),
            description: r.meta.description.clone(),
            human_given: r.meta.human_given,
            value_b64: CarryItem::encode_value(v.value.expose()),
            source_scope: Some(r.meta.scope.kind.as_str().into()),
            grants: r
                .grants
                .iter()
                .map(|g| CarryGrant {
                    delivery: g.delivery.as_str().into(),
                    env_name: g.env_name.clone(),
                    file_name: g.file_name.clone(),
                    hosts_narrowed: g.hosts_narrowed.clone(),
                })
                .collect(),
        });
    }
    let blank: Vec<CarryBlank> = plan
        .blanks()
        .map(|r| CarryBlank {
            name: r.name.clone(),
            kind: r.kind.clone(),
            hosts: r.meta.hosts.clone(),
            delivery: r.meta.default_delivery.as_str().into(),
            env_names: r.env_names.clone(),
            source_scope: r.meta.scope.kind.as_str().into(),
        })
        .collect();
    let n = items.len();
    let file = vault.carry_seal_clone(
        &recipient,
        &target.vault_id,
        own_box_move,
        k2_vault::carry::CloneBinding {
            clone_id: clone_id.to_string(),
            source_workspace_id: plan.source_workspace_id.clone(),
            source_label: plan.source_workspace.clone(),
            ttl_secs: k2_vault::carry::CLONE_CARRY_TTL_SECS,
        },
        items,
        blank,
    )?;
    crate::vault::auth_audit(
        "vault-carry",
        &caller.actor.stamp(),
        &format!(
            "ok clone export {n} (blank {}) to {} via {} clone={clone_id}",
            plan.blanks().count(),
            recipient.fingerprint(),
            method.unwrap_or("none"),
        ),
        "-",
    );
    report["sealed"] = json!(true);
    report["cloneId"] = json!(clone_id);
    report["targetFingerprint"] = json!(recipient.fingerprint());
    Ok(Exported { carry_b64: Some(base64::engine::general_purpose::STANDARD.encode(&file)), report })
}

/// A clone request's vault half, checked before the bundle is built:
/// the plan, and the confirm taken (so a missing confirm refuses the whole
/// request and nothing is sealed or bundled).
pub struct Prepared {
    pub plan: Plan,
    pub ask: CarryAsk,
    pub clone_id: String,
    pub method: Option<String>,
}

/// `None` when the request asks nothing of the vault.
pub fn prepare(ask: &CarryAsk, caller: Option<&VaultCaller>) -> Result<Option<Prepared>, CliResponse> {
    if ask.vault_target.is_none() && !ask.send_env_files && ask.secrets.is_empty() {
        return Ok(None);
    }
    let fail = |e: VaultError| crate::vault_routes::refuse(&e);
    let caller = caller.ok_or_else(|| fail(VaultError::new("unauthorized", 401, "couldn't tell who asked")))?;
    if caller.is_passport() {
        return Err(fail(VaultError::forbidden("people_only", "agents propose clones; a person runs them")));
    }
    let vault = crate::vault::get().map_err(fail)?;
    let plan = plan_for(&vault, caller, ask).map_err(fail)?;
    let given = ask.clone_id.clone().filter(|s| !s.trim().is_empty());
    let copies = plan.copies().next().is_some();
    // The digest binds the clone id, so a copy needs the id the confirm
    // was made for; and a copy needs somewhere to go. Checked before the
    // confirm is taken, so a bad request doesn't burn it.
    if copies && given.is_none() {
        return Err(fail(VaultError::bad_request("clone_id_required", "send the clone_id the confirm was made for")));
    }
    if copies && ask.vault_target.is_none() {
        return Err(fail(VaultError::bad_request("target_required", "copying secrets needs the destination vault (vault_target)")));
    }
    let method = take_confirm(&vault, caller, &plan, ask).map_err(fail)?;
    let clone_id = given.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut ask = ask.clone();
    ask.clone_id = Some(clone_id.clone());
    Ok(Some(Prepared { plan, ask, clone_id, method }))
}

/// A prepared carry handed to the pull pack's worker thread.
pub struct PackCarry {
    pub prepared: Prepared,
    pub caller: Option<VaultCaller>,
}

/// Seal a prepared request (after the bundle was built). Never fails the
/// clone: a problem becomes the report's `error`.
pub fn seal_prepared(p: &Prepared, caller: Option<&VaultCaller>) -> Exported {
    let Some(target) = p.ask.vault_target.as_ref() else {
        let mut report = report_base(&p.plan);
        report["sealed"] = json!(false);
        report["reason"] = json!("no_target");
        return Exported { carry_b64: None, report };
    };
    let (Some(caller), Ok(vault)) = (caller, crate::vault::get()) else {
        return Exported { carry_b64: None, report: json!({ "sealed": false, "reason": "vault_not_open" }) };
    };
    match export(&vault, caller, &p.plan, target, &p.clone_id, p.ask.own_box_move, p.method.as_deref()) {
        Ok(x) => x,
        Err(e) => Exported { carry_b64: None, report: json!({ "sealed": false, "error": { "code": e.code, "hint": e.hint } }) },
    }
}

// ── import (target side) ────────────────────────────────────────────

/// Open a clone carry for `project_id` and write it in. Never fails the
/// caller; the outcome is `{vault_imported, vault_skipped, ...}` (names
/// only) with `vault_error` when nothing could be done.
pub fn import_into_project(
    vault: Result<Arc<LocalVault>, VaultError>,
    caller: Option<&VaultCaller>,
    carry_b64: &str,
    project_id: &str,
    manifest_clone_id: Option<&str>,
) -> Value {
    let fail = |code: &str, hint: &str| {
        json!({ "vault_imported": [], "vault_skipped": [], "vault_error": { "code": code, "hint": hint } })
    };
    let Some(c) = caller else {
        return fail("unauthorized", "Couldn't tell who asked, so the vault secrets weren't added.");
    };
    if c.is_passport() {
        return fail("people_only", "Agents don't land clones.");
    }
    let vault = match vault {
        Ok(v) => v,
        Err(e) => return fail(e.code, "The vault on this server isn't open, so the carried secrets weren't added."),
    };
    match import(&vault, c, carry_b64, project_id, manifest_clone_id) {
        Ok(v) => v,
        Err(e) => {
            crate::vault::auth_audit("vault-carry", &c.actor.stamp(), &format!("deny clone import: {}", e.code), "-");
            fail(e.code, &e.hint)
        }
    }
}

/// [`import_into_project`] with errors.
pub fn import(vault: &LocalVault, caller: &VaultCaller, carry_b64: &str, project_id: &str, manifest_clone_id: Option<&str>) -> Result<Value, VaultError> {
    if crate::vault::workspace_path(project_id).is_none() {
        return Err(VaultError::not_found(format!("no workspace with id {project_id}")));
    }
    let want = manifest_clone_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VaultError::forbidden("carry_clone_mismatch", "this bundle has no clone id, so no vault carry applies to it"))?;
    let file = base64::engine::general_purpose::STANDARD
        .decode(carry_b64.trim())
        .map_err(|_| VaultError::bad_request("bad_carry", "the vault carry wasn't readable"))?;
    let p = vault.carry_open_clone(&file, want)?;
    apply(vault, caller, &p, project_id)
}

fn apply(vault: &LocalVault, caller: &VaultCaller, p: &CarryPayload, project_id: &str) -> Result<Value, VaultError> {
    let sys = Actor::System("clone".into());
    let mut imported = vec![];
    let mut skipped = vec![];
    let mut grants = 0usize;
    let mut copied_ws: HashSet<String> = HashSet::new();
    let mut copied_server: HashSet<String> = HashSet::new();
    for i in &p.items {
        if k2_vault::carry::never_carried_kind(&i.kind) || i.scope_kind == "session" {
            skipped.push(format!("{}: never carried", i.name));
            continue;
        }
        let mut req = PutRequest::new(Namespace::User, Scope::workspace(project_id), i.name.clone(), Kind::parse(&i.kind)?);
        req.hosts = i.hosts.clone();
        req.inject = i.inject.clone();
        req.default_delivery = Some(k2_vault::Delivery::parse(&i.default_delivery)?);
        req.delivery_ceiling = Some(k2_vault::Delivery::parse(&i.delivery_ceiling)?);
        req.description = i.description.clone();
        req.origin = format!("import:clone:{}", p.source_vault_id);
        req.mode = PutMode::Create;
        // VS20: person-given only survives the owner's own move.
        req.human_given_override = Some(i.human_given && p.own_box_move);
        let meta = match vault.put(req, i.value()?, &sys) {
            Ok(o) => o.meta,
            Err(e) if e.code == "secret_exists" => {
                skipped.push(format!("{}: exists (kept this workspace's)", i.name));
                continue;
            }
            Err(e) => {
                skipped.push(format!("{}: {}", i.name, e.code));
                continue;
            }
        };
        for g in &i.grants {
            let delivery = k2_vault::Delivery::parse(&g.delivery)?;
            let r = vault.grant(
                GrantRequest {
                    selector: Selector::Id(meta.id.clone()),
                    consumer: Consumer::workspace(project_id),
                    delivery,
                    env_name: g.env_name.clone(),
                    file_name: g.file_name.clone(),
                    hosts_narrowed: g.hosts_narrowed.clone(),
                    expires_at: None,
                    version: None,
                },
                &sys,
            );
            match r {
                Ok(_) => grants += 1,
                Err(e) => skipped.push(format!("{}: grant {}: {}", i.name, g.env_name.as_deref().or(g.file_name.as_deref()).unwrap_or("-"), e.code)),
            }
        }
        if i.source_scope.as_deref() == Some("server") {
            copied_server.insert(i.name.clone());
        } else {
            copied_ws.insert(i.name.clone());
        }
        imported.push(i.name.clone());
    }
    let label = crate::vault::workspace_label(project_id);
    let source_refs: Vec<String> = [p.source_label.clone(), p.source_workspace_id.clone()].into_iter().flatten().collect();
    let env_k2 = crate::vault::workspace_path(project_id)
        .map(|dir| rewrite_env_k2(Path::new(&dir), &source_refs, &copied_ws, &copied_server, &label))
        .unwrap_or_default();
    let cards = post_blank_cards(project_id, &p.blank, p.source_label.as_deref().unwrap_or("the source"));
    crate::vault::auth_audit(
        "vault-carry",
        &caller.actor.stamp(),
        &format!(
            "ok clone import {} from {} into {project_id} clone={}",
            imported.len(),
            p.source_vault_id,
            p.clone_id.as_deref().unwrap_or("-")
        ),
        "-",
    );
    Ok(json!({
        "vault_imported": imported,
        "vault_skipped": skipped,
        "vault_grants": grants,
        "vault_env_k2_rewritten": env_k2.rewritten,
        "vault_env_k2_left": env_k2.left,
        "vault_cards_posted": cards.posted,
        "vault_cards_deferred": cards.deferred,
        "vault_cards_overflow": cards.overflow,
    }))
}

#[derive(Default)]
struct EnvK2 {
    rewritten: Vec<String>,
    left: Vec<String>,
}

/// `.env.k2` in the clone: `k2v://ws/<source>/NAME` (and `k2v://server/NAME`
/// for a copied server secret) → `k2v://ws/<clone>/NAME` for names that
/// were copied. Other references stay and are reported (by key).
fn rewrite_env_k2(dir: &Path, source_refs: &[String], copied_ws: &HashSet<String>, copied_server: &HashSet<String>, label: &str) -> EnvK2 {
    let mut out = EnvK2::default();
    let path = dir.join(".env.k2");
    let Ok(text) = std::fs::read_to_string(&path) else { return out };
    let mut changed = false;
    let mut lines = vec![];
    for line in text.lines() {
        let t = line.trim().trim_start_matches("export ");
        let parsed = (!t.starts_with('#')).then(|| t.split_once('=')).flatten();
        let Some((k, v)) = parsed else {
            lines.push(line.to_string());
            continue;
        };
        let raw = v.trim().trim_matches('"').trim_matches('\'');
        let Ok(r) = k2_vault::refs::VaultRef::parse(raw) else {
            lines.push(line.to_string());
            continue;
        };
        let hit = match r.scope_kind {
            ScopeKind::Workspace => source_refs.iter().any(|s| s == &r.scope_ref) && copied_ws.contains(&r.name),
            ScopeKind::Server => copied_server.contains(&r.name),
            _ => false,
        };
        if hit {
            let nr = k2_vault::refs::VaultRef { scope_kind: ScopeKind::Workspace, scope_ref: label.to_string(), name: r.name.clone(), version: None };
            let prefix = if line.trim_start().starts_with("export ") { "export " } else { "" };
            lines.push(format!("{prefix}{}={}", k.trim(), nr.render()));
            out.rewritten.push(k.trim().to_string());
            changed = true;
        } else {
            out.left.push(k.trim().to_string());
            lines.push(line.to_string());
        }
    }
    if changed {
        let mut body = lines.join("\n");
        if text.ends_with('\n') {
            body.push('\n');
        }
        if let Err(e) = std::fs::write(&path, body) {
            k2_core::log_debug!("[clone-vault] .env.k2 rewrite failed: {e}");
        }
    }
    out
}

// ── leave blank → secret cards (§6.6, CL17, CL18) ───────────────────

#[derive(Default, Serialize, Deserialize)]
struct Cards {
    posted: Vec<String>,
    deferred: Vec<String>,
    overflow: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeferredCards {
    source: String,
    blank: Vec<CarryBlank>,
}

fn deferred_path(project_id: &str) -> PathBuf {
    let safe: String = project_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    k2_core::paths::k2_home().join("clone-vault").join("pending-cards").join(format!("{safe}.json"))
}

/// The clone's own canonical Thread id, if it has one that no other
/// workspace owns (same box: the pin can still be the source's, CL11).
fn own_canonical_conversation(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let pin = k2_core::db::schema::WorkspaceSession::get(&conn, project_id)
        .ok()
        .flatten()
        .and_then(|s| s.session_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    match k2_core::overlay::catalog::get(&conn, &pin) {
        Ok(Some((owner, _, _))) if owner != project_id => None,
        Ok(_) => Some(pin),
        Err(_) => None,
    }
}

thread_local! {
    static DEFER_CARDS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with leave-blank cards kept for later instead of posted: the
/// clone unpack imports the vault carry before the Threads (which need
/// the copied names) and posts the cards after them, so the cards come
/// after the imported history.
pub fn with_cards_deferred<R>(f: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            DEFER_CARDS.with(|d| d.set(false));
        }
    }
    DEFER_CARDS.with(|d| d.set(true));
    let _reset = Reset;
    f()
}

/// Post one secret card per blank row on the clone's Thread (≤ 10; the
/// rest in one line). Without a Thread of its own yet, keep them (names
/// only) for [`post_deferred_cards`].
fn post_blank_cards(project_id: &str, blank: &[CarryBlank], source: &str) -> Cards {
    let mut cards = Cards::default();
    if blank.is_empty() {
        return cards;
    }
    let conv = if DEFER_CARDS.with(|d| d.get()) { None } else { own_canonical_conversation(project_id) };
    let Some(conv) = conv else {
        let d = DeferredCards { source: source.to_string(), blank: blank.to_vec() };
        let p = deferred_path(project_id);
        let ok = p.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
            && serde_json::to_vec(&d).ok().is_some_and(|b| k2_core::fs_atomic::atomic_write_private(&p, &b).is_ok());
        if !ok {
            k2_core::log_debug!("[clone-vault] couldn't keep {} blank card(s) for later", blank.len());
        }
        cards.deferred = blank.iter().map(|b| b.name.clone()).collect();
        return cards;
    };
    let addr = crate::vault::workspace_label(project_id);
    for (i, b) in blank.iter().enumerate() {
        if i >= MAX_BLANK_CARDS {
            cards.overflow.push(b.name.clone());
            continue;
        }
        let prompt = format!("Carried over from {source}: please add this secret");
        let r = crate::overlay_routes::post_clone_secret_card(
            &conv,
            project_id,
            &addr,
            &b.name,
            &b.kind,
            &b.hosts,
            &b.delivery,
            &prompt.chars().take(200).collect::<String>(),
        );
        match r {
            Ok(_) => cards.posted.push(b.name.clone()),
            Err(e) => {
                k2_core::log_debug!("[clone-vault] card for {} not posted: {e}", b.name);
                cards.overflow.push(b.name.clone());
            }
        }
    }
    if !cards.overflow.is_empty() {
        let body = format!(
            "Also left blank: {}. Add each with `k2 vault set NAME --scope ws:{addr}`.",
            cards.overflow.join(", ")
        );
        crate::overlay_routes::post_clone_text_line(&conv, project_id, &addr, &body);
    }
    cards
}

/// Post leave-blank cards kept while the clone had no Thread of its own
/// (CL18). Slice B's pending-canonical settle and its boot sweep call this
/// once the clone has a pin; a no-op when nothing is kept.
pub fn post_deferred_cards(project_id: &str) -> Vec<String> {
    let p = deferred_path(project_id);
    let Ok(bytes) = std::fs::read(&p) else { return vec![] };
    let Ok(d) = serde_json::from_slice::<DeferredCards>(&bytes) else { return vec![] };
    if own_canonical_conversation(project_id).is_none() {
        return vec![];
    }
    let _ = std::fs::remove_file(&p);
    let c = post_blank_cards(project_id, &d.blank, &d.source);
    c.posted
}

/// Slice B's settle listener (CL18): a pending imported Thread just moved
/// onto the clone's pin, so its kept leave-blank cards can go there. The
/// listener runs while `settle_pending` holds the shared DB lock, so the
/// cards are posted from another thread.
pub fn on_settled(s: &k2_core::overlay::transfer::SettledImport) {
    let project_id = s.project_id.clone();
    if !deferred_path(&project_id).exists() {
        return;
    }
    std::thread::spawn(move || {
        let posted = post_deferred_cards(&project_id);
        k2_core::log_debug!("[clone-vault] {} deferred card(s) posted after a Thread settled", posted.len());
    });
}

/// Leave-blank cards are still kept for `project_id` (no Thread yet).
pub fn has_deferred_cards(project_id: &str) -> bool {
    deferred_path(project_id).exists()
}

/// Register [`on_settled`] with the Threads import (daemon boot).
pub fn install_settle_listener() {
    k2_core::overlay::transfer::set_settle_listener(on_settled);
}

/// Boot sweep: post kept cards for every clone that has its own Thread by
/// now. Returns how many cards were posted.
pub fn sweep_deferred_cards() -> usize {
    let dir = k2_core::paths::k2_home().join("clone-vault").join("pending-cards");
    let Ok(entries) = std::fs::read_dir(&dir) else { return 0 };
    entries
        .flatten()
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
        .map(|project_id| post_deferred_cards(&project_id).len())
        .sum()
}

/// The names an import landed (`vault_imported`), for the Threads import's
/// secret-card references (slice B `copied_secret_names`).
pub fn imported_names(out: &Value) -> HashSet<String> {
    out["vault_imported"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

// ── same box (§6.5) ─────────────────────────────────────────────────

/// Same-box clone: re-encrypt each copy row under `workspace:<new id>`
/// (AAD binds the scope), re-create grants, post blank cards. Confirm as
/// for a carry, with the target = this vault.
pub fn copy_same_box(vault: &LocalVault, caller: &VaultCaller, ask: &CarryAsk, new_project_id: &str) -> Result<Value, VaultError> {
    if crate::vault::workspace_path(new_project_id).is_none() {
        return Err(VaultError::not_found(format!("no workspace with id {new_project_id}")));
    }
    let pid = source_project_id(ask)?;
    if pid == new_project_id {
        return Err(VaultError::bad_request("same_workspace", "the clone is a different workspace"));
    }
    let mut ask = ask.clone();
    ask.vault_target = Some(own_target(vault, ask.vault_target.as_ref().and_then(|t| t.label.clone()))?);
    let plan = build_plan(vault, caller, &pid, &ask)?;
    let method = take_confirm(vault, caller, &plan, &ask)?;
    let clone_id = ask.clone_id.clone().unwrap_or_default();
    let mut items = vec![];
    for r in plan.copies() {
        let v = vault.resolve(&Selector::Id(r.meta.id.clone()), Use::System, &Requester::System("clone".into()))?;
        items.push(CarryItem {
            scope_kind: "workspace".into(),
            scope_label: plan.source_workspace.clone(),
            scope_id: Some(pid.clone()),
            name: r.name.clone(),
            kind: r.kind.clone(),
            hosts: r.meta.hosts.clone(),
            inject: r.meta.inject.clone(),
            default_delivery: r.meta.default_delivery.as_str().into(),
            delivery_ceiling: r.meta.delivery_ceiling.as_str().into(),
            description: r.meta.description.clone(),
            human_given: r.meta.human_given,
            value_b64: CarryItem::encode_value(v.value.expose()),
            source_scope: Some(r.meta.scope.kind.as_str().into()),
            grants: r
                .grants
                .iter()
                .map(|g| CarryGrant { delivery: g.delivery.as_str().into(), env_name: g.env_name.clone(), file_name: g.file_name.clone(), hosts_narrowed: g.hosts_narrowed.clone() })
                .collect(),
        });
    }
    let blank = plan
        .blanks()
        .map(|r| CarryBlank {
            name: r.name.clone(),
            kind: r.kind.clone(),
            hosts: r.meta.hosts.clone(),
            delivery: r.meta.default_delivery.as_str().into(),
            env_names: r.env_names.clone(),
            source_scope: r.meta.scope.kind.as_str().into(),
        })
        .collect();
    // In process: no seal. The payload shape is shared with `apply`.
    let p = CarryPayload::in_process(vault.vault_id(), ask.own_box_move, &clone_id, &pid, &plan.source_workspace, items, blank);
    crate::vault::auth_audit(
        "vault-carry",
        &caller.actor.stamp(),
        &format!("ok clone copy {} from {pid} via {} clone={}", p.items.len(), method.as_deref().unwrap_or("none"), if clone_id.is_empty() { "-" } else { &clone_id }),
        "-",
    );
    let mut out = apply(vault, caller, &p, new_project_id)?;
    out["copied"] = json!(plan.names(Choice::Copy));
    out["blank"] = json!(plan.names(Choice::Blank));
    out["skipped"] = json!(plan.names(Choice::Skip));
    Ok(out)
}

fn own_target(vault: &LocalVault, label: Option<String>) -> Result<VaultTarget, VaultError> {
    Ok(VaultTarget { recipient: vault.carry_recipient()?.encode(), vault_id: vault.vault_id().to_string(), label: label.or(Some("this server".into())) })
}

// ── routes ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ConfirmBody {
    #[serde(flatten)]
    ask: CarryAsk,
    method: Option<String>,
    password: Option<String>,
    /// 0.46.1: an agent's clone proposal this clone runs (`cp_…`). The one
    /// confirm then also approves it (see `confirm_route`).
    #[serde(default)]
    proposal: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ImportBody {
    project_id: Option<String>,
    project_path: Option<String>,
    clone_id: Option<String>,
    vault_carry_b64: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CopyBody {
    #[serde(flatten)]
    ask: CarryAsk,
    /// The clone (already registered on this box).
    new_project_id: Option<String>,
    new_project_path: Option<String>,
}

fn parse<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, VaultError> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))
}

/// The `plan` answer: rows, not-carried, `.env` files, and (with a target
/// and clone id) the confirm the client must get.
pub fn plan_json(vault: &LocalVault, caller: &VaultCaller, ask: &CarryAsk) -> Result<Value, VaultError> {
    let plan = plan_for(vault, caller, ask)?;
    let dir = match plan.source_workspace_id.strip_prefix("path:") {
        Some(p) => Some(p.to_string()),
        None => crate::vault::workspace_path(&plan.source_workspace_id),
    };
    let env_files = dir.map(|d| env_files(&d)).unwrap_or_default();
    let mut v = json!({
        "ok": true,
        "plan": plan,
        "envFiles": env_files,
        "envFilesHint": if env_files.is_empty() { Value::Null } else {
            json!(format!("move them into the vault first: k2 vault import <file> --scope ws:{} (POST /cli/vault/import)", plan.source_workspace))
        },
        "needsConfirm": plan.needs_confirm(),
    });
    if plan.needs_confirm() {
        let digest = plan_digest(&plan, ask.vault_target.as_ref(), ask.clone_id.as_deref())?;
        v["confirm"] = json!({
            "action": CONFIRM_ACTION,
            "secretId": confirm_secret_id(&digest),
            "reason": confirm_reason(&plan, ask.vault_target.as_ref()),
            "methods": crate::vault_routes::confirm_methods(vault, caller),
            "allowed": caller.actor.is_admin_human() && !caller.is_passport(),
        });
    }
    Ok(v)
}

/// `/cli/clone/vault/confirm`: one person confirm (Touch ID, the vault
/// password or the Connect password), Owner or Admin people only (D1).
///
/// - Without `proposal`: the `clone-carry` token for the plan (CL14), as
///   before. A plan with nothing to confirm is refused.
/// - With `proposal` (0.46.1, one interaction instead of two): the same
///   single check also mints the `clone-approve` token bound to that
///   proposal (`approveToken`), and the macOS sheet's reason names both
///   ("approve … and copy N secrets …"). Each token stays single-use,
///   60 s, bound to this person and to its own action and id, so the
///   approve route and the clone run check exactly what they checked
///   before: the security level is unchanged, only the second prompt is
///   gone. The proposal must be open, unexpired and for the plan's
///   workspace. A plan with nothing to copy mints only `approveToken`
///   (`token` null).
pub(crate) fn confirm_route(vault: &LocalVault, c: &VaultCaller, b: &ConfirmBody, ingress: &str) -> Result<Value, VaultError> {
    crate::vault_routes::human_admin(c)?;
    let method = b.method.as_deref().unwrap_or("");
    let Some(pid) = b.proposal.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        let (secret_id, reason, _) = confirm_binding(vault, c, &b.ask)?;
        let token = crate::vault_routes::confirm_check(vault, c, CONFIRM_ACTION, Some(&secret_id), method, b.password.as_deref(), &reason, ingress)?;
        return Ok(json!({ "ok": true, "token": token, "secretId": secret_id, "reason": reason, "expiresIn": 60 }));
    };
    confirm_for_proposal(vault, c, &b.ask, pid, method, b.password.as_deref(), ingress)
}

/// The one-interaction confirm for running an agent's clone proposal (see
/// [`confirm_route`]). Also served by `/cli/vault/confirm/finish` (action
/// `clone-carry` with `clonePlan` and `proposal`), which `k2 clone approve`
/// reaches over vault.sock for Touch ID on this Mac.
pub(crate) fn confirm_for_proposal(
    vault: &LocalVault,
    c: &VaultCaller,
    ask: &CarryAsk,
    pid: &str,
    method: &str,
    password: Option<&str>,
    ingress: &str,
) -> Result<Value, VaultError> {
    crate::vault_routes::human_admin(c)?;
    let proposal = open_proposal(pid)?;
    let plan = plan_for(vault, c, ask)?;
    if plan.source_workspace_id != proposal.project_id {
        return Err(VaultError::bad_request("proposal_mismatch", "that proposal is for another workspace; open it from its own workspace"));
    }
    let to = if proposal.target == "here" { "a second copy on this server".to_string() } else { proposal.target.clone() };
    let approve_reason = format!("approve the clone of {} to {to}", plan.source_workspace);
    let approve_action = crate::clone_proposal_routes::CONFIRM_ACTION;
    if !plan.needs_confirm() {
        let approve = crate::vault_routes::confirm_check(vault, c, approve_action, Some(&proposal.id), method, password, &approve_reason, ingress)?;
        return Ok(json!({ "ok": true, "token": Value::Null, "approveToken": approve, "proposal": proposal.id, "reason": approve_reason, "expiresIn": 60 }));
    }
    let digest = plan_digest(&plan, ask.vault_target.as_ref(), ask.clone_id.as_deref())?;
    let secret_id = confirm_secret_id(&digest);
    let reason = format!("{approve_reason} and {}", confirm_reason(&plan, ask.vault_target.as_ref()));
    // The one person check (and its audit line) happens here; the approve
    // token below is minted only after it passed, for the same person and
    // method.
    let token = crate::vault_routes::confirm_check(vault, c, CONFIRM_ACTION, Some(&secret_id), method, password, &reason, ingress)?;
    let stamp = c.actor.stamp();
    let approve = vault.confirm.mint(approve_action, Some(&proposal.id), &stamp, method);
    crate::vault::auth_audit("vault-confirm", &stamp, &format!("ok {approve_action} via {method} (with {CONFIRM_ACTION})"), ingress);
    Ok(json!({
        "ok": true,
        "token": token,
        "approveToken": approve,
        "proposal": proposal.id,
        "secretId": secret_id,
        "reason": reason,
        "expiresIn": 60,
    }))
}

/// An open, unexpired clone proposal by id (or unique prefix).
fn open_proposal(token: &str) -> Result<k2_core::clone_proposals::Proposal, VaultError> {
    let p = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::clone_proposals::find(&conn, token).map_err(|e| VaultError::new("db", 500, format!("database: {e}")))?
    }
    .ok_or_else(|| VaultError::not_found(format!("no clone proposal '{token}'")))?;
    if p.status != k2_core::clone_proposals::STATUS_OPEN {
        return Err(VaultError::conflict("proposal_not_open", "this proposal was already decided"));
    }
    if p.expires_at <= chrono::Utc::now().timestamp() {
        return Err(VaultError::new("proposal_expired", 410, "this proposal expired (7 days); the agent can propose again"));
    }
    Ok(p)
}

/// `.env*` files that "Send .env files as files" would put in the bundle.
fn env_files(dir: &str) -> Vec<String> {
    match k2_core::clone::inventory(dir, k2_core::clone::CloneOptions::default()) {
        Ok(inv) => inv.scrubbed_secrets,
        Err(_) => vec![],
    }
}

pub fn handle(path: &str, caller: Result<VaultCaller, VaultError>, body: &[u8], ingress: &str) -> CliResponse {
    let r = (|| -> Result<Value, VaultError> {
        let c = caller?;
        if c.is_passport() {
            return Err(VaultError::forbidden("people_only", "agents propose clones (k2 clone propose); a person runs them"));
        }
        let vault = crate::vault::get()?;
        match path {
            PLAN => plan_json(&vault, &c, &parse::<CarryAsk>(body)?),
            CONFIRM => confirm_route(&vault, &c, &parse::<ConfirmBody>(body)?, ingress),
            EXPORT => {
                let ask: CarryAsk = parse(body)?;
                let target = ask.vault_target.clone().ok_or_else(|| VaultError::bad_request("target_required", "vault_target {recipient, vault_id}"))?;
                let clone_id = ask.clone_id.clone().filter(|s| !s.trim().is_empty()).ok_or_else(|| VaultError::bad_request("clone_id_required", "clone_id"))?;
                let pid = source_project_id(&ask)?;
                let plan = build_plan(&vault, &c, &pid, &ask)?;
                let method = take_confirm(&vault, &c, &plan, &ask)?;
                let x = export(&vault, &c, &plan, &target, &clone_id, ask.own_box_move, method.as_deref())?;
                Ok(json!({ "ok": true, "vault_carry": x.report, "vault_carry_b64": x.carry_b64 }))
            }
            IMPORT => {
                let b: ImportBody = parse(body)?;
                let pid = match (b.project_id.as_deref(), b.project_path.as_deref()) {
                    (Some(id), _) if !id.trim().is_empty() => id.trim().to_string(),
                    (_, Some(p)) => project_id_for_path(p).ok_or_else(|| VaultError::not_found("that folder isn't a registered workspace"))?,
                    _ => return Err(VaultError::bad_request("project_required", "project_id of the clone on this box")),
                };
                let carry = b.vault_carry_b64.ok_or_else(|| VaultError::bad_request("carry_required", "vault_carry_b64"))?;
                let mut v = import(&vault, &c, &carry, &pid, b.clone_id.as_deref())?;
                v["ok"] = json!(true);
                Ok(v)
            }
            COPY => {
                let b: CopyBody = parse(body)?;
                let new_id = match (b.new_project_id.as_deref(), b.new_project_path.as_deref()) {
                    (Some(id), _) if !id.trim().is_empty() => id.trim().to_string(),
                    (_, Some(p)) => project_id_for_path(p).ok_or_else(|| VaultError::not_found("the clone folder isn't a registered workspace"))?,
                    _ => return Err(VaultError::bad_request("project_required", "new_project_id of the clone")),
                };
                let mut v = copy_same_box(&vault, &c, &b.ask, &new_id)?;
                v["ok"] = json!(true);
                Ok(v)
            }
            _ => Err(VaultError::not_found("no such clone vault route")),
        }
    })();
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => crate::vault_routes::refuse(&e),
    }
}

#[cfg(test)]
#[path = "clone_vault_tests.rs"]
mod tests;
