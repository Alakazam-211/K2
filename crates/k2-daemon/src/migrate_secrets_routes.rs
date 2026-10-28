//! `/cli/migrate/secrets/*` — box moves carry the vault
//! (`.k2/prds/prd-box-move-carry-v1.md`, `k2 migrate export-secrets` /
//! `import-secrets`).
//!
//! The data key moves, never the box key: the new box makes a one-time
//! move key (`begin`), the old box exports its whole vault to it and
//! freezes (`export`), the new box checks (`import`, a dry run) and adopts
//! (`import` with `apply`). `unfreeze` (old box) and `undo` (new box) call
//! a move off.
//!
//! **Who (Q3, Rosson 10-28).** `export`, `import --apply`, `undo` and
//! `unfreeze` are for the box owner as a person, with the vault confirm:
//! - from a terminal outside K2 or an SSH login (the vault socket's person
//!   check) or a Connect owner login, with the confirm (`vault-move`,
//!   bound to the step); the K2 app's owner token with that confirm too;
//! - anyone else (K2 Web over SSH without a terminal, an agent) gets the
//!   step **held** as `mv_…` (202 `status: held`): the owner approves it
//!   with `k2 migrate approve mv_…` from their own terminal, or taps the
//!   Thread card (`k2 permission request vault.move-approve request=mv_…`,
//!   the same confirm). Agents never run a step themselves.
//! - A box whose vault is closed (`vault_key_missing`) has no confirm to
//!   take: there a person's channel alone imports or undoes.
//!
//! `begin`, `status` and the import's dry run change nothing a person must
//! approve, so the owner token, owner/admin logins and agents may run them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use k2_vault::agefmt::{Identity, Recipient};
use k2_vault::boxmove::{self, MoveFile, MoveSource};
use k2_vault::{Actor, HumanProof, LocalVault, Role, SecretStore, VaultError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::{Channel, VaultCaller};

pub const STATUS: &str = "/cli/migrate/secrets/status";
pub const BEGIN: &str = "/cli/migrate/secrets/begin";
pub const EXPORT: &str = "/cli/migrate/secrets/export";
pub const UNFREEZE: &str = "/cli/migrate/secrets/unfreeze";
pub const IMPORT: &str = "/cli/migrate/secrets/import";
pub const UNDO: &str = "/cli/migrate/secrets/undo";
pub const APPROVE: &str = "/cli/migrate/secrets/approve";

pub const ROUTES: &[&str] = &[STATUS, BEGIN, EXPORT, UNFREEZE, IMPORT, UNDO, APPROVE];
pub const POST_ROUTES: &[&str] = &[BEGIN, EXPORT, UNFREEZE, IMPORT, UNDO, APPROVE];

/// The vault confirm action every gated step takes (bound to the step).
pub const CONFIRM_ACTION: &str = "vault-move";
/// A move key lives a day, or until one import succeeds.
pub const MOVE_KEY_TTL_SECS: i64 = 24 * 3600;
/// A held step waits a day for the owner.
pub const HELD_TTL_SECS: i64 = 24 * 3600;
/// The system secret the import reports on.
pub const TUNNEL_ENGINE: &str = "frpc";

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

fn err(code: &'static str, status: u16, hint: impl Into<String>) -> VaultError {
    VaultError::new(code, status, hint)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn rfc(t: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0).map(|d| d.to_rfc3339()).unwrap_or_default()
}

// ── the move key (new box) ──────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveKeyFile {
    /// `AGE-SECRET-KEY-1…` (0600, beside the vault, never in a response).
    identity: String,
    recipient: String,
    created_at: i64,
    expires_at: i64,
}

fn move_dir(home: &Path) -> PathBuf {
    crate::vault::vault_dir(home).join("move")
}

fn move_key_path(home: &Path) -> PathBuf {
    move_dir(home).join("move-key.json")
}

