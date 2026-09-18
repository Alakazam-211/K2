//! Agent address minting — K2 Mail S3 (prd-email-server-v1 §7).
//!
//! An address = a Stalwart account (created through [`AddressEngine`],
//! the same narrow-trait seam S2's `DomainEngine` set — tests never
//! touch a network) + a K2 `mail_addresses` row binding it to the
//! OWNING workspace. K2 governance state (ownership, caps, client
//! ids, retire timestamps) lives ONLY in K2's DB; Stalwart holds only
//! the mailbox (module boundary rule #3).
//!
//! **LOCAL-PART RULE (the §7.2 decision, documented here as the
//! SSOT):** `[a-z0-9._-]{1,64}` where
//! - input is TRIMMED and CASE-FOLDED to lowercase first (mirrors the
//!   pre-mortem-#14 domain normalization: one canonical spelling ever
//!   reaches the DB or Stalwart; `Research-Bot` mints `research-bot`),
//! - the first and last character must be `[a-z0-9]` (no leading or
//!   trailing separator),
//! - NO CONSECUTIVE separators (`..`, `--`, `__`, `.-`, …) — kills the
//!   dot-ambiguous spellings mail providers collapse, and
//! - `+` is deliberately NOT mintable: it is the sub-addressing tag
//!   separator — agents get unlimited `bot+tag@` variants for free
//!   AFTER minting `bot@` (§7.1; the docs teach this as the pattern).
//!
//! **CAP SEMANTICS (§11.1.5, BINDING):** the cap counts ACTIVE
//! addresses only — `k2 mail delete` frees the slot immediately (the
//! abuse guard is the rate limits, not the cap). `0` = unlimited. The
//! effective cap is resolved by the route layer through
//! `k2_core::workspace::settings::mail_address_cap_for_path` — the
//! ONLY sanctioned read path — and passed in here so the ops layer
//! stays fixture-testable.
//!
//! **IDEMPOTENT MINTING (§7.2):** same `(owner_project_id, client_id)`
//! returns the existing ACTIVE address as success (`existing: true`)
//! — the AgentMail retry-safe pattern; the idempotency key WINS even
//! if the retried call spells a different local part. A RETIRED row
//! holding the client id has its `client_id` cleared (the partial
//! unique index `idx_mail_addresses_owner_client` has no status
//! filter) so the retry mints fresh; the retired row keeps its
//! address + history for the retention window.
//!
//! **COMPENSATION (no orphans):** mint order is Stalwart create →
//! password vault → K2 row. If the vault or row write fails, the
//! just-created Stalwart account is DESTROYED (and the vaulted secret
//! deleted) before the error surfaces — a mint either fully exists or
//! fully doesn't.
//!
//! **PASSWORD VAULTING:** each account gets a random 32-byte password
//! (`secrets::generate_secret`) stored in the S1 secret store under
//! kind `account-<row-id>` (ref `mailsec_account-<row-id>_<hex>`).
//! The 0072 schema deliberately has NO secret-ref column: V1 never
//! reads the password back (reads proxy through the service account,
//! §8.1) and never surfaces it (§7.1); the row-id-keyed kind lets a
//! later slice (per-address IMAP creds, §18.1) locate the entry by
//! prefix if that ever lands.
//!
//! ── RETENTION SEAM (do NOT build here) ─────────────────────────────
//! Retired addresses keep their mailbox data — and their
//! `mail_addresses` row, which also RESERVES the address string (the
//! table's UNIQUE(address)) — for [`RETENTION_DAYS`] (§12 default:
//! 90). The purge job is a LATER slice: a background loop registered
//! in `main.rs` next to `dns_verify::spawn` (or a heartbeat task)
//! that finds rows with `status='retired' AND retired_at <= now -
//! RETENTION_DAYS*86400`, destroys the Stalwart account
//! (`StalwartClient::account_destroy` — the one non-compensation
//! caller it will ever have), deletes the vaulted password, and
//! deletes the row (freeing the address string for re-minting).
//! Nothing in S3 purges.

use k2_core::db::schema::{MailAddress, MailDomain};
use rusqlite::Connection;

use super::domains;
use super::jmap::StalwartClient;
use super::secrets::{self, SecretStore};

/// §12: retired-address data (and the address-string reservation) is
/// kept this many days, then purged — by the later purge slice, per
/// the retention seam above.
pub const RETENTION_DAYS: u32 = 90;

/// §12: per-address mailbox quota, applied at Stalwart account create.
pub const QUOTA_BYTES: u64 = 1_073_741_824; // 1 GB
pub const QUOTA_MAX_MESSAGES: u64 = 10_000;

/// Local-part bounds (§7.2).
const MAX_LOCAL_PART_LEN: usize = 64;

// ── Errors (mapped onto the route layer's stable {code, hint}) ─────────

/// Operation errors. Codes stay consistent with S2's set
/// (`usage`/`not_found`/`exists`/`not_ready`/`engine`) plus the §11.1.5
/// `cap_reached`.
#[derive(Debug)]
pub enum AddrError {
    /// Bad input → 400 `usage`.
    Usage(String),
    /// Unknown domain / address not in this workspace → 404
    /// `not_found`. (A FOREIGN address is deliberately answered as
    /// not_found — a workspace never learns which addresses other
    /// workspaces hold.)
    NotFound(String),
    /// Local-part collision / already-retired delete → 409 `exists`.
    Exists(String),
    /// Over the active-address cap → 409 `cap_reached`, §11.1.5 text.
    CapReached(String),
    /// No mintable domain / pending with send unlocked → 503 `not_ready`.
    /// Pending + receive-only is mintable (mailboxes before NS cut).
    NotReady(String),
    /// Stalwart said no / transport failed → 502 `engine`.
    Engine(String),
}

// ── Local-part validation (the documented rule above) ──────────────────

fn is_separator(c: char) -> bool {
    matches!(c, '.' | '_' | '-')
}

/// Validate + normalize a local part per the module-header rule.
/// Returns the canonical lowercase form; every rejection names what
/// was wrong AND the rule (comprehension-gate style).
pub fn validate_local_part(raw: &str) -> Result<String, String> {
    let local = raw.trim().to_ascii_lowercase();
    if local.is_empty() {
        return Err("local part must not be empty".to_string());
    }
    if local.len() > MAX_LOCAL_PART_LEN {
        return Err(format!(
            "local part is longer than {MAX_LOCAL_PART_LEN} characters"
        ));
    }
    if let Some(bad) = local
        .chars()
        .find(|c| !c.is_ascii_lowercase() && !c.is_ascii_digit() && !is_separator(*c))
    {
        return Err(format!(
            "invalid character '{bad}' in local part — allowed: a-z, 0-9, '.', '_', '-' \
             (tip: '+tags' don't need minting — any 'name+tag@' already lands in 'name@')"
        ));
    }
    // First/last must be alphanumeric; charset is already vetted, so
    // only the separator case remains.
    let first = local.chars().next().unwrap_or_default();
    let last = local.chars().last().unwrap_or_default();
    if is_separator(first) || is_separator(last) {
        return Err(
            "local part must start and end with a letter or digit (no leading/trailing \
             '.', '_', or '-')"
                .to_string(),
        );
    }
    let consecutive = local
        .as_bytes()
        .windows(2)
        .any(|w| is_separator(w[0] as char) && is_separator(w[1] as char));
    if consecutive {
        return Err(
            "local part must not contain consecutive separators ('..', '--', '__', '.-', …)"
                .to_string(),
        );
    }
    Ok(local)
}

/// Normalize a FULL address for lookups: lowercase local part +
/// punycode-normalized domain (pre-mortem #14 at every boundary).
pub fn normalize_address(raw: &str) -> Result<String, AddrError> {
    let raw = raw.trim();
    let Some((local, domain)) = raw.split_once('@') else {
        return Err(AddrError::Usage(format!(
            "'{raw}' is not a full address — pass local@domain, e.g. bot@acme.dev"
        )));
    };
    let local = local.trim().to_ascii_lowercase();
    if local.is_empty() {
        return Err(AddrError::Usage(format!("'{raw}' has an empty local part")));
    }
    let domain =
        k2_core::mail_domain::normalize_mail_domain(domain).map_err(AddrError::Usage)?;
    Ok(format!("{local}@{domain}"))
}

// ── Engine seam (Stalwart behind a trait — testable without network) ────

