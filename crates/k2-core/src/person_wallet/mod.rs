//! App users' personal wallets (prd-app-user-llm-logins-v1 rev 2, 0.46.3).
//!
//! **Model.** One wallet per App user (skin principal) on this box, shared
//! across that person's Apps (each item records the App it was added
//! through). Items have a type and a provider so other third-party logins
//! fit later; v1 holds LLM subscriptions (Claude, Codex, Grok: the
//! vendor CLI's own sign-in) and LLM API keys (+ Gemini).
//!
//! **Secrets.** Never in the database and never returned by any route:
//! - a subscription's credential lives in the item's slot
//!   (`~/.k2/llm-accounts/<tool>/<id>/`, 0700/0600, the tool's own home
//!   layout) only while a session runs; idle, it is **sealed** in the
//!   vault's `person` scope (system namespace) and the plaintext is wiped
//!   ([`seal_slot`], [`unseal_into_slot`]). The vault itself is behind the
//!   [`Sealer`] trait (k2-core has no vault dependency; the daemon passes
//!   the real one, tests a fake);
//! - an API key only ever in the vault; it is injected into the personal
//!   agent's env at spawn.
//!
//! **Use.** A person's items run only their own **personal agents**
//! ([`person_agents`]), on that agent's own session keys (not `tab-*`),
//! while the person still holds the room through the marked App and no
//! other App user holds it ([`check_usable`]). Precedence at a fresh spawn:
//! chat pin > person pin > workspace pin > server default. When the
//! person's pick isn't usable the App's fallback setting decides: the
//! company plan, or no spawn.
//!
//! **Lifecycle.** Removing an item deletes it (row, slot, vault secret),
//! never Trash. Losing App access makes items dormant; 30 days later they
//! are deleted; a principal that is gone, or turned into an agent's own
//! login, loses them and its personal-agent marks at the daemon's next
//! sweep (within a minute).

pub mod decide;

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::llm_accounts::{self, state, store, Tool, WalletError};

/// Item types.
pub mod item_type {
    /// A vendor CLI sign-in (Claude, Codex, Grok).
    pub const LLM_SUBSCRIPTION: &str = "llm_subscription";
    /// A pasted LLM API key (Claude, Codex, Grok, Gemini).
    pub const LLM_API_KEY: &str = "llm_api_key";
    pub const ALL: &[&str] = &[LLM_SUBSCRIPTION, LLM_API_KEY];
}

/// The catalog cap of the `wallet.*` verbs: the App's wallet setting
/// (`app_wallet_settings.enabled`), never a pass or room cap.
pub const CAP: &str = "wallet:self";

/// Item ids start with this; the slot folder uses the same id.
pub const ID_PREFIX: &str = "pw_";
/// Dormant items are deleted after this long (Rosson #4).
pub const DORMANT_KEEP_SECS: i64 = 30 * 24 * 3600;

pub fn is_item_id(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One wallet item. Metadata only, never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub principal_id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub provider: String,
    pub label: String,
    pub state: String,
    pub detail: Option<String>,
    pub plan: Option<String>,
    pub expires_at: Option<i64>,
    pub sealed: bool,
    pub added_via: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub refreshed_at: Option<i64>,
    pub dormant_since: Option<i64>,
}

impl Item {
    pub fn tool(&self) -> Option<Tool> {
        Tool::parse(&self.provider)
    }
    pub fn is_api_key(&self) -> bool {
        self.item_type == item_type::LLM_API_KEY
    }
    pub fn is_subscription(&self) -> bool {
        self.item_type == item_type::LLM_SUBSCRIPTION
    }
    /// The vault secret name for this item (scope `person:<principal>`).
    pub fn secret_name(&self) -> String {
        secret_name(&self.id)
    }
}

pub fn secret_name(item_id: &str) -> String {
    format!("wallet-{item_id}")
}

const COLS: &str = "id, principal_id, item_type, provider, label, state, detail, plan, expires_at, sealed, \
                    added_via, created_at, last_used_at, refreshed_at, dormant_since";