/// This box's move key, when one is set and unexpired.
fn load_move_key(home: &Path) -> Option<(Identity, MoveKeyFile)> {
    let raw = std::fs::read(move_key_path(home)).ok()?;
    let f: MoveKeyFile = serde_json::from_slice(&raw).ok()?;
    if f.expires_at <= now() {
        return None;
    }
    let id = Identity::parse(&f.identity).ok()?;
    Some((id, f))
}

fn drop_move_key(home: &Path) {
    let _ = std::fs::remove_file(move_key_path(home));
}

fn key_json(f: &MoveKeyFile) -> Value {
    let fp = Recipient::parse(&f.recipient).map(|r| boxmove::fingerprint(&r)).unwrap_or_default();
    json!({ "recipient": f.recipient, "fingerprint": fp, "createdAt": rfc(f.created_at), "expiresAt": rfc(f.expires_at) })
}

// ── held steps (Q3) ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub id: String,
    /// `export` | `import` | `undo` | `unfreeze`
    pub step: String,
    pub params: Value,
    pub created_at: i64,
    pub expires_at: i64,
    /// Who asked (`owner`, `agent:<ws>`, …).
    pub by: String,
    /// The asking agent's workspace (its permission card), if an agent.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// `pending` | `done` | `failed`
    pub status: String,
    #[serde(default)]
    pub result: Option<Value>,
}

fn held_dir(home: &Path) -> PathBuf {
    move_dir(home).join("held")
}