/// What S3 needs from Stalwart. Production impl is the JMAP client;
/// tests use a recording fake. `destroy_account` exists ONLY for the
/// mint compensation path (and, later, the retention purge slice) —
/// retire uses `disable_account` (§7.2: data kept).
pub trait AddressEngine {
    fn create_account(
        &self,
        local_part: &str,
        stalwart_domain_id: &str,
        password: &str,
        quota_bytes: u64,
        max_messages: u64,
    ) -> Result<String, String>;
    fn disable_account(&self, stalwart_account_id: &str) -> Result<(), String>;
    fn destroy_account(&self, stalwart_account_id: &str) -> Result<(), String>;
    /// Set the IMAP/SMTP Password credential by Stalwart account id.
    /// Callers must pass `stalwart_account_id`, never username lookup
    /// (`rotate_account_secret` is leftover bootstrap-admin recovery).
    fn set_password(&self, stalwart_account_id: &str, new_secret: &str) -> Result<(), String>;
}

impl AddressEngine for StalwartClient {
    fn create_account(
        &self,
        local_part: &str,
        stalwart_domain_id: &str,
        password: &str,
        quota_bytes: u64,
        max_messages: u64,
    ) -> Result<String, String> {
        self.account_create(
            local_part,
            stalwart_domain_id,
            password,
            quota_bytes,
            max_messages,
        )
    }
    fn disable_account(&self, stalwart_account_id: &str) -> Result<(), String> {
        self.account_disable(stalwart_account_id)
    }
    fn destroy_account(&self, stalwart_account_id: &str) -> Result<(), String> {
        self.account_destroy(stalwart_account_id)
    }
    fn set_password(&self, stalwart_account_id: &str, new_secret: &str) -> Result<(), String> {
        self.account_set_password(stalwart_account_id, new_secret)
    }
}

// ── DB access ───────────────────────────────────────────────────────────

fn project_path_for_id(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get(0),
    )
    .ok()
}