fn map_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        id: r.get(0)?,
        principal_id: r.get(1)?,
        item_type: r.get(2)?,
        provider: r.get(3)?,
        label: r.get(4)?,
        state: r.get(5)?,
        detail: r.get(6)?,
        plan: r.get(7)?,
        expires_at: r.get(8)?,
        sealed: r.get::<_, i64>(9)? != 0,
        added_via: r.get(10)?,
        created_at: r.get(11)?,
        last_used_at: r.get(12)?,
        refreshed_at: r.get(13)?,
        dormant_since: r.get(14)?,
    })
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Item>, WalletError> {
    Ok(conn
        .query_row(&format!("SELECT {COLS} FROM person_wallet_items WHERE id = ?1"), params![id.trim()], map_item)
        .optional()?)
}

/// One person's item by id: `NotFound` for anyone else's (no oracle).
pub fn get_own(conn: &Connection, principal_id: &str, id: &str) -> Result<Item, WalletError> {
    match get(conn, id)? {
        Some(i) if i.principal_id == principal_id => Ok(i),
        _ => Err(WalletError::NotFound(id.trim().to_string())),
    }
}

pub fn list_for(conn: &Connection, principal_id: &str) -> Result<Vec<Item>, WalletError> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM person_wallet_items WHERE principal_id = ?1 ORDER BY provider, label COLLATE NOCASE"
    ))?;
    let rows = st.query_map(params![principal_id], map_item)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn list_all(conn: &Connection) -> Result<Vec<Item>, WalletError> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM person_wallet_items ORDER BY principal_id, provider, label COLLATE NOCASE"
    ))?;
    let rows = st.query_map([], map_item)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Check type × provider: subscriptions for Claude/Codex/Grok, API keys for
/// all four.
pub fn parse_kind(item_type_: &str, provider: &str) -> Result<(&'static str, Tool), WalletError> {
    let tool = Tool::parse(provider).ok_or_else(|| WalletError::UnknownTool(provider.to_string()))?;
    match item_type_.trim() {
        item_type::LLM_SUBSCRIPTION => {
            if !tool.subscription_supported() {
                return Err(WalletError::Conflict(
                    "unsupported",
                    format!("{} subscriptions aren't supported; add an API key instead", tool.display()),
                ));
            }
            Ok((item_type::LLM_SUBSCRIPTION, tool))
        }
        item_type::LLM_API_KEY => Ok((item_type::LLM_API_KEY, tool)),
        other => Err(WalletError::Conflict(
            "unknown_type",
            format!("unknown wallet item type {other:?} (llm_subscription, llm_api_key)"),
        )),
    }
}

fn new_id() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("{ID_PREFIX}{}", &u[..16])
}

/// Insert a new item (`not_set_up`). Labels are unique per (person,
/// provider), never server-wide.
pub fn create(
    conn: &Connection,
    principal_id: &str,
    item_type_: &str,
    provider: &str,
    label: &str,
    added_via: Option<&str>,
) -> Result<Item, WalletError> {
    let principal_id = principal_id.trim();
    if principal_id.is_empty() {
        return Err(WalletError::NotFound("principal".into()));
    }
    let (t, tool) = parse_kind(item_type_, provider)?;
    let label = llm_accounts::validate_label(label)?;
    let taken: Option<String> = conn
        .query_row(
            "SELECT id FROM person_wallet_items WHERE principal_id = ?1 AND provider = ?2 AND label = ?3 COLLATE NOCASE",
            params![principal_id, tool.as_str(), label],
            |r| r.get(0),
        )
        .optional()?;
    if taken.is_some() {
        return Err(WalletError::DuplicateLabel(label));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO person_wallet_items (id, principal_id, item_type, provider, label, state, added_via, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'not_set_up', ?6, ?7)",
        params![id, principal_id, t, tool.as_str(), label, added_via, now()],
    )?;
    get(conn, &id)?.ok_or(WalletError::NotFound(id))
}