fn valid_held_id(id: &str) -> bool {
    id.len() == 15 && id.starts_with("mv_") && id[3..].bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn held_get(home: &Path, id: &str) -> Option<Held> {
    if !valid_held_id(id) {
        return None;
    }
    std::fs::read(held_dir(home).join(format!("{id}.json"))).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn held_put(home: &Path, h: &Held) -> Result<(), VaultError> {
    let d = held_dir(home);
    k2_vault::fsutil::ensure_private_dir(&d)?;
    let b = serde_json::to_vec_pretty(h).map_err(|_| VaultError::internal("held json"))?;
    k2_vault::fsutil::write_private_atomic(&d.join(format!("{}.json", h.id)), &b)
}

fn held_list(home: &Path) -> Vec<Held> {
    let mut out: Vec<Held> = std::fs::read_dir(held_dir(home))
        .map(|it| {
            it.flatten()
                .filter_map(|e| std::fs::read(e.path()).ok())
                .filter_map(|b| serde_json::from_slice::<Held>(&b).ok())
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|h| h.created_at);
    out
}

/// A held step that is still waiting.
pub fn held_pending(home: &Path, id: &str) -> Option<Held> {
    held_get(home, id).filter(|h| h.status == "pending" && h.expires_at > now())
}

fn step_words(step: &str) -> &'static str {
    match step {
        "export" => "export this box's vault for a move (and freeze it)",
        "import" => "replace this box's vault with the moved one",
        "undo" => "undo the last vault import on this box",
        "unfreeze" => "call the move off: lift the freeze on this box's vault",
        _ => "a vault move step",
    }
}

fn hold(home: &Path, step: &str, params: Value, c: &VaultCaller) -> Result<Value, VaultError> {
    let id = format!("mv_{}", k2_vault::crypto::hex(&k2_vault::crypto::random_bytes::<6>()));
    let t = now();
    let h = Held {
        id: id.clone(),
        step: step.to_string(),
        params,
        created_at: t,
        expires_at: t + HELD_TTL_SECS,
        by: c.actor.stamp(),
        workspace_id: c.workspace_id().map(str::to_string),
        status: "pending".into(),
        result: None,
    };
    held_put(home, &h)?;
    crate::vault::auth_audit("vault-move", &h.by, &format!("held {step} as {id}"), "migrate");
    Ok(json!({
        "ok": false,
        "status": "held",
        "request": { "id": id, "step": step, "what": step_words(step), "expiresAt": rfc(h.expires_at) },
        "hint": format!(
            "held for the box owner as {id} ({}). The owner approves it as a person, with the vault confirm: \
             k2 migrate approve {id} from their own terminal or SSH login, or the Thread card \
             (an agent asks with: k2 permission request vault.move-approve request={id}). \
             Check with: k2 migrate status",
            step_words(step)
        ),
    }))
}

// ── the person gate ─────────────────────────────────────────────────

enum Gate {
    Allow(Option<HumanProof>),
    Hold,
}

/// Why a gated step didn't run.
enum Stop {
    Refuse(VaultError),
    /// A person at a terminal: get the `vault-move` confirm bound to
    /// `binding`, then send the step again with it.
    Confirm { binding: String, label: String },
}

impl From<VaultError> for Stop {
    fn from(e: VaultError) -> Self {
        Stop::Refuse(e)
    }
}

fn is_owner(a: &Actor) -> bool {
    matches!(a, Actor::Owner | Actor::Login { role: Role::Owner, .. })
}

/// Person + confirm (§3.3, Q3). `vault`: the open vault the confirm is
/// taken on (`None` = closed: a person's channel is enough). `can_hold`:
/// a non-person may have it held for the owner.
#[allow(clippy::too_many_arguments)]
fn gate(
    vault: Option<&LocalVault>,
    c: &VaultCaller,
    not_person: Option<&'static str>,
    binding: &str,
    label: &str,
    token: Option<&str>,
    needs_confirm: bool,
    can_hold: bool,
) -> Result<Gate, Stop> {
    if c.is_job() {
        return Err(err("job_use_only", 403, "a job never moves the vault").into());
    }
    let refuse_non_person = || -> Result<Gate, Stop> {
        if can_hold {
            Ok(Gate::Hold)
        } else {
            Err(err(
                "person_required",
                403,
                "only the box owner, as a person, approves this: from your own terminal or SSH login (not a K2 terminal), or a K2 Connect owner login, with the vault confirm. Agents: don't run it; tell your owner.",
            )
            .into())
        }
    };
    if c.is_passport() {
        return refuse_non_person();
    }
    if !is_owner(&c.actor) {
        return Err(err("owner_only", 403, "box moves are for the box owner").into());
    }
    let person = c.channel.is_person();
    // The K2 app: the owner token on TCP (never the vault socket's
    // refused peer) stands in for a person once it carries the confirm.
    let app = not_person.is_none() && matches!(c.actor, Actor::Owner) && c.channel == Channel::Plain;
    // A confirm the caller brings is always checked (bound to this step),
    // even where a person's channel alone would do.
    if let (Some(t), Some(v)) = (token.map(str::trim).filter(|t| !t.is_empty()), vault) {
        if !(person || app) {
            return refuse_non_person();
        }
        let p = crate::vault_routes::take_proof(v, c, CONFIRM_ACTION, Some(binding), Some(t))?;
        return Ok(Gate::Allow(Some(p)));
    }
    if person {
        return if needs_confirm && vault.is_some() {
            Err(Stop::Confirm { binding: binding.to_string(), label: label.to_string() })
        } else {
            Ok(Gate::Allow(None))
        };
    }
    refuse_non_person()
}

fn confirm_needed(binding: &str, label: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "error": { "code": "vault_confirm_required", "hint": "this needs your vault confirm first (k2 migrate asks for it)" },
            "confirm": { "action": CONFIRM_ACTION, "secretId": binding, "label": label },
        })
        .to_string(),
    }
}

fn held_response(v: Value) -> CliResponse {
    CliResponse { status: "202 Accepted", content_type: "application/json", body: v.to_string() }
}

// ── where the vault is (the daemon's, or a test's own) ──────────────

/// The vault these routes act on. The daemon's ([`Live`]) is the served
/// one; tests give their own, so a freeze never touches the shared test
/// vault.
pub trait MoveEnv {
    fn vault(&self) -> Result<Arc<LocalVault>, VaultError>;
    fn apply(&self, home: &Path, identity: &Identity, mf: &MoveFile) -> Result<(Arc<LocalVault>, boxmove::ImportRecord), VaultError>;
    fn undo(&self, home: &Path, actor: &Actor, proof: Option<&HumanProof>) -> Result<boxmove::ImportRecord, VaultError>;
}