/// Workspace effective quota for a mint. Unknown project → the
/// product fallback ([`QUOTA_BYTES`] / [`QUOTA_MAX_MESSAGES`]). A
/// registered workspace inherits AppSettings when its columns are NULL.
fn mint_quotas_for_project(project_id: &str) -> (u64, u64) {
    match project_path_for_id(project_id) {
        Some(path) => (
            k2_core::workspace::settings::mail_quota_bytes_for_path(&path),
            k2_core::workspace::settings::mail_quota_messages_for_path(&path),
        ),
        None => (QUOTA_BYTES, QUOTA_MAX_MESSAGES),
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const ADDRESS_COLS: &str = "id, address, domain_id, stalwart_account_id, owner_project_id, \
                            client_id, status, created_at, retired_at";

fn map_address_row(r: &rusqlite::Row) -> rusqlite::Result<MailAddress> {
    Ok(MailAddress {
        id: r.get(0)?,
        address: r.get(1)?,
        domain_id: r.get(2)?,
        stalwart_account_id: r.get(3)?,
        owner_project_id: r.get(4)?,
        client_id: r.get(5)?,
        status: r.get(6)?,
        created_at: r.get(7)?,
        retired_at: r.get(8)?,
    })
}

/// Load one row by (already normalized) address — ANY status: retired
/// rows still reserve their address string for the retention window.
pub(crate) fn load_address(conn: &Connection, address: &str) -> Option<MailAddress> {
    conn.query_row(
        &format!("SELECT {ADDRESS_COLS} FROM mail_addresses WHERE address = ?1"),
        rusqlite::params![address],
        map_address_row,
    )
    .ok()
}

/// Load the idempotency match for `(owner, client_id)` — any status
/// (the caller decides what a retired match means).
fn load_by_client_id(conn: &Connection, project_id: &str, client_id: &str) -> Option<MailAddress> {
    conn.query_row(
        &format!(
            "SELECT {ADDRESS_COLS} FROM mail_addresses \
             WHERE owner_project_id = ?1 AND client_id = ?2"
        ),
        rusqlite::params![project_id, client_id],
        map_address_row,
    )
    .ok()
}

/// The cap denominator: ACTIVE rows only (§11.1.5 — delete frees the
/// slot immediately, by construction).
fn count_active(conn: &Connection, project_id: &str) -> u32 {
    conn.query_row(
        "SELECT COUNT(*) FROM mail_addresses WHERE owner_project_id = ?1 AND status = 'active'",
        rusqlite::params![project_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
    .max(0) as u32
}

// ── Domain selection (explicit or the S2 default resolution) ────────────

/// Hosted pending + receive-only is mintable so mailboxes exist before
/// NS is cut to this server. Pending + direct/relay stays locked
/// (send still needs live DNS). Unknown domains are never forced.
fn domain_is_mintable(row: &MailDomain) -> Result<(), AddrError> {
    if row.status == "verified" {
        return Ok(());
    }
    if row.status == "pending" && row.send_mode == "receive-only" {
        return Ok(());
    }
    if row.status == "pending" {
        return Err(AddrError::NotReady(format!(
            "domain '{}' is pending with sendMode '{}' — mint is allowed only on \
             receive-only until DNS verifies; send/direct stay locked. Your human \
             can check it in Settings → Email",
            row.domain, row.send_mode
        )));
    }
    Err(AddrError::NotReady(format!(
        "domain '{}' is not verified yet — records are still propagating or \
         missing; your human can check it in Settings → Email",
        row.domain
    )))
}

/// Pure mint-domain picker (fixture-tested; the impure wrapper feeds
/// it the real rows + the configured default):
/// - explicit domain → must be hosted (else `not_found`). Verified
///   mints as today (any send_mode). Pending + receive-only is
///   allowed; pending + direct/relay is `not_ready`. No `--force`.
/// - no domain → verified default first (`mail_default_domain` when
///   currently Verified, else first Verified); if none, fall through
///   to the configured default or first hosted pending receive-only
///   domain. Hard stop only when nothing mintable exists.
pub fn pick_mint_domain(
    explicit: Option<&str>,
    configured_default: &str,
    all: &[MailDomain],
) -> Result<MailDomain, AddrError> {
    if let Some(raw) = explicit {
        let domain =
            k2_core::mail_domain::normalize_mail_domain(raw).map_err(AddrError::Usage)?;
        let Some(row) = all.iter().find(|d| d.domain == domain) else {
            return Err(AddrError::NotFound(format!(
                "domain '{domain}' is not hosted here — see 'k2 mail domains' for what you \
                 can mint on"
            )));
        };
        domain_is_mintable(row)?;
        return Ok(row.clone());
    }
    let verified: Vec<String> = all
        .iter()
        .filter(|d| d.status == "verified")
        .map(|d| d.domain.clone())
        .collect();
    let pending_ro: Vec<String> = all
        .iter()
        .filter(|d| d.status == "pending" && d.send_mode == "receive-only")
        .map(|d| d.domain.clone())
        .collect();
    let default = domains::resolve_default_domain(configured_default, &verified)
        .or_else(|| domains::resolve_default_domain(configured_default, &pending_ro));
    let Some(default) = default else {
        return Err(AddrError::NotReady(
            "no hosted domain ready to mint on — a pending receive-only domain is \
             enough before NS cut; send stays locked until verified. Ask your human \
             in Settings → Email"
                .to_string(),
        ));
    };
    all.iter()
        .find(|d| d.domain == default)
        .cloned()
        .ok_or_else(|| AddrError::Engine("default domain row vanished".to_string()))
}

// ── Response + error builders ───────────────────────────────────────────

/// §11.1.5 BINDING cap-hit error text, verbatim (dynamic counts).
fn cap_error(used: u32, cap: u32) -> AddrError {
    AddrError::CapReached(format!(
        "address cap reached ({used}/{cap}). Retire one with 'k2 mail delete <addr>' \
         (frees its slot) or ask your human to raise the cap in Settings → Email."
    ))
}

/// The §7.2 collision shapes: suggest `<name>2` on a plain mint; error
/// DISTINCTLY under `--id` (the key names a different address —
/// retrying with a suggestion would break idempotency); name the
/// retention window when a retired row holds the string.
fn collision_error(
    address: &str,
    local: &str,
    domain: &str,
    client_id: Option<&str>,
    holder_status: &str,
) -> AddrError {
    if holder_status == "retired" {
        return AddrError::Exists(format!(
            "address '{address}' was retired and its name is held for the \
             {RETENTION_DAYS}-day retention window — pick a different local part, \
             e.g. '{local}2@{domain}'"
        ));
    }
    match client_id {
        Some(cid) => AddrError::Exists(format!(
            "address '{address}' is taken and does not belong to idempotency id '{cid}' — \
             retry with the id that minted it, or pick a different local part"
        )),
        None => AddrError::Exists(format!(
            "address '{address}' is taken — try '{local}2@{domain}'"
        )),
    }
}

/// One minted/existing address as the create-response JSON (`k2 mail
/// create` prints `created bot@acme.dev (cap 2/5 used)` from this).
fn mint_json(
    row: &MailAddress,
    existing: bool,
    used: u32,
    cap: u32,
    hostname: &str,
    password: Option<&str>,
    domain: &MailDomain,
) -> serde_json::Value {
    let (local, domain_name) = row.address.split_once('@').unwrap_or((row.address.as_str(), ""));
    let host = hostname;
    let mut v = serde_json::json!({
        "ok": true,
        "id": row.id,
        "address": row.address,
        "localPart": local,
        "domain": domain_name,
        "existing": existing,
        "createdAt": row.created_at,
        "cap": { "used": used, "cap": cap },
        "username": row.address,
        "imap": { "host": host, "port": 993, "tls": true },
        "imapStartTls": { "host": host, "port": 143, "startTls": true },
        "submission": { "host": host, "port": 465, "tls": true },
        "jmap": { "host": host, "port": 443, "tls": true },
    });
    if let Some(pw) = password {
        v["password"] = serde_json::Value::String(pw.to_string());
    }
    if domain.status == "pending" {
        v["pending"] = serde_json::Value::Bool(true);
        v["sendMode"] = serde_json::Value::String(domain.send_mode.clone());
        v["note"] = serde_json::Value::String(
            "domain is pending and receive-only — mailbox exists before NS cut; \
             send stays locked until DNS verifies"
                .to_string(),
        );
    }
    v
}

/// IMAP/SMTP client block shown once on mint and on password rotate.
/// IMAPS 993 + IMAP STARTTLS 143; submission 465; JMAP 443.
fn client_block(hostname: &str, username: &str, password: &str) -> serde_json::Value {
    serde_json::json!({
        "username": username,
        "imap": { "host": hostname, "port": 993, "tls": true },
        "imapStartTls": { "host": hostname, "port": 143, "startTls": true },
        "submission": { "host": hostname, "port": 465, "tls": true },
        "jmap": { "host": hostname, "port": 443, "tls": true },
        "password": password,
    })
}

const ROTATE_NOTE: &str =
    "IMAP/SMTP sessions on the mailbox password die; app passwords do not.";

// ── Operations ──────────────────────────────────────────────────────────

/// Mint an address (§7.1/§7.2): validate → pick domain → idempotency →
/// cap (ACTIVE only) → collision → Stalwart account (random vaulted
/// password, §12 quotas) → K2 row, with full compensation on late
/// failure (module header). `raw_local` may be `name` or
/// `name@domain`; an explicit `raw_domain` must agree with any
/// `@domain` spelling. `cap` comes from the route layer's
/// `mail_address_cap_for_path` (0 = unlimited). Mailbox quotas at
/// Stalwart create come from the workspace effective resolver
/// (`mail_quota_bytes_for_path` / `mail_quota_messages_for_path`);
/// missing project row inherits the 1 GB / 10k fallback.
pub fn mint_address(
    engine: &dyn AddressEngine,
    secrets_store: &dyn SecretStore,
    project_id: &str,
    cap: u32,
    raw_local: &str,
    raw_domain: Option<&str>,
    client_id: Option<&str>,
) -> Result<serde_json::Value, AddrError> {
    // `name@domain` spelling: split, and refuse two DIFFERENT domains.
    let (raw_local, raw_domain) = match raw_local.trim().split_once('@') {
        Some((l, at_domain)) => {
            let at_norm = k2_core::mail_domain::normalize_mail_domain(at_domain)
                .map_err(AddrError::Usage)?;
            if let Some(explicit) = raw_domain {
                let explicit_norm = k2_core::mail_domain::normalize_mail_domain(explicit)
                    .map_err(AddrError::Usage)?;
                if explicit_norm != at_norm {
                    return Err(AddrError::Usage(format!(
                        "two different domains given ('{at_norm}' in the address, \
                         '{explicit_norm}' as --domain) — pass one"
                    )));
                }
            }
            (l.to_string(), Some(at_norm))
        }
        None => (raw_local.to_string(), raw_domain.map(String::from)),
    };
    let local = validate_local_part(&raw_local).map_err(AddrError::Usage)?;
    let client_id = client_id.map(str::trim).filter(|s| !s.is_empty());

    // Everything the DB decides, under one lock scope (released before
    // any engine/network call).
    let configured_default = k2_core::app_settings::load().mail_default_domain;
    let hostname = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        super::domains::server_info(&conn)
            .and_then(|i| i.hostname)
            .unwrap_or_default()
    };
    let (domain_row, address, used) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let all = domains::load_all_domains(&conn);
        let domain_row = pick_mint_domain(raw_domain.as_deref(), &configured_default, &all)?;

        // Idempotent `--id` (§7.2): an ACTIVE match is returned as
        // success; a RETIRED match frees its client_id slot (partial
        // unique index) and the mint proceeds fresh.
        if let Some(cid) = client_id {
            if let Some(existing) = load_by_client_id(&conn, project_id, cid) {
                if existing.status == "active" {
                    let used = count_active(&conn, project_id);
                    return Ok(mint_json(
                        &existing,
                        true,
                        used,
                        cap,
                        &hostname,
                        None,
                        &domain_row,
                    ));
                }
                conn.execute(
                    "UPDATE mail_addresses SET client_id = NULL WHERE id = ?1",
                    rusqlite::params![existing.id],
                )
                .map_err(|e| AddrError::Engine(format!("free retired client id: {e}")))?;
            }
        }

        // Cap: ACTIVE rows only (§11.1.5).
        let used = count_active(&conn, project_id);
        if cap != 0 && used >= cap {
            return Err(cap_error(used, cap));
        }

        // Collision pre-check — ANY status (retired rows reserve their
        // address string); the UNIQUE constraint backstops the race.
        let address = format!("{local}@{}", domain_row.domain);
        if let Some(holder) = load_address(&conn, &address) {
            return Err(collision_error(
                &address,
                &local,
                &domain_row.domain,
                client_id,
                &holder.status,
            ));
        }
        (domain_row, address, used)
    };

    let Some(stalwart_domain_id) = domain_row
        .stalwart_domain_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(AddrError::Engine(format!(
            "domain '{}' has no mail-server id — remove and re-add it in Settings → Email",
            domain_row.domain
        )));
    };

    // Stalwart first (nothing local changed if it refuses), then the
    // vault, then the row — compensating backwards on failure.
    let row_id = uuid::Uuid::new_v4().to_string();
    let password = secrets::generate_secret().map_err(AddrError::Engine)?;
    let (quota_bytes, quota_messages) = mint_quotas_for_project(project_id);
    let account_id = engine
        .create_account(
            &local,
            stalwart_domain_id,
            &password,
            quota_bytes,
            quota_messages,
        )
        .map_err(AddrError::Engine)?;
    let secret_ref = match secrets_store.store(&format!("account-{row_id}"), &password) {
        Ok(r) => r,
        Err(e) => {
            let _ = engine.destroy_account(&account_id);
            return Err(AddrError::Engine(format!(
                "could not vault the account password: {e}"
            )));
        }
    };

    let created_at = now_secs();
    let inserted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, client_id, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7)",
            rusqlite::params![row_id, address, domain_row.id, account_id, project_id, client_id, created_at],
        )
    };
    if let Err(e) = inserted {
        // COMPENSATION (module header): no orphan Stalwart account, no
        // orphan vault entry.
        let _ = engine.destroy_account(&account_id);
        let _ = secrets_store.delete(&secret_ref);
        return Err(if e.to_string().contains("UNIQUE") {
            // Lost the race to the same address string.
            collision_error(&address, &local, &domain_row.domain, client_id, "active")
        } else {
            AddrError::Engine(format!("address insert: {e}"))
        });
    }

    let row = MailAddress {
        id: row_id,
        address,
        domain_id: domain_row.id.clone(),
        stalwart_account_id: Some(account_id),
        owner_project_id: project_id.to_string(),
        client_id: client_id.map(String::from),
        status: "active".to_string(),
        created_at,
        retired_at: None,
    };
    Ok(mint_json(
        &row,
        false,
        used + 1,
        cap,
        &hostname,
        Some(&password),
        &domain_row,
    ))
}