pub fn set_state(conn: &Connection, id: &str, st: &str, detail: Option<&str>) -> Result<(), WalletError> {
    if !state::ALL.contains(&st) {
        return Err(WalletError::InvalidLabel(format!("unknown state {st:?}")));
    }
    let detail = detail.filter(|d| !llm_accounts::looks_like_secret(d));
    conn.execute(
        "UPDATE person_wallet_items SET state = ?2, detail = ?3 WHERE id = ?1",
        params![id, st, detail],
    )?;
    Ok(())
}

fn set_sealed(conn: &Connection, id: &str, sealed: bool) -> Result<(), WalletError> {
    conn.execute("UPDATE person_wallet_items SET sealed = ?2 WHERE id = ?1", params![id, sealed as i64])?;
    Ok(())
}

fn set_meta(conn: &Connection, id: &str, m: &store::CredMeta) -> Result<(), WalletError> {
    let plan = m.plan.clone().filter(|p| !llm_accounts::looks_like_secret(p));
    conn.execute(
        "UPDATE person_wallet_items SET plan = COALESCE(?2, plan), expires_at = ?3 WHERE id = ?1",
        params![id, plan, m.expires_at],
    )?;
    Ok(())
}

pub fn touch_used(conn: &Connection, id: &str) -> Result<(), WalletError> {
    conn.execute("UPDATE person_wallet_items SET last_used_at = ?2 WHERE id = ?1", params![id, now()])?;
    Ok(())
}

pub fn set_refreshed(conn: &Connection, id: &str, at: i64) -> Result<(), WalletError> {
    conn.execute(
        "UPDATE person_wallet_items SET refreshed_at = ?2, detail = NULL WHERE id = ?1",
        params![id, at],
    )?;
    Ok(())
}

/// Mark dormant (`Some(ts)`, kept if already dormant) or awake (`None`).
/// Returns whether anything changed.
pub fn set_dormant(conn: &Connection, principal_id: &str, since: Option<i64>) -> Result<usize, WalletError> {
    let n = match since {
        Some(ts) => conn.execute(
            "UPDATE person_wallet_items SET dormant_since = ?2 WHERE principal_id = ?1 AND dormant_since IS NULL",
            params![principal_id, ts],
        )?,
        None => conn.execute(
            "UPDATE person_wallet_items SET dormant_since = NULL WHERE principal_id = ?1 AND dormant_since IS NOT NULL",
            params![principal_id],
        )?,
    };
    Ok(n)
}

// ── The person pin ──────────────────────────────────────────────────

/// The item a person's personal agents use for `provider`, if any.
pub fn use_for(conn: &Connection, principal_id: &str, provider: &str) -> Result<Option<String>, WalletError> {
    Ok(conn
        .query_row(
            "SELECT item_id FROM person_wallet_use WHERE principal_id = ?1 AND provider = ?2",
            params![principal_id, provider],
            |r| r.get(0),
        )
        .optional()?)
}