/// The daemon's served vault.
pub struct Live;

impl MoveEnv for Live {
    fn vault(&self) -> Result<Arc<LocalVault>, VaultError> {
        crate::vault::get()
    }
    fn apply(&self, home: &Path, identity: &Identity, mf: &MoveFile) -> Result<(Arc<LocalVault>, boxmove::ImportRecord), VaultError> {
        crate::vault::move_import_apply(home, identity, mf)
    }
    fn undo(&self, home: &Path, actor: &Actor, proof: Option<&HumanProof>) -> Result<boxmove::ImportRecord, VaultError> {
        crate::vault::move_import_undo(home, actor, proof)
    }
}

// ── entry ───────────────────────────────────────────────────────────

/// Dispatcher and vault-socket entry. Runs whether the vault is open or
/// not (an import must work on a box stuck on `vault_key_missing`).
pub fn handle(
    path: &str,
    caller: Result<VaultCaller, VaultError>,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
    not_person: Option<&'static str>,
) -> CliResponse {
    let Some(home) = crate::vault::boot_home() else {
        return crate::vault_routes::refuse(&err("vault_not_open", 503, "the vault hasn't booted yet"));
    };
    handle_in(&Live, &home, path, caller, body, params, ingress, not_person)
}

/// [`handle`] over a given vault and K2 home (tests use temp ones).
#[allow(clippy::too_many_arguments)]
pub fn handle_in(
    env: &dyn MoveEnv,
    home: &Path,
    path: &str,
    caller: Result<VaultCaller, VaultError>,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
    not_person: Option<&'static str>,
) -> CliResponse {
    let c = match caller {
        Ok(c) => c,
        Err(e) => return crate::vault_routes::refuse(&e),
    };
    if params.keys().any(|k| matches!(k.as_str(), "identity" | "password" | "secret")) {
        return crate::vault_routes::refuse(&err("value_on_argv", 400, "keys never ride in a URL"));
    }
    let b: Body = match crate::vault_routes::parse(body) {
        Ok(b) => b,
        Err(e) => return crate::vault_routes::refuse(&e),
    };
    let r = match path {
        STATUS => status(env, home, &c),
        BEGIN => begin(home, &c, &b),
        IMPORT if !b.apply => dry_run(env, home, &c, &b),
        EXPORT | UNFREEZE | IMPORT | UNDO => step(env, home, path_step(path), &c, &b, ingress, not_person, None),
        APPROVE => approve(env, home, &c, &b, ingress, not_person),
        _ => Err(Stop::Refuse(VaultError::not_found("no such migrate route"))),
    };
    match r {
        Ok(Reply::Ok(v)) => CliResponse::ok_json(v.to_string()),
        Ok(Reply::Held(v)) => held_response(v),
        Err(Stop::Refuse(e)) => crate::vault_routes::refuse(&e),
        Err(Stop::Confirm { binding, label }) => confirm_needed(&binding, &label),
    }
}

enum Reply {
    Ok(Value),
    Held(Value),
}

fn path_step(path: &str) -> &'static str {
    match path {
        EXPORT => "export",
        UNFREEZE => "unfreeze",
        IMPORT => "import",
        _ => "undo",
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Body {
    /// export: the new box's move key (`age1…`).
    to: Option<String>,
    /// export: where to write the `.k2mv` file (absolute).
    out: Option<String>,
    /// import: the `.k2mv` file (absolute).
    file: Option<String>,
    #[serde(default)]
    apply: bool,
    /// begin: replace an unexpired move key.
    #[serde(default)]
    new: bool,
    /// approve: the held step.
    request: Option<String>,
    confirm_token: Option<String>,
}

fn require_reader(c: &VaultCaller) -> Result<(), Stop> {
    if c.is_job() {
        return Err(err("job_use_only", 403, "a job never moves the vault").into());
    }
    if c.is_passport() || c.actor.is_admin_human() {
        return Ok(());
    }
    Err(err("owner_only", 403, "box moves are for the box owner").into())
}

fn abs_path(s: Option<&str>, what: &str) -> Result<PathBuf, Stop> {
    let s = s.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| err("path_required", 400, format!("name the {what}")))?;
    let p = PathBuf::from(s);
    if !p.is_absolute() {
        return Err(err("path_not_absolute", 400, format!("the {what} must be an absolute path")).into());
    }
    Ok(p)
}