/// Retire an address (§7.2): Stalwart account DISABLED (never
/// destroyed — retention window §12), K2 row → `retired` +
/// `retired_at`. Frees the cap slot by construction (the cap counts
/// active rows). Ownership = the caller's workspace must hold the row;
/// the owner's Settings table retires any address by addressing the
/// HOLDER's workspace (its `holderProjectId` from the `?all=true`
/// list) — the same declarative-identity model every workspace-scoped
/// route uses.
pub fn retire_address(
    engine: Option<&dyn AddressEngine>,
    project_id: &str,
    raw_address: &str,
) -> Result<serde_json::Value, AddrError> {
    let address = normalize_address(raw_address)?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        load_address(&conn, &address)
    };
    let Some(row) = row else {
        return Err(AddrError::NotFound(format!(
            "no address '{address}' in this workspace"
        )));
    };
    // Foreign address → the SAME not_found as unknown (no existence
    // leak across workspaces).
    if row.owner_project_id != project_id {
        return Err(AddrError::NotFound(format!(
            "no address '{address}' in this workspace"
        )));
    }
    if row.status != "active" {
        return Err(AddrError::Exists(format!(
            "address '{address}' is already retired — nothing to delete"
        )));
    }

    // Stalwart first: if the disable fails, nothing local changes
    // (fail closed — a "retired" K2 row whose mailbox still receives
    // would be a lie).
    if let Some(account_id) = row.stalwart_account_id.as_deref() {
        let Some(engine) = engine else {
            return Err(AddrError::Engine(
                "the mail server is unavailable — start it in Settings → Email, then retry \
                 the delete"
                    .to_string(),
            ));
        };
        engine.disable_account(account_id).map_err(AddrError::Engine)?;
    }

    let retired_at = now_secs();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_addresses SET status = 'retired', retired_at = ?1 WHERE id = ?2",
            rusqlite::params![retired_at, row.id],
        )
        .map_err(|e| AddrError::Engine(format!("address retire: {e}")))?;
    }
    Ok(serde_json::json!({
        "ok": true,
        "address": address,
        "status": "retired",
        "retiredAt": retired_at,
    }))
}

/// Rotate the IMAP/SMTP secret for an **active hosted** address.
/// Lookup is by address only (not minting-workspace, not canManage).
/// Auth is the route gate (owner/admin or mail_manage).
///
/// Stalwart `account_set_password` first; vault `store_exact("account-{row.id}")`
/// overwrites the deterministic key (never `store()`, which orphans a
/// random ref). Vault failure still returns the once password — the
/// engine already accepted it.
pub fn rotate_address_password(
    engine: &dyn AddressEngine,
    secrets_store: &dyn SecretStore,
    raw_address: &str,
) -> Result<serde_json::Value, AddrError> {
    let address = normalize_address(raw_address)?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        load_address(&conn, &address)
    };
    let Some(row) = row else {
        return Err(AddrError::NotFound(format!(
            "no hosted address '{address}'"
        )));
    };
    if row.status != "active" {
        return Err(AddrError::NotFound(format!(
            "no hosted address '{address}'"
        )));
    }
    let Some(account_id) = row
        .stalwart_account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(AddrError::Engine(format!(
            "address '{address}' has no mail-server account"
        )));
    };

    let hostname = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        super::domains::server_info(&conn)
            .and_then(|i| i.hostname)
            .unwrap_or_default()
    };

    let password = secrets::generate_secret().map_err(AddrError::Engine)?;
    engine
        .set_password(account_id, &password)
        .map_err(AddrError::Engine)?;
    // Deterministic vault key so a later rotate overwrites. `store()`
    // mints a random `mailsec_*` ref the row never records.
    if let Err(e) = secrets_store.store_exact(&format!("account-{}", row.id), &password) {
        k2_core::log_debug!(
            "[mail] address password vault write failed after Stalwart set for {address}: {e}"
        );
    }

    let mut v = client_block(&hostname, &row.address, &password);
    v["ok"] = serde_json::Value::Bool(true);
    v["address"] = serde_json::Value::String(row.address.clone());
    v["note"] = serde_json::Value::String(ROTATE_NOTE.to_string());
    Ok(v)
}

/// The caller's view (`k2 mail addresses`): ACTIVE rows only +
/// `createdAt` + the cap usage `{used, cap}` (cap 0 = unlimited —
/// callers render "unlimited").
pub fn list_for_project_json(project_id: &str, cap: u32) -> serde_json::Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(&format!(
        "SELECT {ADDRESS_COLS} FROM mail_addresses \
         WHERE owner_project_id = ?1 AND status = 'active' ORDER BY created_at, address"
    )) {
        Ok(s) => s,
        Err(e) => {
            return serde_json::json!({
                "ok": false,
                "error": { "code": "engine", "hint": format!("address list: {e}") },
            })
        }
    };
    let rows: Vec<MailAddress> = stmt
        .query_map(rusqlite::params![project_id], map_address_row)
        .map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default();
    let used = rows.len();
    let addresses: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "address": r.address,
                "createdAt": r.created_at,
            })
        })
        .collect();
    serde_json::json!({
        "ok": true,
        "addresses": addresses,
        "cap": { "used": used, "cap": cap },
    })
}