/// Every pin of one person: provider → item id.
pub fn uses_of(conn: &Connection, principal_id: &str) -> Result<Vec<(String, String)>, WalletError> {
    let mut st = conn.prepare("SELECT provider, item_id FROM person_wallet_use WHERE principal_id = ?1 ORDER BY provider")?;
    let rows = st.query_map(params![principal_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Choose (`Some(id)`, one of the person's own items for that provider) or
/// clear (`None`) the person pin.
pub fn set_use(conn: &Connection, principal_id: &str, provider: &str, item_id: Option<&str>) -> Result<(), WalletError> {
    let tool = Tool::parse(provider).ok_or_else(|| WalletError::UnknownTool(provider.to_string()))?;
    match item_id {
        None => {
            conn.execute(
                "DELETE FROM person_wallet_use WHERE principal_id = ?1 AND provider = ?2",
                params![principal_id, tool.as_str()],
            )?;
        }
        Some(id) => {
            let item = get_own(conn, principal_id, id)?;
            if item.provider != tool.as_str() {
                return Err(WalletError::Conflict(
                    "wrong_provider",
                    format!("{} is a {} item, not {}", item.label, item.provider, tool.as_str()),
                ));
            }
            conn.execute(
                "INSERT INTO person_wallet_use (principal_id, provider, item_id, set_at) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(principal_id, provider) DO UPDATE SET item_id = excluded.item_id, set_at = excluded.set_at",
                params![principal_id, tool.as_str(), item.id, now()],
            )?;
        }
    }
    Ok(())
}

// ── Personal agents ────────────────────────────────────────────────

/// "This agent workspace is <person>'s own agent, used through <App>."
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonAgent {
    pub project_id: String,
    pub principal_id: String,
    pub service_id: String,
    pub set_by: Option<String>,
    pub set_at: i64,
}

fn map_agent(r: &rusqlite::Row<'_>) -> rusqlite::Result<PersonAgent> {
    Ok(PersonAgent {
        project_id: r.get(0)?,
        principal_id: r.get(1)?,
        service_id: r.get(2)?,
        set_by: r.get(3)?,
        set_at: r.get(4)?,
    })
}

pub fn person_agent(conn: &Connection, project_id: &str) -> Result<Option<PersonAgent>, WalletError> {
    Ok(conn
        .query_row(
            "SELECT project_id, principal_id, service_id, set_by, set_at FROM person_agents WHERE project_id = ?1",
            params![project_id.trim()],
            map_agent,
        )
        .optional()?)
}

pub fn person_agents(conn: &Connection) -> Result<Vec<PersonAgent>, WalletError> {
    let mut st = conn.prepare(
        "SELECT project_id, principal_id, service_id, set_by, set_at FROM person_agents ORDER BY principal_id, project_id",
    )?;
    let rows = st.query_map([], map_agent)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn agents_of(conn: &Connection, principal_id: &str) -> Result<Vec<PersonAgent>, WalletError> {
    Ok(person_agents(conn)?.into_iter().filter(|a| a.principal_id == principal_id).collect())
}

/// Mark (replacing any earlier person) or clear a personal agent. The
/// caller checked the person holds the room through `service_id`.
pub fn set_person_agent(
    conn: &Connection,
    project_id: &str,
    mark: Option<(&str, &str)>,
    by: Option<&str>,
) -> Result<bool, WalletError> {
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err(WalletError::InvalidLabel("workspace is empty".into()));
    }
    Ok(match mark {
        None => conn.execute("DELETE FROM person_agents WHERE project_id = ?1", params![pid])? > 0,
        Some((principal_id, service_id)) => {
            conn.execute(
                "INSERT INTO person_agents (project_id, principal_id, service_id, set_by, set_at) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(project_id) DO UPDATE SET principal_id = excluded.principal_id, \
                 service_id = excluded.service_id, set_by = excluded.set_by, set_at = excluded.set_at",
                params![pid, principal_id, service_id, by, now()],
            )?;
            true
        }
    })
}

// ── Per-App settings ───────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub service_id: String,
    /// The App's guests may use their wallet (`wallet:self`). Default off.
    pub enabled: bool,
    /// When a person's pick isn't usable, run their agent on the company
    /// plan (true) or not at all (false). Default off.
    pub fallback: bool,
    pub set_by: Option<String>,
    pub set_at: Option<i64>,
}

pub fn app_settings(conn: &Connection, service_id: &str) -> Result<AppSettings, WalletError> {
    let svc = service_id.trim();
    let row = conn
        .query_row(
            "SELECT enabled, fallback, set_by, set_at FROM app_wallet_settings WHERE service_id = ?1",
            params![svc],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(match row {
        Some((enabled, fallback, set_by, set_at)) => {
            AppSettings { service_id: svc.to_string(), enabled, fallback, set_by, set_at: Some(set_at) }
        }
        None => AppSettings { service_id: svc.to_string(), ..Default::default() },
    })
}

pub fn set_app_settings(
    conn: &Connection,
    service_id: &str,
    enabled: Option<bool>,
    fallback: Option<bool>,
    by: &str,
) -> Result<(AppSettings, AppSettings), WalletError> {
    let svc = service_id.trim();
    if svc.is_empty() || svc == crate::skin::HOST_APP_ID {
        return Err(WalletError::InvalidLabel("name a published App (host is the box login, not an App)".into()));
    }
    let before = app_settings(conn, svc)?;
    let e = enabled.unwrap_or(before.enabled);
    let f = fallback.unwrap_or(before.fallback);
    conn.execute(
        "INSERT INTO app_wallet_settings (service_id, enabled, fallback, set_by, set_at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(service_id) DO UPDATE SET enabled = excluded.enabled, fallback = excluded.fallback, \
         set_by = excluded.set_by, set_at = excluded.set_at",
        params![svc, e as i64, f as i64, by, now()],
    )?;
    Ok((before, app_settings(conn, svc)?))
}

// ── Sealing ────────────────────────────────────────────────────────

/// What a sealed secret holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealKind {
    /// A subscription's credential file (the tool's own format).
    LoginSlot,
    /// A pasted API key.
    ApiKey,
}

/// The vault's `person` scope, as the wallet needs it. Values cross this
/// trait only between the vault and a slot / a spawn env; never a caller.
pub trait Sealer {
    /// Store (or replace) `bytes` as `wallet-<item>` in scope
    /// `person:<principal>`.
    fn seal(&self, principal_id: &str, item_id: &str, kind: SealKind, bytes: &[u8]) -> Result<(), String>;
    /// The sealed value, `Ok(None)` when there is none.
    fn unseal(&self, principal_id: &str, item_id: &str) -> Result<Option<Vec<u8>>, String>;
    /// Destroy the sealed value (a no-op when there is none).
    fn destroy(&self, principal_id: &str, item_id: &str) -> Result<(), String>;
}

pub fn slot_dir(tool: Tool, item_id: &str) -> PathBuf {
    store::slot_dir(tool, item_id)
}

/// Remove the plaintext credential from a slot (the file, and on macOS the
/// per-slot keychain item Claude may have written).
fn wipe_slot_cred(tool: Tool, item_id: &str) -> Result<(), WalletError> {
    if tool == Tool::Claude && store::keychain_enabled() {
        let svc = store::claude_keychain_service(Some(&slot_dir(tool, item_id)));
        let acct = store::claude_keychain_account();
        if let Ok(true) = store::keychain::exists(&svc, &acct) {
            store::keychain::delete(&svc, &acct).map_err(WalletError::Io)?;
        }
    }
    overwrite_then_unlink(&store::slot_cred_path(tool, item_id))
}

/// Overwrite a credential file with zeros (its whole length, fsynced),
/// then unlink it. Best effort against recovery: on copy-on-write
/// filesystems and SSDs old blocks may survive (the same honest limit as
/// the vault's own key wipe). A missing file is fine.
pub(crate) fn overwrite_then_unlink(path: &Path) -> Result<(), WalletError> {
    use std::io::Write;
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(WalletError::Io(e.to_string())),
        Ok(m) if m.file_type().is_symlink() => {
            // Never write through a link: just remove the link itself.
        }
        Ok(m) => {
            let len = m.len() as usize;
            let mut f = std::fs::OpenOptions::new().write(true).open(path).map_err(|e| WalletError::Io(e.to_string()))?;
            f.write_all(&vec![0u8; len.max(1)]).map_err(|e| WalletError::Io(e.to_string()))?;
            f.sync_all().map_err(|e| WalletError::Io(e.to_string()))?;
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WalletError::Io(e.to_string())),
    }
}

/// Did the vault keep exactly `bytes` for this item? Reads it back.
fn vault_holds(sealer: &dyn Sealer, item: &Item, bytes: &[u8]) -> Result<(), String> {
    match sealer.unseal(&item.principal_id, &item.id) {
        Ok(Some(back)) if back == bytes => Ok(()),
        Ok(Some(back)) => Err(format!("read back {} bytes, not the {} written", back.len(), bytes.len())),
        Ok(None) => Err("read back nothing".into()),
        Err(e) => Err(format!("read back failed ({e})")),
    }
}

/// Is there a plaintext credential in the slot right now?
pub fn slot_has_plaintext(tool: Tool, item_id: &str) -> bool {
    matches!(store::read_slot(tool, item_id), Ok(Some(_)))
}

/// Seal a subscription slot, never losing the login:
/// 1. write its credential (keychain-first on macOS, as Claude reads it)
///    into the vault;
/// 2. read it back and compare byte for byte;
/// 3. only then remove the plaintext (overwrite, then unlink; and Claude's
///    per-slot keychain item).
///
/// A failed write or a read-back that doesn't match keeps the plaintext
/// (the slot stays usable), marks the item unsealed, logs it, and returns
/// `Err(seal_unverified)`. The caller holds the slot lock and has checked
/// no session uses it. `Ok(false)` when the slot holds nothing to seal.
pub fn seal_slot(conn: &Connection, sealer: &dyn Sealer, item: &Item) -> Result<bool, WalletError> {
    let tool = item.tool().ok_or_else(|| WalletError::UnknownTool(item.provider.clone()))?;
    if !item.is_subscription() {
        return Ok(false);
    }
    let Some(bytes) = store::read_slot(tool, &item.id)? else {
        return Ok(false);
    };
    store::validate_cred(tool, &bytes).map_err(|why| store::invalid_cred(tool, "sign-in to seal", why))?;
    set_meta(conn, &item.id, &store::parse_meta(tool, &bytes))?;
    let verified = sealer
        .seal(&item.principal_id, &item.id, SealKind::LoginSlot, &bytes)
        .map_err(|e| format!("vault write failed ({e})"))
        .and_then(|()| vault_holds(sealer, item, &bytes));
    if let Err(why) = verified {
        crate::log_debug!("[wallet] {}: not sealed, plaintext kept: {why}", item.id);
        set_sealed(conn, &item.id, false)?;
        return Err(WalletError::Conflict(
            "seal_unverified",
            format!("could not seal the sign-in in the vault ({why}); it stays in its private folder"),
        ));
    }
    wipe_slot_cred(tool, &item.id)?;
    set_sealed(conn, &item.id, true)?;
    Ok(true)
}

/// Reconcile each item's `sealed` column with the facts: a subscription is
/// sealed when the vault holds its secret and no plaintext is in its slot;
/// an API key when the vault holds it. Items whose vault read fails (the
/// vault is closed) are left as they are. Returns how many changed. The
/// daemon runs it at boot and on every sweep.
pub fn reconcile_sealed(conn: &Connection, sealer: &dyn Sealer) -> Result<usize, WalletError> {
    let mut changed = 0;
    for item in list_all(conn)? {
        let Some(tool) = item.tool() else { continue };
        let in_vault = match sealer.unseal(&item.principal_id, &item.id) {
            Ok(v) => v.is_some(),
            Err(_) => continue,
        };
        let truth = if item.is_subscription() { in_vault && !slot_has_plaintext(tool, &item.id) } else { in_vault };
        if truth != item.sealed {
            set_sealed(conn, &item.id, truth)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// Put a sealed subscription back into its slot for a session (0600). The
/// caller holds the slot lock. `NotSignedIn` when nothing is sealed and the
/// slot is empty.
pub fn unseal_into_slot(sealer: &dyn Sealer, item: &Item) -> Result<(), WalletError> {
    let tool = item.tool().ok_or_else(|| WalletError::UnknownTool(item.provider.clone()))?;
    if slot_has_plaintext(tool, &item.id) {
        return Ok(());
    }
    let bytes = sealer
        .unseal(&item.principal_id, &item.id)
        .map_err(|e| WalletError::Io(format!("could not open the sealed sign-in ({e})")))?
        .ok_or_else(|| WalletError::NotSignedIn(item.id.clone()))?;
    store::write_slot(tool, &item.id, &bytes)
}

/// An API key item's key, for a spawn env only.
pub fn api_key_value(sealer: &dyn Sealer, item: &Item) -> Result<String, WalletError> {
    let b = sealer
        .unseal(&item.principal_id, &item.id)
        .map_err(|e| WalletError::Io(format!("could not open the sealed key ({e})")))?
        .ok_or_else(|| WalletError::NotSignedIn(item.id.clone()))?;
    String::from_utf8(b).map_err(|_| WalletError::Io("the sealed key isn't text".into()))
}

/// Env for a spawn on `item`: the slot home variable, or the API key.
pub fn spawn_env(sealer: &dyn Sealer, item: &Item) -> Result<Vec<(String, String)>, WalletError> {
    let tool = item.tool().ok_or_else(|| WalletError::UnknownTool(item.provider.clone()))?;
    if item.is_api_key() {
        let key = api_key_value(sealer, item)?;
        return Ok(tool.api_key_env().iter().map(|v| (v.to_string(), key.clone())).collect());
    }
    Ok(vec![(tool.home_env_var().to_string(), slot_dir(tool, &item.id).to_string_lossy().into_owned())])
}

/// Make a person slot ready for a session. Codex: a **full private home**
/// (never a shadow of the shared `~/.codex`, so the person's conversations
/// stay theirs), with the file credential store forced. Claude: the shared
/// settings linked and `cwd` trusted (history stays in the slot).
pub fn prepare_person_home(tool: Tool, item_id: &str, cwd: Option<&Path>) -> Result<(), WalletError> {
    let slot = slot_dir(tool, item_id);
    store::ensure_private_dir(&slot).map_err(|e| WalletError::Io(e.to_string()))?;
    match tool {
        Tool::Codex => {
            for f in llm_accounts::pins::CODEX_PRIVATE_FILES {
                let p = slot.join(f);
                if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                    return Err(WalletError::Io(format!("refusing a symlinked {f} in a person's Codex home")));
                }
            }
            store::write_private(&slot.join("config.toml"), b"# Written by K2: a person's private Codex home.\ncli_auth_credentials_store = \"file\"\n")
                .map_err(|e| WalletError::Io(e.to_string()))
        }
        Tool::Claude => {
            let (live, _) = store::live_home(tool);
            llm_accounts::pins::prepare_claude_home(&slot, &live, cwd).map_err(WalletError::Io)
        }
        _ => Ok(()),
    }
}

/// After the vendor CLI's sign-in exits: Claude's per-slot keychain item
/// (macOS) into the slot file, check there is a whole credential, record
/// plan / expiry, seal it, and make it the person pin when they have none
/// for that provider. The sign-in stays usable when sealing fails (the
/// vault is closed): `sealed` stays false and the sweep retries.
pub fn finalize_signin(conn: &Connection, sealer: &dyn Sealer, item_id: &str) -> Result<Item, WalletError> {
    let item = get(conn, item_id)?.ok_or_else(|| WalletError::NotFound(item_id.to_string()))?;
    let tool = item.tool().ok_or_else(|| WalletError::UnknownTool(item.provider.clone()))?;
    let _lock = store::lock_slot(tool, &item.id, std::time::Duration::from_secs(30))?;
    if tool == Tool::Claude && store::keychain_enabled() {
        let svc = store::claude_keychain_service(Some(&slot_dir(tool, &item.id)));
        let acct = store::claude_keychain_account();
        if let Ok(Some(bytes)) = store::keychain::read(&svc, &acct) {
            store::write_slot(tool, &item.id, &bytes)?;
            store::keychain::delete(&svc, &acct).map_err(WalletError::Io)?;
        }
    }
    if !slot_has_plaintext(tool, &item.id) {
        set_state(conn, &item.id, state::NOT_SET_UP, Some("sign-in did not finish"))?;
        return Err(WalletError::NotSignedIn(item.id));
    }
    let _ = store::set_mode(&store::slot_cred_path(tool, &item.id), 0o600);
    set_state(conn, &item.id, state::SIGNED_IN, None)?;
    set_refreshed(conn, &item.id, now())?;
    let fresh = get(conn, &item.id)?.ok_or_else(|| WalletError::NotFound(item.id.clone()))?;
    if let Err(e) = seal_slot(conn, sealer, &fresh) {
        crate::log_debug!("[wallet] {}: sign-in kept unsealed: {}", fresh.id, e.code());
        set_meta(conn, &fresh.id, &store::slot_meta(tool, &fresh.id))?;
    }
    if use_for(conn, &fresh.principal_id, &fresh.provider)?.is_none() {
        set_use(conn, &fresh.principal_id, &fresh.provider, Some(&fresh.id))?;
    }
    get(conn, &fresh.id)?.ok_or(WalletError::NotFound(fresh.id))
}

/// Add an API-key item: the key goes straight into the vault (never a
/// file); the item is `signed_in`, and the person pin when they have none.
pub fn add_api_key(
    conn: &Connection,
    sealer: &dyn Sealer,
    principal_id: &str,
    provider: &str,
    label: &str,
    key: &str,
    added_via: Option<&str>,
) -> Result<Item, WalletError> {
    let k = key.trim();
    if k.len() < 8 || k.len() > 512 || k.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(WalletError::InvalidLabel("the API key must be 8–512 characters with no spaces".into()));
    }
    let item = create(conn, principal_id, item_type::LLM_API_KEY, provider, label, added_via)?;
    // Write, then read back byte for byte: the vault is the key's only home.
    let verified = sealer
        .seal(principal_id, &item.id, SealKind::ApiKey, k.as_bytes())
        .map_err(|e| format!("vault write failed ({e})"))
        .and_then(|()| vault_holds(sealer, &item, k.as_bytes()));
    if let Err(why) = verified {
        let _ = sealer.destroy(principal_id, &item.id);
        conn.execute("DELETE FROM person_wallet_items WHERE id = ?1", params![item.id])?;
        return Err(WalletError::Io(format!("could not seal the key in the vault ({why}); nothing was saved")));
    }
    conn.execute(
        "UPDATE person_wallet_items SET state = 'signed_in', sealed = 1, plan = 'API key' WHERE id = ?1",
        params![item.id],
    )?;
    if use_for(conn, principal_id, &item.provider)?.is_none() {
        set_use(conn, principal_id, &item.provider, Some(&item.id))?;
    }
    get(conn, &item.id)?.ok_or(WalletError::NotFound(item.id))
}

/// Delete an item for good (Rosson #5: deleted, not Trash): the vault
/// secret, the slot folder, the person pins that named it, the row. The
/// caller stops sessions running on it first.
pub fn delete_item(conn: &Connection, sealer: &dyn Sealer, item: &Item) -> Result<(), WalletError> {
    sealer
        .destroy(&item.principal_id, &item.id)
        .map_err(|e| WalletError::Io(format!("could not destroy the sealed secret ({e}); the item was kept")))?;
    if let Some(tool) = item.tool() {
        let _ = wipe_slot_cred(tool, &item.id);
        let dir = slot_dir(tool, &item.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
        }
    }
    conn.execute("DELETE FROM person_wallet_use WHERE item_id = ?1", params![item.id])?;
    conn.execute("DELETE FROM person_wallet_items WHERE id = ?1", params![item.id])?;
    Ok(())
}

/// Drop a first sign-in that never got a credential (cancelled / failed).
pub fn discard_unfinished(conn: &Connection, sealer: &dyn Sealer, item_id: &str) -> Result<(), WalletError> {
    let Some(item) = get(conn, item_id)? else { return Ok(()) };
    if item.state == state::SIGNED_IN || item.sealed {
        return Ok(());
    }
    delete_item(conn, sealer, &item)
}

#[cfg(test)]
mod tests;