fn status(env: &dyn MoveEnv, home: &Path, c: &VaultCaller) -> Result<Reply, Stop> {
    require_reader(c)?;
    let (vault, frozen) = match env.vault() {
        Ok(v) => (json!({ "open": true, "vaultId": v.vault_id(), "keyHome": v.key_store().kind().as_str() }), json!(v.frozen_marker())),
        Err(e) => (json!({ "open": false, "error": { "code": e.code, "hint": e.hint } }), Value::Null),
    };
    let held: Vec<Value> = held_list(home)
        .into_iter()
        .filter(|h| h.created_at > now() - 7 * 24 * 3600)
        .map(|h| {
            let state = if h.status == "pending" && h.expires_at <= now() { "expired".to_string() } else { h.status.clone() };
            json!({ "id": h.id, "step": h.step, "what": step_words(&h.step), "status": state, "by": h.by,
                    "createdAt": rfc(h.created_at), "expiresAt": rfc(h.expires_at), "result": h.result })
        })
        .collect();
    Ok(Reply::Ok(json!({
        "vault": vault,
        "frozen": frozen,
        "moveKey": load_move_key(home).map(|(_, f)| key_json(&f)),
        "lastImport": boxmove::last_import(&crate::vault::vault_dir(home)),
        "held": held,
    })))
}

fn begin(home: &Path, c: &VaultCaller, b: &Body) -> Result<Reply, Stop> {
    require_reader(c)?;
    if let Some((_, f)) = load_move_key(home).filter(|_| !b.new) {
        return Ok(Reply::Ok(json!({ "ok": true, "created": false, "moveKey": key_json(&f) })));
    }
    let id = Identity::generate();
    let t = now();
    let f = MoveKeyFile {
        identity: id.encode().expose_str().unwrap_or_default().to_string(),
        recipient: id.recipient().encode(),
        created_at: t,
        expires_at: t + MOVE_KEY_TTL_SECS,
    };
    let d = move_dir(home);
    k2_vault::fsutil::ensure_private_dir(&d)?;
    let raw = serde_json::to_vec_pretty(&f).map_err(|_| VaultError::internal("move key json"))?;
    k2_vault::fsutil::write_private_atomic(&move_key_path(home), &raw)?;
    crate::vault::auth_audit("vault-move", &c.actor.stamp(), "ok begin (new move key)", "migrate");
    Ok(Reply::Ok(json!({ "ok": true, "created": true, "moveKey": key_json(&f) })))
}

fn read_move_file(home: &Path, b: &Body) -> Result<(Identity, MoveKeyFile, MoveFile), Stop> {
    let (id, kf) = load_move_key(home).ok_or_else(|| {
        err("no_move_key", 409, "this box has no move key (or it expired after 24 h): run k2 migrate import-secrets begin first, then export again on the old box")
    })?;
    let path = abs_path(b.file.as_deref(), "move file")?;
    let meta = std::fs::metadata(&path).map_err(|_| VaultError::not_found(format!("no file at {}", path.display())))?;
    if meta.len() as usize > boxmove::MAX_FILE_BYTES {
        return Err(err("move_file_bad", 400, "that file is larger than any move file").into());
    }
    let bytes = std::fs::read(&path).map_err(|e| VaultError::internal(format!("read: {}", e.kind())))?;
    let mf = boxmove::open_move_file(&id, &bytes)?;
    Ok((id, kf, mf))
}