/// The owner table (`?all=true` — the Settings→Email addresses table):
/// EVERY address, retired included, with the holder workspace.
pub fn list_all_json() -> serde_json::Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT a.id, a.address, a.status, a.created_at, a.retired_at, a.owner_project_id, \
                p.name, p.path \
         FROM mail_addresses a LEFT JOIN projects p ON p.id = a.owner_project_id \
         ORDER BY a.created_at, a.address",
    ) {
        Ok(s) => s,
        Err(e) => {
            return serde_json::json!({
                "ok": false,
                "error": { "code": "engine", "hint": format!("address list: {e}") },
            })
        }
    };
    let addresses: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            let name: Option<String> = r.get(6)?;
            let path: Option<String> = r.get(7)?;
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "address": r.get::<_, String>(1)?,
                "status": r.get::<_, String>(2)?,
                "createdAt": r.get::<_, i64>(3)?,
                "retiredAt": r.get::<_, Option<i64>>(4)?,
                "holderProjectId": r.get::<_, String>(5)?,
                // Display name for the table: workspace name, else the
                // path basename, else null (project row purged — the
                // address record survives as auditable history, 0072).
                "holderWorkspace": name.or_else(|| {
                    path.as_deref()
                        .and_then(|p| p.rsplit('/').next())
                        .map(String::from)
                }),
            }))
        })
        .map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default();
    serde_json::json!({ "ok": true, "addresses": addresses })
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — fake engine + fake vault, shared test DB,
// no network (house rules)
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── Fakes ──

    /// Recording engine. `on_create` runs MID-FLOW (after the
    /// collision pre-check, before the K2 row insert) — the
    /// compensation test uses it to model the lost-the-race DB
    /// failure honestly, with no cfg hooks in production code.
    pub(crate) struct FakeAddrEngine {
        pub created: Mutex<Vec<(String, String)>>, // (local, domain_id)
        pub created_quotas: Mutex<Vec<(u64, u64)>>, // (bytes, messages)
        pub disabled: Mutex<Vec<String>>,
        pub destroyed: Mutex<Vec<String>>,
        pub passwords_set: Mutex<Vec<(String, String)>>, // (account_id, secret)
        pub fail_create: bool,
        pub fail_disable: bool,
        pub fail_set_password: bool,
        pub on_create: Option<Box<dyn Fn() + Send + Sync>>,
        next_id: Mutex<u32>,
    }

    impl FakeAddrEngine {
        pub(crate) fn ok() -> Self {
            Self {
                created: Mutex::new(Vec::new()),
                created_quotas: Mutex::new(Vec::new()),
                disabled: Mutex::new(Vec::new()),
                destroyed: Mutex::new(Vec::new()),
                passwords_set: Mutex::new(Vec::new()),
                fail_create: false,
                fail_disable: false,
                fail_set_password: false,
                on_create: None,
                next_id: Mutex::new(0),
            }
        }
    }

    impl AddressEngine for FakeAddrEngine {
        fn create_account(
            &self,
            local_part: &str,
            stalwart_domain_id: &str,
            password: &str,
            quota_bytes: u64,
            max_messages: u64,
        ) -> Result<String, String> {
            assert_eq!(password.len(), 64, "32 random bytes as hex");
            if self.fail_create {
                return Err("Account/set create rejected — forbidden".to_string());
            }
            if let Some(hook) = &self.on_create {
                hook();
            }
            self.created
                .lock()
                .unwrap()
                .push((local_part.to_string(), stalwart_domain_id.to_string()));
            self.created_quotas
                .lock()
                .unwrap()
                .push((quota_bytes, max_messages));
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            Ok(format!("acc-{n}"))
        }
        fn disable_account(&self, id: &str) -> Result<(), String> {
            if self.fail_disable {
                return Err("Account/set update rejected — notFound".to_string());
            }
            self.disabled.lock().unwrap().push(id.to_string());
            Ok(())
        }
        fn destroy_account(&self, id: &str) -> Result<(), String> {
            self.destroyed.lock().unwrap().push(id.to_string());
            Ok(())
        }
        fn set_password(&self, stalwart_account_id: &str, new_secret: &str) -> Result<(), String> {
            assert_eq!(new_secret.len(), 64, "32 random bytes as hex");
            if self.fail_set_password {
                return Err("Account/set password rejected".to_string());
            }
            self.passwords_set
                .lock()
                .unwrap()
                .push((stalwart_account_id.to_string(), new_secret.to_string()));
            Ok(())
        }
    }

    /// Recording secret vault (never the real ~/.k2 file).
    #[derive(Default)]
    pub(crate) struct FakeVault {
        pub stored: Mutex<Vec<(String, String)>>, // (kind, secret)
        pub deleted: Mutex<Vec<String>>,
        pub fail_store: bool,
    }

    impl SecretStore for FakeVault {
        fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
            if self.fail_store {
                return Err("vault unavailable".to_string());
            }
            self.stored
                .lock()
                .unwrap()
                .push((kind.to_string(), secret.to_string()));
            Ok(format!("mailsec_{kind}_test"))
        }
        fn resolve(&self, _sref: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn delete(&self, sref: &str) -> Result<(), String> {
            self.deleted.lock().unwrap().push(sref.to_string());
            Ok(())
        }
        fn store_exact(&self, key: &str, secret: &str) -> Result<(), String> {
            if self.fail_store {
                return Err("vault unavailable".to_string());
            }
            self.stored
                .lock()
                .unwrap()
                .push((key.to_string(), secret.to_string()));
            Ok(())
        }
    }

    // ── DB seeding helpers (unique names per test — shared DB) ──

    fn unique(label: &str) -> String {
        format!("{label}-{}", uuid::Uuid::new_v4().simple())
    }

    /// Seed a mail_domains row directly (S2 owns the add flow; here we
    /// only need rows to exist) and return its id.
    fn seed_domain(domain: &str, status: &str, stalwart_id: Option<&str>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, domain, stalwart_id, status, now_secs()],
        )
        .expect("seed domain");
        id
    }

    fn cleanup_domain(domain: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE domain_id IN \
             (SELECT id FROM mail_domains WHERE domain = ?1)",
            rusqlite::params![domain],
        );
        let _ = conn.execute(
            "DELETE FROM mail_domains WHERE domain = ?1",
            rusqlite::params![domain],
        );
    }

    fn address_row(address: &str) -> Option<MailAddress> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        load_address(&conn, address)
    }

    // ── Local-part rule ──

    #[test]
    fn local_part_rule_accepts_normalizes_and_rejects() {
        // Accept + normalize (trim, casefold).
        for (input, want) in [
            ("bot", "bot"),
            ("Research-Bot", "research-bot"),
            ("  a  ", "a"),
            ("x2", "x2"),
            ("a.b-c_d", "a.b-c_d"),
            ("9lives", "9lives"),
        ] {
            assert_eq!(validate_local_part(input).expect(input), want);
        }
        // Exactly 64 ok; 65 rejected.
        let max = "a".repeat(64);
        assert_eq!(validate_local_part(&max).expect("64 chars"), max);
        assert!(validate_local_part(&"a".repeat(65)).is_err());

        // Rejections, each with a named reason.
        for bad in [
            "",
            "   ",
            ".bot",
            "bot.",
            "-bot",
            "bot-",
            "_bot",
            "bot_",
            "a..b",
            "a--b",
            "a__b",
            "a.-b",
            "a_.b",
            "bób",
            "bot tag",
            "bot+tag", // '+' is the sub-addressing separator, never minted
            "bot@x",   // full addresses are split BEFORE validation
        ] {
            assert!(validate_local_part(bad).is_err(), "must reject {bad:?}");
        }
        // The '+' rejection teaches plus-addressing (§7.1 docs tie-in).
        let err = validate_local_part("bot+tag").unwrap_err();
        assert!(err.contains("+tag"), "{err}");
    }

    #[test]
    fn normalize_address_lowercases_and_punycodes() {
        assert_eq!(
            normalize_address("  Bot@ACME.dev.  ").expect("normalized"),
            "bot@acme.dev"
        );
        // IDN domain part → A-label (pre-mortem #14).
        let n = normalize_address("bot@bücher.example").expect("idn");
        assert_eq!(n, "bot@xn--bcher-kva.example");
        for bad in ["botacme.dev", "@acme.dev", "bot@", "bot@not a domain"] {
            assert!(
                matches!(normalize_address(bad), Err(AddrError::Usage(_))),
                "must reject {bad:?}"
            );
        }
    }

    // ── Domain picking (pure) ──

    fn domain_fixture(domain: &str, status: &str) -> MailDomain {
        domain_with_mode(domain, status, "receive-only")
    }

    fn domain_with_mode(domain: &str, status: &str, send_mode: &str) -> MailDomain {
        MailDomain {
            id: format!("d-{domain}"),
            domain: domain.to_string(),
            stalwart_domain_id: Some(format!("stw-{domain}")),
            send_mode: send_mode.to_string(),
            relay_config_id: None,
            status: status.to_string(),
            dns_status_json: None,
            verified_at: None,
            last_checked_at: None,
            created_at: 100,
        }
    }

    #[test]
    fn pick_mint_domain_explicit_and_default_paths() {
        let all = vec![
            domain_fixture("pending.example", "pending"),
            domain_fixture("acme.dev", "verified"),
            domain_fixture("beta.example", "verified"),
        ];
        // Explicit verified (un-normalized spelling still hits).
        let d = pick_mint_domain(Some("ACME.dev."), "", &all).expect("verified pick");
        assert_eq!(d.domain, "acme.dev");
        // Explicit pending + receive-only → allowed (boxes before NS cut).
        let d = pick_mint_domain(Some("pending.example"), "", &all).expect("pending receive-only");
        assert_eq!(d.domain, "pending.example");
        // Explicit pending + send_mode direct → still not_ready (send locked).
        let pending_direct = vec![domain_with_mode("pending-send.example", "pending", "direct")];
        match pick_mint_domain(Some("pending-send.example"), "", &pending_direct) {
            Err(AddrError::NotReady(hint)) => {
                assert!(hint.contains("Settings → Email"), "{hint}");
                assert!(hint.contains("direct") || hint.contains("send"), "{hint}");
            }
            other => panic!("must be NotReady, got {other:?}"),
        }
        // Explicit unknown → not_found naming `k2 mail domains`.
        match pick_mint_domain(Some("ghost.example"), "", &all) {
            Err(AddrError::NotFound(hint)) => assert!(hint.contains("k2 mail domains"), "{hint}"),
            other => panic!("must be NotFound, got {other:?}"),
        }
        // Garbage → usage.
        assert!(matches!(
            pick_mint_domain(Some("not a domain"), "", &all),
            Err(AddrError::Usage(_))
        ));
        // Default: first verified when unconfigured; the configured
        // default wins when it is verified; a stale configured value
        // falls back (S2's resolve_default_domain contract). Verified
        // is preferred even when a pending receive-only domain exists.
        assert_eq!(pick_mint_domain(None, "", &all).expect("first").domain, "acme.dev");
        assert_eq!(
            pick_mint_domain(None, "beta.example", &all).expect("configured").domain,
            "beta.example"
        );
        assert_eq!(
            pick_mint_domain(None, "gone.example", &all).expect("stale").domain,
            "acme.dev"
        );
        assert_eq!(
            pick_mint_domain(None, "pending.example", &all)
                .expect("verified still preferred")
                .domain,
            "acme.dev"
        );
        // No verified, one pending receive-only hosted → that domain.
        let unverified = vec![domain_fixture("pending.example", "pending")];
        assert_eq!(
            pick_mint_domain(None, "", &unverified)
                .expect("pending receive-only default")
                .domain,
            "pending.example"
        );
        // Configured default among pending receive-only.
        let two_pending = vec![
            domain_fixture("alpha.example", "pending"),
            domain_fixture("beta.example", "pending"),
        ];
        assert_eq!(
            pick_mint_domain(None, "beta.example", &two_pending)
                .expect("configured pending receive-only")
                .domain,
            "beta.example"
        );
        // No verified, only pending + direct → still not_ready.
        match pick_mint_domain(None, "", &pending_direct) {
            Err(AddrError::NotReady(hint)) => {
                assert!(hint.contains("Settings → Email"), "{hint}");
            }
            other => panic!("must be NotReady, got {other:?}"),
        }
    }

    // ── Mint ──

    #[test]
    fn mint_happy_path_creates_account_vaults_password_and_persists() {
        let domain = unique("mint") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");

        let v = mint_address(&engine, &vault, &project, 5, "Scout", Some(&domain), None)
            .expect("mint ok");
        assert_eq!(v["ok"], true);
        assert_eq!(v["address"], format!("scout@{domain}"), "casefolded at the boundary");
        assert_eq!(v["localPart"], "scout");
        assert_eq!(v["domain"], domain);
        assert_eq!(v["existing"], false);
        assert_eq!(v["cap"]["used"], 1);
        assert_eq!(v["cap"]["cap"], 5);
        assert!(v["createdAt"].as_i64().unwrap() > 0);
        assert!(v["password"].as_str().unwrap().len() >= 32, "once password");
        assert_eq!(v["username"], format!("scout@{domain}"));
        assert_eq!(v["imap"]["port"], 993);
        assert_eq!(v["imap"]["tls"], true);
        assert_eq!(v["imapStartTls"]["port"], 143);
        assert_eq!(v["submission"]["port"], 465);
        assert_eq!(v["jmap"]["port"], 443);

        // Engine got local + STALWART domain id (never the K2 row id).
        assert_eq!(
            engine.created.lock().unwrap().as_slice(),
            [("scout".to_string(), "stw-1".to_string())]
        );
        // Vault entry keyed by the row id (module-header convention).
        let row = address_row(&format!("scout@{domain}")).expect("row persisted");
        assert_eq!(row.status, "active");
        assert_eq!(row.domain_id, domain_id);
        assert_eq!(row.owner_project_id, project);
        assert_eq!(row.stalwart_account_id.as_deref(), Some("acc-1"));
        let stored = vault.stored.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].0, format!("account-{}", row.id));
        assert_eq!(stored[0].1.len(), 64, "32-byte random password, hex");
        cleanup_domain(&domain);
    }

    #[test]
    fn mint_on_pending_receive_only_creates_the_address() {
        let domain = unique("pending-mint") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain, "pending", Some("stw-pending"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");

        let v = mint_address(&engine, &vault, &project, 5, "precut", Some(&domain), None)
            .expect("mint on pending receive-only");
        assert_eq!(v["ok"], true);
        assert_eq!(v["address"], format!("precut@{domain}"));
        assert_eq!(v["existing"], false);
        assert_eq!(v["pending"], true);
        assert_eq!(v["sendMode"], "receive-only");
        let note = v["note"].as_str().unwrap_or("");
        assert!(note.contains("pending"), "{note}");
        assert!(note.contains("receive-only"), "{note}");

        assert_eq!(
            engine.created.lock().unwrap().as_slice(),
            [("precut".to_string(), "stw-pending".to_string())]
        );
        assert_eq!(
            engine.created_quotas.lock().unwrap().as_slice(),
            [(QUOTA_BYTES, QUOTA_MAX_MESSAGES)],
            "pending mint still uses fallback 1GB/10k without a workspace override"
        );
        let row = address_row(&format!("precut@{domain}")).expect("row persisted");
        assert_eq!(row.status, "active");
        assert_eq!(row.domain_id, domain_id);
        assert_eq!(row.owner_project_id, project);
        cleanup_domain(&domain);
    }

    #[test]
    fn mint_uses_workspace_quota_override() {
        let domain = unique("quota-mint") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-q"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let path = format!("/tmp/mail-quota-mint-{}", uuid::Uuid::new_v4());
        let project = {
            let id = uuid::Uuid::new_v4().to_string();
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, mail_quota_bytes, mail_quota_messages) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, "quota-mint", path, 3_221_225_472i64, 40_000i64],
            )
            .expect("insert project with quota override");
            id
        };

        mint_address(&engine, &vault, &project, 5, "bigbox", Some(&domain), None)
            .expect("mint with workspace quota");
        assert_eq!(
            engine.created_quotas.lock().unwrap().as_slice(),
            [(3_221_225_472, 40_000)],
            "mint must pass workspace override into Account/set, not the 1GB/10k fallback"
        );
        cleanup_domain(&domain);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM projects WHERE path = ?1", rusqlite::params![path]);
        }
    }

    #[test]
    fn mint_zero_quota_is_unlimited() {
        let domain = unique("quota-zero") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-z"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let path = format!("/tmp/mail-quota-zero-{}", uuid::Uuid::new_v4());
        let project = {
            let id = uuid::Uuid::new_v4().to_string();
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, mail_quota_bytes, mail_quota_messages) \
                 VALUES (?1, ?2, ?3, 0, 0)",
                rusqlite::params![id, "quota-zero", path],
            )
            .expect("insert unlimited quota project");
            id
        };

        mint_address(&engine, &vault, &project, 5, "openbox", Some(&domain), None)
            .expect("mint with 0 = unlimited");
        assert_eq!(
            engine.created_quotas.lock().unwrap().as_slice(),
            [(0, 0)],
            "0 must pass through to Stalwart as unlimited"
        );
        cleanup_domain(&domain);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM projects WHERE path = ?1", rusqlite::params![path]);
        }
    }

    #[test]
    fn mint_at_domain_spelling_and_domain_disagreement() {
        let domain = unique("atspell") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");

        // `name@domain` in the local slot works (CLI surface §11).
        let v = mint_address(
            &engine,
            &vault,
            &project,
            0,
            &format!("bot@{domain}"),
            None,
            None,
        )
        .expect("mint ok");
        assert_eq!(v["address"], format!("bot@{domain}"));

        // Same domain twice (either spelling) is fine…
        let v = mint_address(
            &engine,
            &vault,
            &project,
            0,
            &format!("bot2@{}", domain.to_uppercase()),
            Some(&domain),
            None,
        )
        .expect("agreeing spellings");
        assert_eq!(v["address"], format!("bot2@{domain}"));

        // …two DIFFERENT domains is a usage error naming both.
        match mint_address(
            &engine,
            &vault,
            &project,
            0,
            &format!("bot3@{domain}"),
            Some("other.example"),
            None,
        ) {
            Err(AddrError::Usage(hint)) => {
                assert!(hint.contains(&domain) && hint.contains("other.example"), "{hint}")
            }
            other => panic!("must be Usage, got {other:?}"),
        }
        cleanup_domain(&domain);
    }

    #[test]
    fn cap_counts_active_only_and_retire_frees_the_slot() {
        let domain = unique("cap") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");

        // Fill a cap of 2.
        for name in ["one", "two"] {
            mint_address(&engine, &vault, &project, 2, name, Some(&domain), None)
                .expect("under cap");
        }
        // Over cap → the §11.1.5 BINDING text, verbatim.
        match mint_address(&engine, &vault, &project, 2, "three", Some(&domain), None) {
            Err(AddrError::CapReached(hint)) => assert_eq!(
                hint,
                "address cap reached (2/2). Retire one with 'k2 mail delete <addr>' \
                 (frees its slot) or ask your human to raise the cap in Settings → Email.",
            ),
            other => panic!("must be CapReached, got {other:?}"),
        }
        // No engine call, no vault entry for the refused mint.
        assert_eq!(engine.created.lock().unwrap().len(), 2);
        assert_eq!(vault.stored.lock().unwrap().len(), 2);

        // Retire one → the slot frees IMMEDIATELY (§11.1.5).
        retire_address(Some(&engine), &project, &format!("one@{domain}")).expect("retire");
        let v = mint_address(&engine, &vault, &project, 2, "three", Some(&domain), None)
            .expect("slot freed by retire");
        assert_eq!(v["cap"]["used"], 2);

        // cap 0 = unlimited.
        let v = mint_address(&engine, &vault, &project, 0, "four", Some(&domain), None)
            .expect("unlimited");
        assert_eq!(v["cap"]["cap"], 0);
        cleanup_domain(&domain);
    }

    #[test]
    fn idempotent_client_id_returns_existing_active_and_refreshes_after_retire() {
        let domain = unique("idem") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");

        let v1 = mint_address(&engine, &vault, &project, 5, "bot", Some(&domain), Some("ci-1"))
            .expect("first mint");
        assert_eq!(v1["existing"], false);

        // Retry with the same id → the EXISTING address, no second
        // Stalwart account, even under a different requested name
        // (idempotency key wins — AgentMail pattern).
        let v2 = mint_address(&engine, &vault, &project, 5, "other-name", Some(&domain), Some("ci-1"))
            .expect("idempotent retry");
        assert_eq!(v2["existing"], true);
        assert_eq!(v2["address"], v1["address"]);
        assert_eq!(v2["id"], v1["id"]);
        assert!(
            v2.get("password").is_none() || v2["password"].is_null(),
            "C26: existing --id must not re-print the password: {v2}"
        );
        assert_eq!(engine.created.lock().unwrap().len(), 1, "no second account");

        // A DIFFERENT workspace may reuse the same client id (the
        // unique index is per-owner).
        let other_project = unique("proj");
        let v3 = mint_address(&engine, &vault, &other_project, 5, "bot2", Some(&domain), Some("ci-1"))
            .expect("other workspace, same id");
        assert_eq!(v3["existing"], false);

        // Retire the original → the retired row frees its client_id
        // slot and a retry mints FRESH (retired row keeps its address
        // + history, loses only the idempotency binding).
        retire_address(Some(&engine), &project, v1["address"].as_str().unwrap())
            .expect("retire");
        let v4 = mint_address(&engine, &vault, &project, 5, "bot-next", Some(&domain), Some("ci-1"))
            .expect("fresh mint after retire");
        assert_eq!(v4["existing"], false);
        assert_eq!(v4["address"], format!("bot-next@{domain}"));
        let old = address_row(v1["address"].as_str().unwrap()).expect("retired row kept");
        assert_eq!(old.status, "retired");
        assert_eq!(old.client_id, None, "client id slot freed");
        cleanup_domain(&domain);
    }

    #[test]
    fn collisions_suggest_name2_error_distinctly_under_id_and_name_retention() {
        let domain = unique("coll") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");

        mint_address(&engine, &vault, &project, 5, "bot", Some(&domain), None).expect("mint");

        // Plain collision → suggest `<name>2` (§7.2).
        match mint_address(&engine, &vault, &project, 5, "bot", Some(&domain), None) {
            Err(AddrError::Exists(hint)) => {
                assert!(hint.contains(&format!("bot2@{domain}")), "{hint}")
            }
            other => panic!("must be Exists, got {other:?}"),
        }
        // Under --id with a DIFFERENT (unmatched) key → the DISTINCT
        // error (no `<name>2` suggestion — §7.2/§11.1.10).
        match mint_address(&engine, &vault, &project, 5, "bot", Some(&domain), Some("ci-x")) {
            Err(AddrError::Exists(hint)) => {
                assert!(hint.contains("idempotency id 'ci-x'"), "{hint}");
                assert!(!hint.contains("bot2@"), "no rename suggestion under --id: {hint}");
            }
            other => panic!("must be Exists, got {other:?}"),
        }
        // A RETIRED holder names the retention window.
        retire_address(Some(&engine), &project, &format!("bot@{domain}")).expect("retire");
        match mint_address(&engine, &vault, &project, 5, "bot", Some(&domain), None) {
            Err(AddrError::Exists(hint)) => {
                assert!(hint.contains("90-day retention window"), "{hint}");
            }
            other => panic!("must be Exists, got {other:?}"),
        }
        cleanup_domain(&domain);
    }

    #[test]
    fn engine_refusal_stores_nothing() {
        let domain = unique("refuse") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine { fail_create: true, ..FakeAddrEngine::ok() };
        let vault = FakeVault::default();
        match mint_address(&engine, &vault, &unique("proj"), 5, "bot", Some(&domain), None) {
            Err(AddrError::Engine(hint)) => assert!(hint.contains("forbidden"), "{hint}"),
            other => panic!("must be Engine, got {other:?}"),
        }
        assert!(address_row(&format!("bot@{domain}")).is_none(), "no row");
        assert!(vault.stored.lock().unwrap().is_empty(), "no vault entry");
        cleanup_domain(&domain);
    }

    /// THE compensation test (task-binding): Stalwart create succeeds,
    /// the K2 row write then fails (modeled honestly as losing the
    /// UNIQUE(address) race — a conflicting row lands mid-flow via the
    /// fake's on_create hook) → the just-created account is DESTROYED
    /// and the vaulted secret deleted. No orphans.
    #[test]
    fn db_write_failure_destroys_the_stalwart_account() {
        let domain = unique("comp") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain, "verified", Some("stw-1"));
        let racer_address = format!("bot@{domain}");
        let racer_id = uuid::Uuid::new_v4().to_string();
        let engine = FakeAddrEngine {
            on_create: Some(Box::new({
                let racer_address = racer_address.clone();
                let racer_id = racer_id.clone();
                let domain_id = domain_id.clone();
                move || {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    conn.execute(
                        "INSERT INTO mail_addresses (id, address, domain_id, \
                         owner_project_id, status, created_at) \
                         VALUES (?1, ?2, ?3, 'racer-proj', 'active', 1)",
                        rusqlite::params![racer_id, racer_address, domain_id],
                    )
                    .expect("racer insert");
                }
            })),
            ..FakeAddrEngine::ok()
        };
        let vault = FakeVault::default();

        match mint_address(&engine, &vault, &unique("proj"), 5, "bot", Some(&domain), None) {
            Err(AddrError::Exists(hint)) => {
                assert!(hint.contains(&format!("bot2@{domain}")), "{hint}")
            }
            other => panic!("race maps to Exists, got {other:?}"),
        }
        // COMPENSATED: the account Stalwart minted is destroyed…
        assert_eq!(engine.destroyed.lock().unwrap().as_slice(), ["acc-1"]);
        // …the vault entry is gone…
        assert_eq!(vault.deleted.lock().unwrap().len(), 1);
        // …and the only row for the address is the racer's.
        let row = address_row(&racer_address).expect("racer row");
        assert_eq!(row.id, racer_id);
        cleanup_domain(&domain);
    }

    /// A vault failure AFTER the Stalwart create also compensates.
    #[test]
    fn vault_failure_destroys_the_stalwart_account() {
        let domain = unique("vaultfail") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault { fail_store: true, ..FakeVault::default() };
        match mint_address(&engine, &vault, &unique("proj"), 5, "bot", Some(&domain), None) {
            Err(AddrError::Engine(hint)) => assert!(hint.contains("vault"), "{hint}"),
            other => panic!("must be Engine, got {other:?}"),
        }
        assert_eq!(engine.destroyed.lock().unwrap().as_slice(), ["acc-1"]);
        assert!(address_row(&format!("bot@{domain}")).is_none(), "no row");
        cleanup_domain(&domain);
    }

    /// A verified domain row missing its Stalwart id fails loudly
    /// BEFORE any engine call (never a half-mint).
    #[test]
    fn domain_without_stalwart_id_is_an_engine_error() {
        let domain = unique("noid") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", None);
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        match mint_address(&engine, &vault, &unique("proj"), 5, "bot", Some(&domain), None) {
            Err(AddrError::Engine(hint)) => assert!(hint.contains("re-add"), "{hint}"),
            other => panic!("must be Engine, got {other:?}"),
        }
        assert!(engine.created.lock().unwrap().is_empty(), "engine untouched");
        cleanup_domain(&domain);
    }

    // ── Retire ──

    #[test]
    fn retire_disables_account_sets_retired_at_and_guards_repeats_and_foreign() {
        let domain = unique("retire") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");
        mint_address(&engine, &vault, &project, 5, "bot", Some(&domain), None).expect("mint");
        let address = format!("bot@{domain}");

        // Foreign workspace → the MASKED not_found (no existence leak).
        match retire_address(Some(&engine), "someone-else", &address) {
            Err(AddrError::NotFound(hint)) => {
                assert!(hint.contains("in this workspace"), "{hint}")
            }
            other => panic!("must be NotFound, got {other:?}"),
        }
        // Unknown address → the same shape.
        assert!(matches!(
            retire_address(Some(&engine), &project, &format!("ghost@{domain}")),
            Err(AddrError::NotFound(_))
        ));

        // Own address (un-normalized spelling still hits): disabled —
        // never destroyed — and the row flips retired with a timestamp.
        let v = retire_address(Some(&engine), &project, &format!("  BOT@{} ", domain.to_uppercase()))
            .expect("retire ok");
        assert_eq!(v["ok"], true);
        assert_eq!(v["status"], "retired");
        assert!(v["retiredAt"].as_i64().unwrap() > 0);
        assert_eq!(engine.disabled.lock().unwrap().as_slice(), ["acc-1"]);
        assert!(engine.destroyed.lock().unwrap().is_empty(), "retire NEVER destroys (§12)");
        let row = address_row(&address).expect("row kept");
        assert_eq!(row.status, "retired");
        assert!(row.retired_at.is_some());

        // Again → already-retired, clean and structured.
        match retire_address(Some(&engine), &project, &address) {
            Err(AddrError::Exists(hint)) => assert!(hint.contains("already retired"), "{hint}"),
            other => panic!("must be Exists, got {other:?}"),
        }
        cleanup_domain(&domain);
    }

    #[test]
    fn retire_fails_closed_without_an_engine_and_on_engine_refusal() {
        let domain = unique("retfc") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");
        mint_address(&engine, &vault, &project, 5, "bot", Some(&domain), None).expect("mint");
        let address = format!("bot@{domain}");

        // Stalwart id present + NO engine → fail closed, row untouched.
        match retire_address(None, &project, &address) {
            Err(AddrError::Engine(hint)) => assert!(hint.contains("Settings → Email"), "{hint}"),
            other => panic!("must fail closed, got {other:?}"),
        }
        assert_eq!(address_row(&address).unwrap().status, "active");

        // Engine refuses the disable → fail closed too.
        let refusing = FakeAddrEngine { fail_disable: true, ..FakeAddrEngine::ok() };
        assert!(matches!(
            retire_address(Some(&refusing), &project, &address),
            Err(AddrError::Engine(_))
        ));
        assert_eq!(address_row(&address).unwrap().status, "active");
        cleanup_domain(&domain);
    }

    fn seed_active_address(
        address: &str,
        domain_id: &str,
        project_id: &str,
        stalwart_account_id: Option<&str>,
        status: &str,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 100)",
            rusqlite::params![id, address, domain_id, stalwart_account_id, project_id, status],
        )
        .expect("seed address");
        id
    }

    #[test]
    fn rotate_pending_active_row_sets_password_once_via_account_id() {
        let domain = unique("rot-pending") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain, "pending", Some("stw-pending"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");
        let address = format!("precut@{domain}");
        let row_id = seed_active_address(&address, &domain_id, &project, Some("acc-rot"), "active");

        let v = rotate_address_password(&engine, &vault, &format!("  PRECUT@{domain} "))
            .expect("rotate pending active");
        assert_eq!(v["ok"], true);
        assert_eq!(v["address"], address);
        assert_eq!(v["username"], address);
        let password = v["password"].as_str().expect("once password");
        assert_eq!(password.len(), 64);
        assert_eq!(v["imap"]["port"], 993);
        assert_eq!(v["imap"]["tls"], true);
        assert_eq!(v["imapStartTls"]["port"], 143);
        assert_eq!(v["submission"]["port"], 465);
        assert_eq!(v["jmap"]["port"], 443);
        let note = v["note"].as_str().unwrap_or("");
        assert!(
            note.contains("IMAP/SMTP sessions on the mailbox password die"),
            "{note}"
        );
        assert!(note.contains("app passwords do not"), "{note}");

        assert_eq!(
            engine.passwords_set.lock().unwrap().as_slice(),
            [("acc-rot".to_string(), password.to_string())]
        );
        let stored = vault.stored.lock().unwrap();
        assert_eq!(stored.len(), 1, "store_exact once, never store() orphan ref");
        assert_eq!(stored[0].0, format!("account-{row_id}"));
        assert_eq!(stored[0].1, password);
        cleanup_domain(&domain);
    }

    #[test]
    fn rotate_vault_failure_still_returns_password() {
        let domain = unique("rot-vault") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain, "pending", Some("stw-p"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault {
            fail_store: true,
            ..FakeVault::default()
        };
        let address = format!("box@{domain}");
        seed_active_address(&address, &domain_id, &unique("proj"), Some("acc-v"), "active");

        let v = rotate_address_password(&engine, &vault, &address).expect("engine won");
        assert_eq!(v["ok"], true);
        assert_eq!(v["password"].as_str().unwrap().len(), 64);
        assert_eq!(engine.passwords_set.lock().unwrap().len(), 1);
        assert!(vault.stored.lock().unwrap().is_empty());
        cleanup_domain(&domain);
    }

    #[test]
    fn rotate_skips_retired_and_does_not_use_rotate_account_secret_shape() {
        let domain = unique("rot-miss") + ".example";
        cleanup_domain(&domain);
        let domain_id = seed_domain(&domain, "pending", Some("stw-p"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let project = unique("proj");
        seed_active_address(
            &format!("gone@{domain}"),
            &domain_id,
            &project,
            Some("acc-gone"),
            "retired",
        );
        match rotate_address_password(&engine, &vault, &format!("gone@{domain}")) {
            Err(AddrError::NotFound(hint)) => assert!(hint.contains("hosted address"), "{hint}"),
            other => panic!("retired must be not_found, got {other:?}"),
        }
        match rotate_address_password(&engine, &vault, &format!("ghost@{domain}")) {
            Err(AddrError::NotFound(_)) => {}
            other => panic!("unknown must be not_found, got {other:?}"),
        }
        assert!(engine.passwords_set.lock().unwrap().is_empty());
        let refusing = FakeAddrEngine {
            fail_set_password: true,
            ..FakeAddrEngine::ok()
        };
        seed_active_address(
            &format!("live@{domain}"),
            &domain_id,
            &project,
            Some("acc-live"),
            "active",
        );
        assert!(matches!(
            rotate_address_password(&refusing, &vault, &format!("live@{domain}")),
            Err(AddrError::Engine(_))
        ));
        assert!(vault.stored.lock().unwrap().is_empty(), "no vault on engine fail");
        cleanup_domain(&domain);
    }

    // ── Lists ──

    #[test]
    fn lists_scope_by_owner_and_the_owner_table_includes_retired() {
        let domain = unique("list") + ".example";
        cleanup_domain(&domain);
        seed_domain(&domain, "verified", Some("stw-1"));
        let engine = FakeAddrEngine::ok();
        let vault = FakeVault::default();
        let mine = unique("proj");
        let theirs = unique("proj");
        mint_address(&engine, &vault, &mine, 5, "alpha", Some(&domain), None).expect("mint");
        mint_address(&engine, &vault, &mine, 5, "beta", Some(&domain), None).expect("mint");
        mint_address(&engine, &vault, &theirs, 5, "gamma", Some(&domain), None).expect("mint");
        retire_address(Some(&engine), &mine, &format!("beta@{domain}")).expect("retire");

        // Caller view: ACTIVE only, own only, with cap usage.
        let v = list_for_project_json(&mine, 5);
        assert_eq!(v["ok"], true);
        let addrs: Vec<&str> = v["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["address"].as_str().unwrap())
            .collect();
        assert_eq!(addrs, [format!("alpha@{domain}")], "active + own only");
        assert!(v["addresses"][0]["createdAt"].as_i64().unwrap() > 0);
        assert_eq!(v["cap"]["used"], 1);
        assert_eq!(v["cap"]["cap"], 5);

        // Owner table: every row incl. retired, with holder + status.
        let v = list_all_json();
        let all = v["addresses"].as_array().unwrap();
        let beta = all
            .iter()
            .find(|a| a["address"] == format!("beta@{domain}"))
            .expect("retired row listed");
        assert_eq!(beta["status"], "retired");
        assert!(beta["retiredAt"].as_i64().unwrap() > 0);
        assert_eq!(beta["holderProjectId"], mine);
        let gamma = all
            .iter()
            .find(|a| a["address"] == format!("gamma@{domain}"))
            .expect("other workspace listed");
        assert_eq!(gamma["holderProjectId"], theirs);
        cleanup_domain(&domain);
    }
}