fn check_json(m: &boxmove::MoveCheck, kf: &MoveKeyFile) -> Value {
    let token = m.system_present.first().map(|(_, p)| *p).unwrap_or(false);
    json!({
        "vaultId": m.vault_id,
        "gen": m.gen,
        "secrets": { "user": m.user_secrets, "system": m.system_secrets },
        "source": m.source,
        "sourceKeyHome": m.source_key_home,
        "exportedAt": m.created_at,
        "moveKey": key_json(kf),
        "tunnelToken": if token { "present" } else { "MISSING" },
        "systemdCreds": m.systemd_creds.iter().map(|(n, u)| json!({ "secret": n, "unit": u })).collect::<Vec<_>>(),
    })
}

fn dry_run(env: &dyn MoveEnv, home: &Path, c: &VaultCaller, b: &Body) -> Result<Reply, Stop> {
    require_reader(c)?;
    let (id, kf, mf) = read_move_file(home, b)?;
    let dir = crate::vault::vault_dir(home);
    boxmove::refuse_stale(&dir.join("vault.db"), &mf.manifest)?;
    let scratch = boxmove::scratch_dir(&dir);
    let r = boxmove::check_move(&mf, &id, &scratch, k2_vault::confirm::KdfCost::Strong, &[(TUNNEL_ENGINE, crate::vault::tunnel::NAME)]);
    let _ = std::fs::remove_dir_all(&scratch);
    let m = r?;
    let mut out = check_json(&m, &kf);
    out["dryRun"] = json!(true);
    out["replaces"] = match env.vault() {
        Ok(v) => json!({ "open": true, "vaultId": v.vault_id(), "userSecrets": v.user_secret_count() }),
        Err(e) => json!({ "open": false, "error": e.code }),
    };
    out["next"] = json!("nothing changed. To import: k2 migrate import-secrets <file> --apply (you confirm)");
    Ok(Reply::Ok(out))
}

/// One gated step, from a route or from an approval (`approved`: the held
/// step being run, whose binding is its id).
#[allow(clippy::too_many_arguments)]
fn step(
    env: &dyn MoveEnv,
    home: &Path,
    step: &'static str,
    c: &VaultCaller,
    b: &Body,
    ingress: &str,
    not_person: Option<&'static str>,
    approved: Option<&Held>,
) -> Result<Reply, Stop> {
    let current = env.vault();
    let closed = current.as_ref().err().cloned();
    let vault = current.ok();
    let stamp = c.actor.stamp();
    // What the step works on, checked before anything is held.
    let (params, binding, label, needs_confirm): (Value, String, String, bool) = match step {
        "export" | "unfreeze" => {
            let v = vault.as_ref().ok_or_else(|| closed.clone().unwrap_or_else(|| err("vault_not_open", 503, "the vault isn't open")))?;
            if step == "export" {
                let to = b.to.as_deref().map(str::trim).unwrap_or("");
                let r = Recipient::parse(to).map_err(|_| err("bad_move_key", 400, "--to takes the new box's move key (age1…, from k2 migrate import-secrets begin)"))?;
                let out = abs_path(b.out.as_deref(), "output file (--out)")?;
                if out.exists() {
                    return Err(err("out_exists", 409, format!("{} already exists; pick a new name", out.display())).into());
                }
                let fp = boxmove::fingerprint(&r);
                (json!({ "to": to, "out": out }), format!("export:{fp}"), format!("export the vault to move key {fp}"), true)
            } else {
                if !v.is_frozen() {
                    return Err(err("not_frozen", 409, "this vault isn't frozen for a move; nothing to unfreeze").into());
                }
                (json!({}), "unfreeze".into(), "lift the box-move freeze".into(), true)
            }
        }
        "import" => {
            let (_, kf, _) = read_move_file(home, b)?;
            let file = abs_path(b.file.as_deref(), "move file")?;
            let fp = Recipient::parse(&kf.recipient).map(|r| boxmove::fingerprint(&r)).unwrap_or_default();
            // As `k2 vault restore`: replacing a vault that holds secrets
            // takes the confirm; a closed or empty one, a person's channel.
            let confirm = vault.as_ref().is_some_and(|v| v.user_secret_count() > 0);
            (json!({ "file": file, "apply": true }), format!("import:{fp}"), format!("replace this vault with the moved one (move key {fp})"), confirm)
        }
        _ => {
            if boxmove::last_import(&crate::vault::vault_dir(home)).is_none() {
                return Err(err("no_import", 409, "no import to undo on this box (k2 migrate status)").into());
            }
            (json!({}), "undo".into(), "undo the last vault import".into(), vault.is_some())
        }
    };
    let (binding, label) = match approved {
        Some(h) => (h.id.clone(), format!("{} ({})", label, h.id)),
        None => (binding, label),
    };
    let proof = match gate(vault.as_deref(), c, not_person, &binding, &label, b.confirm_token.as_deref(), needs_confirm, approved.is_none())? {
        Gate::Hold => return Ok(Reply::Held(hold(home, step, params, c)?)),
        Gate::Allow(p) => p,
    };
    let r = run_step(env, home, step, c, b, vault, proof.as_ref());
    match &r {
        Ok(_) => crate::vault::auth_audit("vault-move", &stamp, &format!("ok {step}"), ingress),
        Err(Stop::Refuse(e)) => crate::vault::auth_audit("vault-move", &stamp, &format!("deny {step}: {}", e.code), ingress),
        Err(Stop::Confirm { .. }) => {}
    }
    r
}

fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: gethostname writes at most `len` bytes into our buffer.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        if rc == 0 {
            let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..n]).into_owned();
        }
        String::new()
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").unwrap_or_default()
    }
}

fn run_step(env: &dyn MoveEnv, home: &Path, step: &str, c: &VaultCaller, b: &Body, vault: Option<Arc<LocalVault>>, proof: Option<&HumanProof>) -> Result<Reply, Stop> {
    match step {
        "export" => {
            let v = vault.ok_or_else(|| err("vault_not_open", 503, "the vault isn't open"))?;
            let to = Recipient::parse(b.to.as_deref().unwrap_or(""))?;
            let out = abs_path(b.out.as_deref(), "output file (--out)")?;
            let cfg = k2_core::tunnel::config::load().ok();
            let source = MoveSource {
                host: hostname(),
                tunnel_subdomain: cfg.as_ref().map(|c| c.subdomain.trim().to_string()).filter(|s| !s.is_empty()),
                tunnel_device_id: cfg.as_ref().and_then(|c| c.device_id.clone()),
            };
            // The gate always takes the confirm on an open vault (VU-B1: a
            // new key holder is a person's call).
            let proof = proof.ok_or_else(|| err("vault_confirm_required", 403, "a box move export needs the owner's confirm"))?;
            let exp = v.move_export(&to, source, &c.actor, proof)?;
            if let Err(e) = k2_vault::fsutil::write_private_atomic(&out, &exp.file) {
                // Nothing left behind: no file, no freeze.
                let _ = v.move_unfreeze(&c.actor);
                return Err(e.into());
            }
            let m = &exp.manifest;
            Ok(Reply::Ok(json!({
                "ok": true,
                "out": out,
                "bytes": exp.file.len(),
                "moveKey": m.move_key,
                "vaultId": m.source_vault_id,
                "gen": m.gen,
                "secrets": { "user": m.user_secrets, "system": m.system_secrets },
                "frozen": true,
                "next": "This vault is now frozen: reads work, writes wait. Copy the file to the new box (scp), then there: k2 migrate import-secrets <file> (a dry run), then --apply. Called off? k2 migrate export-secrets --unfreeze",
            })))
        }
        "unfreeze" => {
            let v = vault.ok_or_else(|| err("vault_not_open", 503, "the vault isn't open"))?;
            let was = v.move_unfreeze(&c.actor)?;
            Ok(Reply::Ok(json!({ "ok": true, "wasFrozen": was, "frozen": false })))
        }
        "import" => {
            let (id, kf, mf) = read_move_file(home, b)?;
            let (v, rec) = env.apply(home, &id, &mf)?;
            v.audit_carry("move-import", &c.actor, &format!("move:{}", rec.move_key), &[]);
            drop_move_key(home);
            let token = v.has_system_secret(TUNNEL_ENGINE, crate::vault::tunnel::NAME);
            let st = v.status();
            Ok(Reply::Ok(json!({
                "ok": true,
                "vaultId": rec.vault_id,
                "gen": rec.gen,
                "keyHome": st.key_home,
                "rollback": st.rollback,
                "secrets": { "user": st.counts.user, "system": st.counts.system },
                "tunnelToken": if token { "present" } else { "MISSING" },
                "systemdCreds": v.systemd_cred_placements().unwrap_or_default().iter().map(|(n, u)| json!({ "secret": n, "unit": u })).collect::<Vec<_>>(),
                "replaced": crate::vault::vault_dir(home).join(&rec.aside),
                "moveKey": key_json(&kf),
                "next": [
                    "Check: k2 vault status (open, rollback: false, the key home and secret count above).",
                    "Re-place each systemd credential listed: k2 vault systemd-cred NAME --replace.",
                    "The tunnel isn't started; hand it over as your runbook says.",
                    "Optional clean break: k2 vault rekey (a new data key; old copies of the vault can't read secrets added after).",
                    "Undo before the cut-over: k2 migrate import-secrets --undo.",
                ],
            })))
        }
        _ => {
            let rec = env.undo(home, &c.actor, proof)?;
            Ok(Reply::Ok(json!({
                "ok": true,
                "undone": rec.vault_id,
                "restoredFrom": rec.aside,
                "vault": match env.vault() { Ok(v) => json!({ "open": true, "vaultId": v.vault_id() }), Err(e) => json!({ "open": false, "error": e.code }) },
                "note": "The imported vault is kept in an undone-<time> folder beside vault.db.",
            })))
        }
    }
}

fn approve(env: &dyn MoveEnv, home: &Path, c: &VaultCaller, b: &Body, ingress: &str, not_person: Option<&'static str>) -> Result<Reply, Stop> {
    let id = b.request.as_deref().map(str::trim).unwrap_or("");
    let h = held_get(home, id).ok_or_else(|| VaultError::not_found(format!("no held move step {id} (k2 migrate status)")))?;
    if h.status != "pending" {
        return Err(err("already_decided", 409, format!("{id} is {} already", h.status)).into());
    }
    if h.expires_at <= now() {
        return Err(err("held_expired", 410, format!("{id} expired unanswered; run the step again")).into());
    }
    let hb: Body = serde_json::from_value(h.params.clone()).unwrap_or_default();
    let body = Body { confirm_token: b.confirm_token.clone(), ..hb };
    let r = step(env, home, step_static(&h.step), c, &body, ingress, not_person, Some(&h));
    let mut done = h.clone();
    match &r {
        Ok(Reply::Ok(v)) => {
            done.status = "done".into();
            done.result = Some(v.clone());
        }
        Err(Stop::Refuse(e)) if !matches!(e.code, "vault_confirm_required" | "confirm_failed" | "person_required" | "owner_only") => {
            done.status = "failed".into();
            done.result = Some(json!({ "error": { "code": e.code, "hint": e.hint } }));
        }
        _ => return r,
    }
    held_put(home, &done)?;
    r
}

fn step_static(s: &str) -> &'static str {
    match s {
        "export" => "export",
        "unfreeze" => "unfreeze",
        "import" => "import",
        _ => "undo",
    }
}

#[cfg(test)]
#[path = "migrate_secrets_routes_tests.rs"]
mod tests;
