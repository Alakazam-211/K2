//! S11 — the ONE inbox access layer over BOTH provisioning sources
//! (PRD §17.5). Hosted addresses (`mail_addresses`, minted on a
//! verified domain) and linked inboxes (`mail_external_inboxes`, the
//! user's OWN IMAP account) are governed by a SINGLE model:
//!
//! - Each provisioning row's `owner_project_id` is the **PRIMARY**
//!   workspace (it manages the inbox); `primary_level` is the primary's
//!   own ceiling (hosted default 'send', linked default 'draft').
//! - `mail_inbox_grants(source, inbox_id, project_id, level)` gives ANY
//!   other workspace access. Levels order **read < draft < send**.
//! - `effective_level(project, source, inbox_id)` = the primary's
//!   `primary_level` when `project` IS the primary, else the grant row
//!   level, else `None` (no access).
//! - READ needs ≥ read; DRAFT needs ≥ draft; **SEND needs level=='send'
//!   for EITHER source** (§17.5 linked-send opt-in): hosted 'send' goes
//!   out through Stalwart submission under the `mail_agent_send`
//!   off/approval/on governance; linked 'send' goes out through SMTP
//!   submission from the user's own account and is UNGATED for now. A
//!   linked primary still *defaults* to 'draft' (send is opt-in — the
//!   owner raises the level), but its ceiling is 'send'.
//!
//! MASKING (the S3 rule, preserved everywhere): a workspace with no
//! access to an address gets the SAME `not_found` a foreign/unknown
//! address gets — it never learns which inboxes exist outside it. The
//! agent-facing gates ([`can_read`]/[`can_draft`]/[`can_send`]) return
//! that masked shape in every deny branch. The owner/primary MANAGEMENT
//! surface ([`grant_access`]/[`revoke_access`]/[`set_primary`]/
//! [`set_level`]) is NOT masked (the primary sees its own inboxes).
//!
//! Send governance stays ORTHOGONAL: `can_send` only answers "may this
//! workspace send FROM this address" (either source). For HOSTED, the
//! actual send still passes the `mail_agent_send` off/approval/on gate
//! (S5); for LINKED, send is UNGATED for now (§17.5 — unified gating for
//! linked lands with the wider email layer).

use crate::mail::addresses::{self, AddrError};
use crate::mail::external;
use crate::mail::messages::ReadError;
use k2_core::db::schema::MailExternalInbox;

// ── Levels ──────────────────────────────────────────────────────────────

pub const LEVEL_READ: &str = "read";
pub const LEVEL_DRAFT: &str = "draft";
pub const LEVEL_SEND: &str = "send";

/// read < draft < send (the derived `Ord` follows declaration order).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Read,
    Draft,
    Send,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Read => LEVEL_READ,
            Level::Draft => LEVEL_DRAFT,
            Level::Send => LEVEL_SEND,
        }
    }
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim() {
            LEVEL_READ => Some(Level::Read),
            LEVEL_DRAFT => Some(Level::Draft),
            LEVEL_SEND => Some(Level::Send),
            _ => None,
        }
    }
    /// Clamp to a source's ceiling. Both sources now ceiling at `send`,
    /// so this is a no-op today — kept as the single defensive choke
    /// point should a source ceiling ever drop below `send` again.
    pub fn clamp_to(self, source: Source) -> Level {
        self.min(source.max_level())
    }
}

// ── Sources ─────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Source {
    Hosted,
    Linked,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Hosted => "hosted",
            Source::Linked => "linked",
        }
    }
    /// The highest level this source can ever reach. Both sources now
    /// reach `send`: hosted via Stalwart submission, LINKED via SMTP
    /// submission from the user's own account (§17.5 linked-send opt-in
    /// — off by default: a linked primary still *defaults* to 'draft',
    /// but its ceiling is 'send' so the owner can raise it).
    pub fn max_level(self) -> Level {
        match self {
            Source::Hosted => Level::Send,
            Source::Linked => Level::Send,
        }
    }
}

// ── Management errors (owner/primary surface — NOT masked) ───────────────

#[derive(Debug)]
pub enum AccessError {
    /// Bad input → 400 `usage`.
    Usage(String),
    /// Unknown inbox (owner surface — the primary sees its own) → 404.
    NotFound(String),
    /// Stalwart/DB said no → 502 `engine`.
    Engine(String),
}

// ── The resolved-inbox handle the read/send paths need ──────────────────

/// What an agent-facing gate resolves to: enough to build a backend and
/// re-check server-side. `account_id` is the backend mailbox handle
/// (hosted → the Stalwart account id; linked → the external row id).
#[derive(Debug, Clone)]
pub struct AccessInbox {
    pub source: Source,
    /// Normalized address (the seam key).
    pub address: String,
    /// The backend account handle; `None` for a hosted row with no
    /// Stalwart account yet (a degraded mint — callers fail loudly).
    pub account_id: Option<String>,
    /// The linked row (present only for [`Source::Linked`]) — the draft
    /// path needs it to build the IMAP backend.
    pub linked: Option<MailExternalInbox>,
    /// The caller's effective level on this inbox (read by tests and
    /// available to callers that gate on the exact level).
    #[allow(dead_code)]
    pub your_level: Level,
}

// ── Row lookups ─────────────────────────────────────────────────────────

/// A hosted ACTIVE address row (the fields the access layer needs).
struct HostedRow {
    id: String,
    stalwart_account_id: Option<String>,
    owner_project_id: String,
    primary_level: String,
    /// The PRIMARY's own management/delete caps (0081) — read only for
    /// the manage/delete gates; the read/draft/send path ignores them.
    primary_can_manage: bool,
    primary_can_delete: bool,
}

fn hosted_active_row(conn: &rusqlite::Connection, address: &str) -> Option<HostedRow> {
    conn.query_row(
        "SELECT id, stalwart_account_id, owner_project_id, primary_level, \
                primary_can_manage, primary_can_delete \
         FROM mail_addresses WHERE address = ?1 AND status = 'active'",
        rusqlite::params![address],
        |r| {
            Ok(HostedRow {
                id: r.get(0)?,
                stalwart_account_id: r.get(1)?,
                owner_project_id: r.get(2)?,
                primary_level: r.get(3)?,
                primary_can_manage: r.get::<_, i64>(4)? != 0,
                primary_can_delete: r.get::<_, i64>(5)? != 0,
            })
        },
    )
    .ok()
}

/// The grant level a workspace holds on `(source, inbox_id)`, or `None`
/// (a locking convenience over [`grant_level_conn`] — test-only for now;
/// production reads go through the resolve/effective paths).
#[cfg(test)]
pub fn grant_level(source: Source, inbox_id: &str, project_id: &str) -> Option<Level> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    grant_level_conn(&conn, source, inbox_id, project_id)
}

fn grant_level_conn(
    conn: &rusqlite::Connection,
    source: Source,
    inbox_id: &str,
    project_id: &str,
) -> Option<Level> {
    conn.query_row(
        "SELECT level FROM mail_inbox_grants \
         WHERE source = ?1 AND inbox_id = ?2 AND project_id = ?3",
        rusqlite::params![source.as_str(), inbox_id, project_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| Level::parse(&s))
}

/// The effective level a workspace has on an inbox: the primary's
/// `primary_level` when it IS the primary, else its grant row, else
/// `None`. `primary_level` is clamped to the source ceiling (defensive
/// — a no-op today since both ceilings are 'send').
fn effective_level_conn(
    conn: &rusqlite::Connection,
    project_id: &str,
    source: Source,
    inbox_id: &str,
    owner_project_id: &str,
    primary_level: &str,
) -> Option<Level> {
    if project_id == owner_project_id {
        let lvl = Level::parse(primary_level).unwrap_or(Level::Read).clamp_to(source);
        return Some(lvl);
    }
    grant_level_conn(conn, source, inbox_id, project_id).map(|l| l.clamp_to(source))
}

// ── Management/delete caps (0081 — ORTHOGONAL to level) ─────────────────

/// The `(can_manage, can_delete)` a grant row holds on `(source,
/// inbox_id, project_id)`, or `None` when there is no grant row.
fn grant_caps_conn(
    conn: &rusqlite::Connection,
    source: Source,
    inbox_id: &str,
    project_id: &str,
) -> Option<(bool, bool)> {
    conn.query_row(
        "SELECT can_manage, can_delete FROM mail_inbox_grants \
         WHERE source = ?1 AND inbox_id = ?2 AND project_id = ?3",
        rusqlite::params![source.as_str(), inbox_id, project_id],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
    )
    .ok()
}

/// The effective `(can_manage, can_delete)` a workspace has on an inbox:
/// the primary's own caps when it IS the primary, else its grant row's
/// caps, else `None` (no access at all — masked). INDEPENDENT of level.
fn manage_caps_conn(
    conn: &rusqlite::Connection,
    project_id: &str,
    source: Source,
    inbox_id: &str,
    owner_project_id: &str,
    primary_can_manage: bool,
    primary_can_delete: bool,
) -> Option<(bool, bool)> {
    if project_id == owner_project_id {
        return Some((primary_can_manage, primary_can_delete));
    }
    grant_caps_conn(conn, source, inbox_id, project_id)
}

/// The primary's `(can_manage, can_delete)` on a LINKED inbox (a locking
/// convenience over the raw column read, mirroring
/// [`linked_primary_level_conn`]).
fn linked_primary_caps_conn(conn: &rusqlite::Connection, inbox_id: &str) -> (bool, bool) {
    conn.query_row(
        "SELECT primary_can_manage, primary_can_delete FROM mail_external_inboxes WHERE id = ?1",
        rusqlite::params![inbox_id],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
    )
    .unwrap_or((false, false))
}

// ── Agent-facing gates (MASKED) ─────────────────────────────────────────

fn normalize_or_usage(raw_address: &str) -> Result<String, ReadError> {
    addresses::normalize_address(raw_address).map_err(|e| match e {
        AddrError::Usage(hint) => ReadError::Usage(hint),
        _ => ReadError::Usage(format!("'{raw_address}' is not a valid address")),
    })
}

/// Resolve an explicitly named address to an [`AccessInbox`], enforcing
/// `need` (and, for send, source=='hosted' && level=='send'). Every deny
/// branch answers the SAME masked `not_found` (no existence leak).
fn resolve(project_id: &str, raw_address: &str, need: Level) -> Result<AccessInbox, ReadError> {
    let address = normalize_or_usage(raw_address)?;
    let masked = || ReadError::NotFound(format!("no address '{address}' in this workspace"));

    // Linked first (its row is the §17.5 seam key); then hosted active.
    if let Some(row) = external::inbox_for_address(&address) {
        let source = Source::Linked;
        let db = k2_core::db::shared();
        let conn = db.lock();
        let primary = linked_primary_level_conn(&conn, &row.id);
        let eff = effective_level_conn(
            &conn,
            project_id,
            source,
            &row.id,
            &row.owner_project_id,
            &primary,
        )
        .ok_or_else(masked)?;
        drop(conn);
        // SEND is now allowed on linked (§17.5 opt-in) — the gate is the
        // effective level alone, same as hosted. `account_id` is the
        // external row id (the SMTP path resolves the vault key + route
        // from the linked row it also carries).
        if eff < need {
            return Err(masked());
        }
        return Ok(AccessInbox {
            source,
            address,
            account_id: Some(row.id.clone()),
            linked: Some(row),
            your_level: eff,
        });
    }

    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(row) = hosted_active_row(&conn, &address) else {
        return Err(masked());
    };
    let source = Source::Hosted;
    let eff = effective_level_conn(
        &conn,
        project_id,
        source,
        &row.id,
        &row.owner_project_id,
        &row.primary_level,
    )
    .ok_or_else(masked)?;
    drop(conn);
    if eff < need {
        return Err(masked());
    }
    if need == Level::Send && eff != Level::Send {
        return Err(masked());
    }
    Ok(AccessInbox {
        source,
        address,
        account_id: row.stalwart_account_id,
        linked: None,
        your_level: eff,
    })
}

/// The MASKED READ gate (messages/read/wait/attachments across BOTH
/// sources): the caller may read when its effective level ≥ read.
pub fn can_read(project_id: &str, raw_address: &str) -> Result<AccessInbox, ReadError> {
    resolve(project_id, raw_address, Level::Read)
}

/// The MASKED DRAFT gate: effective level ≥ draft. Used by the linked
/// `k2 mail draft` verb (append `\Draft` into the account's Drafts
/// folder); a read-only grant, or no access, answers the same masked
/// `not_found`.
pub fn can_draft(project_id: &str, raw_address: &str) -> Result<AccessInbox, ReadError> {
    resolve(project_id, raw_address, Level::Draft)
}

/// The MASKED SEND gate: effective level == send, for EITHER source
/// (§17.5). A draft/read grant, or no access, answers the same masked
/// `not_found`. Send GOVERNANCE differs by source and is layered on by
/// the caller: HOSTED passes the off/approval/on gate; LINKED is ungated
/// (SMTP submission). This only answers "may this workspace send FROM
/// this address" — the caller branches on `AccessInbox.source`.
pub fn can_send(project_id: &str, raw_address: &str) -> Result<AccessInbox, ReadError> {
    resolve(project_id, raw_address, Level::Send)
}

// ── Management/delete gates (0081 — ORTHOGONAL to the read/draft/send
//    level; MASKED, same shape as the read gate) ────────────────────────

/// Resolve `raw_address` to its inbox AND the caller's
/// `(can_manage, can_delete)` caps. A workspace with NO access at all
/// (not primary, no grant row) answers the SAME masked `not_found` the
/// read gate uses (no existence leak). `your_level` is the caller's
/// effective level (caps are independent of it — a read-only workspace
/// may still be granted manage).
fn resolve_caps(
    project_id: &str,
    raw_address: &str,
) -> Result<(AccessInbox, bool, bool), ReadError> {
    let address = normalize_or_usage(raw_address)?;
    let masked = || ReadError::NotFound(format!("no address '{address}' in this workspace"));
    // Linked first (its row is the §17.5 seam key); then hosted active.
    if let Some(row) = external::inbox_for_address(&address) {
        let source = Source::Linked;
        let db = k2_core::db::shared();
        let conn = db.lock();
        let (pcm, pcd) = linked_primary_caps_conn(&conn, &row.id);
        let (cm, cd) =
            manage_caps_conn(&conn, project_id, source, &row.id, &row.owner_project_id, pcm, pcd)
                .ok_or_else(masked)?;
        let primary = linked_primary_level_conn(&conn, &row.id);
        let eff = effective_level_conn(
            &conn,
            project_id,
            source,
            &row.id,
            &row.owner_project_id,
            &primary,
        )
        .unwrap_or(Level::Read);
        drop(conn);
        let inbox = AccessInbox {
            source,
            address,
            account_id: Some(row.id.clone()),
            linked: Some(row),
            your_level: eff,
        };
        return Ok((inbox, cm, cd));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(row) = hosted_active_row(&conn, &address) else {
        return Err(masked());
    };
    let source = Source::Hosted;
    let (cm, cd) = manage_caps_conn(
        &conn,
        project_id,
        source,
        &row.id,
        &row.owner_project_id,
        row.primary_can_manage,
        row.primary_can_delete,
    )
    .ok_or_else(masked)?;
    let eff = effective_level_conn(
        &conn,
        project_id,
        source,
        &row.id,
        &row.owner_project_id,
        &row.primary_level,
    )
    .unwrap_or(Level::Read);
    drop(conn);
    let inbox = AccessInbox {
        source,
        address,
        account_id: row.stalwart_account_id,
        linked: None,
        your_level: eff,
    };
    Ok((inbox, cm, cd))
}

/// The MASKED MANAGEMENT gate (move / flag / archive / folder ops): the
/// caller may manage when it holds `can_manage` (primary's own col, or a
/// grant's col). Every deny — no access, OR access without manage —
/// answers the same masked `not_found` (no existence leak).
pub fn can_manage(project_id: &str, raw_address: &str) -> Result<AccessInbox, ReadError> {
    let (inbox, cm, _cd) = resolve_caps(project_id, raw_address)?;
    if !cm {
        return Err(ReadError::NotFound(format!(
            "no address '{}' in this workspace",
            inbox.address
        )));
    }
    Ok(inbox)
}

/// The MASKED DELETE gate (delete = MOVE to Trash, never EXPUNGE):
/// requires `can_delete` (which the set path guarantees implies
/// `can_manage`). A workspace with NO manage access is masked
/// (`not_found`, no leak); one that CAN manage but lacks delete gets a
/// TEACHING usage error (it already sees the inbox, so nothing leaks).
pub fn can_delete(project_id: &str, raw_address: &str) -> Result<AccessInbox, ReadError> {
    let (inbox, cm, cd) = resolve_caps(project_id, raw_address)?;
    if !cm {
        return Err(ReadError::NotFound(format!(
            "no address '{}' in this workspace",
            inbox.address
        )));
    }
    if !cd {
        return Err(ReadError::Usage(format!(
            "deleting from '{}' needs the delete capability — ask your human to enable it \
             ('k2 mail access manage {} <workspace> --delete')",
            inbox.address, inbox.address
        )));
    }
    Ok(inbox)
}

/// Every hosted ACTIVE address a workspace can READ (primary or grant) —
/// the no-address `messages`/`wait` sweep's hosted half. Returns
/// `(address, stalwart_account_id)` pairs (rows without a Stalwart
/// account are skipped, mirroring the pre-S11 behavior).
pub fn readable_hosted(project_id: &str) -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT a.address, a.stalwart_account_id \
         FROM mail_addresses a \
         WHERE a.status = 'active' AND a.stalwart_account_id IS NOT NULL AND ( \
             a.owner_project_id = ?1 \
             OR EXISTS (SELECT 1 FROM mail_inbox_grants g \
                        WHERE g.source = 'hosted' AND g.inbox_id = a.id \
                          AND g.project_id = ?1) ) \
         ORDER BY a.created_at, a.address",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Every LINKED inbox a workspace can READ (primary or grant) — the
/// no-address sweep's linked half. Returns `(address, inbox_id)` pairs.
pub fn readable_linked(project_id: &str) -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT i.email_address, i.id \
         FROM mail_external_inboxes i \
         WHERE i.owner_project_id = ?1 \
            OR EXISTS (SELECT 1 FROM mail_inbox_grants g \
                       WHERE g.source = 'linked' AND g.inbox_id = i.id \
                         AND g.project_id = ?1) \
         ORDER BY i.created_at, i.email_address",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Every hosted ACTIVE address a workspace can SEND FROM (effective
/// level == send — primary at 'send', or a 'send' grant). Returns
/// `(address, stalwart_account_id)`. Linked inboxes are NOT swept here —
/// linked send requires an explicit `--from <linked-address>` (this is
/// the implicit-`from` resolver for `k2 mail send`, and it stays
/// hosted-only so the ambiguity story is unchanged).
pub fn sendable_hosted(project_id: &str) -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT a.address, a.stalwart_account_id \
         FROM mail_addresses a \
         WHERE a.status = 'active' AND a.stalwart_account_id IS NOT NULL AND ( \
             (a.owner_project_id = ?1 AND a.primary_level = 'send') \
             OR EXISTS (SELECT 1 FROM mail_inbox_grants g \
                        WHERE g.source = 'hosted' AND g.inbox_id = a.id \
                          AND g.project_id = ?1 AND g.level = 'send') ) \
         ORDER BY a.created_at, a.address",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

// ── Linked helpers ──────────────────────────────────────────────────────

fn linked_primary_level_conn(conn: &rusqlite::Connection, inbox_id: &str) -> String {
    conn.query_row(
        "SELECT primary_level FROM mail_external_inboxes WHERE id = ?1",
        rusqlite::params![inbox_id],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_else(|_| LEVEL_DRAFT.to_string())
}

// ── Owner/primary MANAGEMENT surface (NOT masked) ───────────────────────

/// A resolved management target: which provisioning row an address names.
struct ManageTarget {
    source: Source,
    inbox_id: String,
    address: String,
    owner_project_id: String,
    primary_level: String,
}

fn manage_error_usage(raw_address: &str) -> impl Fn(AddrError) -> AccessError + '_ {
    move |e| match e {
        AddrError::Usage(hint) => AccessError::Usage(hint),
        _ => AccessError::Usage(format!("'{raw_address}' is not a valid address")),
    }
}

/// Resolve an address to its provisioning row for MANAGEMENT (unmasked
/// — this is the primary's own surface). Linked wins the seam key; then
/// hosted (any status, so a retired hosted address still resolves for
/// e.g. revoke cleanup).
fn manage_target(raw_address: &str) -> Result<ManageTarget, AccessError> {
    let address =
        addresses::normalize_address(raw_address).map_err(manage_error_usage(raw_address))?;
    if let Some(row) = external::inbox_for_address(&address) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let primary = linked_primary_level_conn(&conn, &row.id);
        return Ok(ManageTarget {
            source: Source::Linked,
            inbox_id: row.id,
            address: row.email_address,
            owner_project_id: row.owner_project_id,
            primary_level: primary,
        });
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = conn
        .query_row(
            "SELECT id, address, owner_project_id, primary_level FROM mail_addresses \
             WHERE address = ?1",
            rusqlite::params![address],
            |r| {
                Ok(ManageTarget {
                    source: Source::Hosted,
                    inbox_id: r.get(0)?,
                    address: r.get(1)?,
                    owner_project_id: r.get(2)?,
                    primary_level: r.get(3)?,
                })
            },
        )
        .ok();
    row.ok_or_else(|| {
        AccessError::NotFound(format!(
            "no inbox '{address}' — 'k2 mail inboxes' shows what you manage"
        ))
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Validate a level word against a source: any of read/draft/send.
/// 'send' is now valid on BOTH sources (hosted → Stalwart submission;
/// linked → SMTP submission, §17.5 opt-in). `source` is retained for a
/// future per-source ceiling but is not currently a constraint.
fn validate_level_for(_source: Source, level: &str) -> Result<Level, AccessError> {
    Level::parse(level).ok_or_else(|| {
        AccessError::Usage(format!(
            "invalid level '{}' — 'read' (messages/read/wait), 'draft' (read + save reply \
             drafts), or 'send' (read + send/reply)",
            level.trim()
        ))
    })
}

/// GRANT `level` to a workspace on any inbox (Primary-gated at the
/// route). Upsert. Granting the PRIMARY workspace is a teaching error
/// (its access is the ownership binding + primary_level, not a grant).
/// Returns the resolved address + source for the route echo.
pub fn grant_access(
    raw_address: &str,
    grantee_project_id: &str,
    level: &str,
) -> Result<(String, Source), AccessError> {
    let target = manage_target(raw_address)?;
    let lvl = validate_level_for(target.source, level)?;
    if target.owner_project_id == grantee_project_id {
        return Err(AccessError::Usage(
            "that workspace is the PRIMARY — it already manages this inbox at its \
             primary level. Use 'set-level' to change the primary's own ceiling."
                .to_string(),
        ));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_inbox_grants (source, inbox_id, project_id, level, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(source, inbox_id, project_id) DO UPDATE SET level = excluded.level",
            rusqlite::params![
                target.source.as_str(),
                target.inbox_id,
                grantee_project_id,
                lvl.as_str(),
                now_secs()
            ],
        )
        .map_err(|e| AccessError::Engine(format!("grant access: {e}")))?;
    }
    k2_core::log_debug!(
        "[mail/access] granted '{}' on {} ({}) to workspace {}",
        lvl.as_str(),
        target.address,
        target.source.as_str(),
        grantee_project_id
    );
    Ok((target.address, target.source))
}

/// REVOKE a workspace's grant on an inbox (Primary-gated). Idempotent
/// (no grant → no-op ok). Revoking the PRIMARY teaches (transfer with
/// `set-primary` or tear the inbox down).
pub fn revoke_access(
    raw_address: &str,
    grantee_project_id: &str,
) -> Result<String, AccessError> {
    let target = manage_target(raw_address)?;
    if target.owner_project_id == grantee_project_id {
        return Err(AccessError::Usage(format!(
            "that workspace is the PRIMARY of '{}' — you can't revoke the manager. Transfer \
             with 'k2 mail access primary', or remove a linked inbox with 'k2 mail link remove'.",
            target.address
        )));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "DELETE FROM mail_inbox_grants WHERE source = ?1 AND inbox_id = ?2 AND project_id = ?3",
            rusqlite::params![target.source.as_str(), target.inbox_id, grantee_project_id],
        )
        .map_err(|e| AccessError::Engine(format!("revoke access: {e}")))?;
    }
    Ok(target.address)
}

/// TRANSFER the primary (managing) workspace. The OLD primary becomes a
/// grant at its prior `primary_level` (clamped to the source ceiling);
/// the NEW primary's grant row (if any) is removed and its
/// `primary_level` is set to its prior grant level (else the source
/// ceiling), clamped to the source. Primary→same-primary teaches.
pub fn set_primary(
    raw_address: &str,
    new_project_id: &str,
) -> Result<(String, Source), AccessError> {
    let target = manage_target(raw_address)?;
    if target.owner_project_id == new_project_id {
        return Err(AccessError::Usage(format!(
            "that workspace is already the primary of '{}'",
            target.address
        )));
    }
    let old_primary_level = Level::parse(&target.primary_level)
        .unwrap_or(Level::Read)
        .clamp_to(target.source);
    let db = k2_core::db::shared();
    let conn = db.lock();
    // The new primary inherits its prior grant level (if any), else the
    // source ceiling — it is now the manager.
    let new_primary_level = grant_level_conn(&conn, target.source, &target.inbox_id, new_project_id)
        .unwrap_or_else(|| target.source.max_level())
        .clamp_to(target.source);
    let table = match target.source {
        Source::Hosted => "mail_addresses",
        Source::Linked => "mail_external_inboxes",
    };
    conn.execute(
        &format!("UPDATE {table} SET owner_project_id = ?1, primary_level = ?2 WHERE id = ?3"),
        rusqlite::params![new_project_id, new_primary_level.as_str(), target.inbox_id],
    )
    .map_err(|e| AccessError::Engine(format!("set primary: {e}")))?;
    // The new primary is no longer a grantee.
    conn.execute(
        "DELETE FROM mail_inbox_grants WHERE source = ?1 AND inbox_id = ?2 AND project_id = ?3",
        rusqlite::params![target.source.as_str(), target.inbox_id, new_project_id],
    )
    .map_err(|e| AccessError::Engine(format!("set primary (clear grant): {e}")))?;
    // The old primary keeps access as a grant at its prior ceiling.
    conn.execute(
        "INSERT INTO mail_inbox_grants (source, inbox_id, project_id, level, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(source, inbox_id, project_id) DO UPDATE SET level = excluded.level",
        rusqlite::params![
            target.source.as_str(),
            target.inbox_id,
            target.owner_project_id,
            old_primary_level.as_str(),
            now_secs()
        ],
    )
    .map_err(|e| AccessError::Engine(format!("set primary (demote old): {e}")))?;
    Ok((target.address, target.source))
}

/// SET a workspace's level. When `project` IS the primary, updates the
/// primary_level (validated vs source); otherwise upserts its grant.
pub fn set_level(
    raw_address: &str,
    project_id: &str,
    level: &str,
) -> Result<(String, Source, bool), AccessError> {
    let target = manage_target(raw_address)?;
    let lvl = validate_level_for(target.source, level)?;
    let is_primary = target.owner_project_id == project_id;
    let db = k2_core::db::shared();
    let conn = db.lock();
    if is_primary {
        let table = match target.source {
            Source::Hosted => "mail_addresses",
            Source::Linked => "mail_external_inboxes",
        };
        conn.execute(
            &format!("UPDATE {table} SET primary_level = ?1 WHERE id = ?2"),
            rusqlite::params![lvl.as_str(), target.inbox_id],
        )
        .map_err(|e| AccessError::Engine(format!("set primary level: {e}")))?;
    } else {
        conn.execute(
            "INSERT INTO mail_inbox_grants (source, inbox_id, project_id, level, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(source, inbox_id, project_id) DO UPDATE SET level = excluded.level",
            rusqlite::params![
                target.source.as_str(),
                target.inbox_id,
                project_id,
                lvl.as_str(),
                now_secs()
            ],
        )
        .map_err(|e| AccessError::Engine(format!("set grant level: {e}")))?;
    }
    Ok((target.address, target.source, is_primary))
}

/// SET a workspace's management/delete caps (0081; Primary-gated at the
/// route). `can_delete` REQUIRES `can_manage` — passing delete=true with
/// manage=false is a teaching usage error; clearing manage clears delete
/// (belt: normalized here even if the caller slips). When `project` IS
/// the primary, updates the primary's own cols; otherwise upserts its
/// grant row (a new grant defaults to level 'read' so a manage-only
/// workspace can still see the inbox — caps are orthogonal to, but never
/// below, read visibility). Returns `(address, source, is_primary)`.
pub fn set_manage(
    raw_address: &str,
    project_id: &str,
    can_manage: bool,
    can_delete: bool,
) -> Result<(String, Source, bool), AccessError> {
    if can_delete && !can_manage {
        return Err(AccessError::Usage(
            "delete requires manage — enable delete only with manage on (a workspace can't \
             delete what it can't manage)"
                .to_string(),
        ));
    }
    // Clearing manage clears delete (defensive normalization).
    let can_delete = can_delete && can_manage;
    let target = manage_target(raw_address)?;
    let is_primary = target.owner_project_id == project_id;
    let db = k2_core::db::shared();
    let conn = db.lock();
    if is_primary {
        let table = match target.source {
            Source::Hosted => "mail_addresses",
            Source::Linked => "mail_external_inboxes",
        };
        conn.execute(
            &format!(
                "UPDATE {table} SET primary_can_manage = ?1, primary_can_delete = ?2 WHERE id = ?3"
            ),
            rusqlite::params![can_manage as i64, can_delete as i64, target.inbox_id],
        )
        .map_err(|e| AccessError::Engine(format!("set primary manage caps: {e}")))?;
    } else {
        // Upsert the grant: a NEW row lands at level 'read' (manage
        // implies at least read visibility); an existing row keeps its
        // level and only the caps change.
        conn.execute(
            "INSERT INTO mail_inbox_grants \
             (source, inbox_id, project_id, level, can_manage, can_delete, created_at) \
             VALUES (?1, ?2, ?3, 'read', ?4, ?5, ?6) \
             ON CONFLICT(source, inbox_id, project_id) \
             DO UPDATE SET can_manage = excluded.can_manage, can_delete = excluded.can_delete",
            rusqlite::params![
                target.source.as_str(),
                target.inbox_id,
                project_id,
                can_manage as i64,
                can_delete as i64,
                now_secs()
            ],
        )
        .map_err(|e| AccessError::Engine(format!("set grant manage caps: {e}")))?;
    }
    Ok((target.address, target.source, is_primary))
}

/// Cascade every grant row for one inbox (on `link remove` / hosted
/// retire) — no grant may outlive the inbox it points at.
pub fn cascade_grants(conn: &rusqlite::Connection, source: Source, inbox_id: &str) {
    let _ = conn.execute(
        "DELETE FROM mail_inbox_grants WHERE source = ?1 AND inbox_id = ?2",
        rusqlite::params![source.as_str(), inbox_id],
    );
}

// ── The unified catalog (GET /cli/mail/inboxes) ─────────────────────────

/// Resolve a project id → workspace display name (name, else path
/// basename).
fn workspace_name(conn: &rusqlite::Connection, project_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT name, path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| {
            let name: Option<String> = r.get(0)?;
            let path: Option<String> = r.get(1)?;
            Ok(name.or_else(|| {
                path.as_deref()
                    .and_then(|p| p.rsplit('/').next())
                    .map(String::from)
            }))
        },
    )
    .ok()
    .flatten()
}

/// One catalog row (pre-shaped, before the viewer filter).
struct CatalogRow {
    source: Source,
    address: String,
    inbox_id: String,
    display_name: Option<String>,
    status: String,
    owner_project_id: String,
    primary_level: String,
    // 0081 management/delete caps (the PRIMARY's own).
    primary_can_manage: bool,
    primary_can_delete: bool,
    // source-specific
    domain: Option<String>,
    host: Option<String>,
    tls: Option<String>,
}

fn load_catalog_rows(conn: &rusqlite::Connection) -> Vec<CatalogRow> {
    let mut rows: Vec<CatalogRow> = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT address, id, owner_project_id, primary_level, status, \
                primary_can_manage, primary_can_delete \
         FROM mail_addresses WHERE status = 'active' ORDER BY created_at, address",
    ) {
        let iter = stmt.query_map([], |r| {
            let address: String = r.get(0)?;
            let domain = address.split_once('@').map(|(_, d)| d.to_string());
            Ok(CatalogRow {
                source: Source::Hosted,
                address,
                inbox_id: r.get(1)?,
                display_name: None,
                owner_project_id: r.get(2)?,
                primary_level: r.get(3)?,
                status: r.get(4)?,
                primary_can_manage: r.get::<_, i64>(5)? != 0,
                primary_can_delete: r.get::<_, i64>(6)? != 0,
                domain,
                host: None,
                tls: None,
            })
        });
        if let Ok(iter) = iter {
            rows.extend(iter.filter_map(Result::ok));
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT email_address, id, owner_project_id, primary_level, status, display_name, \
                host, tls, primary_can_manage, primary_can_delete \
         FROM mail_external_inboxes ORDER BY created_at, email_address",
    ) {
        let iter = stmt.query_map([], |r| {
            Ok(CatalogRow {
                source: Source::Linked,
                address: r.get(0)?,
                inbox_id: r.get(1)?,
                owner_project_id: r.get(2)?,
                primary_level: r.get(3)?,
                status: r.get(4)?,
                display_name: r.get(5)?,
                domain: None,
                host: r.get(6)?,
                tls: r.get(7)?,
                primary_can_manage: r.get::<_, i64>(8)? != 0,
                primary_can_delete: r.get::<_, i64>(9)? != 0,
            })
        });
        if let Ok(iter) = iter {
            rows.extend(iter.filter_map(Result::ok));
        }
    }
    rows
}

/// One grant row for the catalog: `(project_id, level, can_manage,
/// can_delete)`.
struct GrantRow {
    project_id: String,
    level: String,
    can_manage: bool,
    can_delete: bool,
}

fn grants_for(conn: &rusqlite::Connection, source: Source, inbox_id: &str) -> Vec<GrantRow> {
    conn.prepare(
        "SELECT project_id, level, can_manage, can_delete FROM mail_inbox_grants \
         WHERE source = ?1 AND inbox_id = ?2 ORDER BY created_at, project_id",
    )
    .ok()
    .and_then(|mut stmt| {
        stmt.query_map(rusqlite::params![source.as_str(), inbox_id], |r| {
            Ok(GrantRow {
                project_id: r.get::<_, String>(0)?,
                level: r.get::<_, String>(1)?,
                can_manage: r.get::<_, i64>(2)? != 0,
                can_delete: r.get::<_, i64>(3)? != 0,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect::<Vec<_>>())
        .ok()
    })
    .unwrap_or_default()
}

/// The unified inbox catalog. `viewer` = `Some(project_id)` for the
/// agent view (only inboxes that workspace can access, with `yourLevel`);
/// `None` for the owner/admin view (ALL inboxes, full primary + grants).
pub fn catalog_json(viewer: Option<&str>) -> serde_json::Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = load_catalog_rows(&conn);
    let mut out: Vec<serde_json::Value> = Vec::new();
    for row in &rows {
        let grants = grants_for(&conn, row.source, &row.inbox_id);
        // The caller's effective level (agent view filters on it).
        let your = viewer.and_then(|p| {
            effective_level_conn(
                &conn,
                p,
                row.source,
                &row.inbox_id,
                &row.owner_project_id,
                &row.primary_level,
            )
        });
        if viewer.is_some() && your.is_none() {
            continue; // no access → invisible (masking preserved)
        }
        let primary_level = Level::parse(&row.primary_level)
            .unwrap_or(Level::Read)
            .clamp_to(row.source);
        let grants_json: Vec<serde_json::Value> = grants
            .iter()
            .map(|g| {
                serde_json::json!({
                    "projectId": g.project_id,
                    "workspace": workspace_name(&conn, &g.project_id),
                    "level": g.level,
                    "canManage": g.can_manage,
                    "canDelete": g.can_delete,
                })
            })
            .collect();
        let mut entry = serde_json::json!({
            "address": row.address,
            "source": row.source.as_str(),
            "displayName": row.display_name,
            "status": row.status,
            "primary": {
                "projectId": row.owner_project_id,
                "workspace": workspace_name(&conn, &row.owner_project_id),
                "level": primary_level.as_str(),
                "canManage": row.primary_can_manage,
                "canDelete": row.primary_can_delete,
            },
            "grants": grants_json,
            "yourLevel": your.map(|l| l.as_str()),
            "maxLevel": row.source.max_level().as_str(),
        });
        // Source-specific fields.
        if let Some(obj) = entry.as_object_mut() {
            match row.source {
                Source::Hosted => {
                    obj.insert("domain".to_string(), serde_json::json!(row.domain));
                }
                Source::Linked => {
                    obj.insert("host".to_string(), serde_json::json!(row.host));
                    obj.insert("tls".to_string(), serde_json::json!(row.tls));
                }
            }
        }
        out.push(entry);
    }
    serde_json::json!({ "ok": true, "count": out.len(), "inboxes": out })
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — the unified layer over BOTH sources, shared test
// DB, no network (house rules).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::messages::ReadError;

    fn unique_project() -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, format!("acc-{id}"), format!("/tmp/acc-{id}")],
        )
        .expect("project row");
        id
    }

    fn unique_addr(label: &str) -> String {
        format!("{label}-{}@acc-test.example", &uuid::Uuid::new_v4().simple().to_string()[..12])
    }

    /// Seed a hosted ACTIVE address owned by `owner` (default primary
    /// 'send'). Returns the row id.
    fn seed_hosted(owner: &str, address: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at, primary_level) \
             VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100, 'send')",
            rusqlite::params![id, address, format!("acct-{}", &id[..8]), owner],
        )
        .expect("seed hosted");
        id
    }

    /// Seed a linked inbox owned by `owner` (default primary 'draft').
    fn seed_linked(owner: &str, address: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, primary_level) \
             VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100, 'draft')",
            rusqlite::params![id, owner, address],
        )
        .expect("seed linked");
        id
    }

    fn cleanup(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_inbox_grants WHERE project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_inbox_grants WHERE inbox_id IN \
             (SELECT id FROM mail_addresses WHERE owner_project_id = ?1) \
             OR inbox_id IN (SELECT id FROM mail_external_inboxes WHERE owner_project_id = ?1)",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    }

    #[test]
    fn level_ordering_and_source_ceilings() {
        assert!(Level::Read < Level::Draft && Level::Draft < Level::Send);
        assert_eq!(Source::Hosted.max_level(), Level::Send);
        // §17.5: linked now ceilings at 'send' too (opt-in via SMTP).
        assert_eq!(Source::Linked.max_level(), Level::Send);
        assert_eq!(Level::Send.clamp_to(Source::Linked), Level::Send);
        assert_eq!(Level::Send.clamp_to(Source::Hosted), Level::Send);
    }

    #[test]
    fn hosted_primary_reads_drafts_and_sends_foreign_is_masked() {
        let owner = unique_project();
        let addr = unique_addr("hosted");
        seed_hosted(&owner, &addr);

        // Primary at 'send' clears every gate.
        assert_eq!(can_read(&owner, &addr).unwrap().source, Source::Hosted);
        assert!(can_draft(&owner, &addr).is_ok());
        let send = can_send(&owner, &addr).expect("hosted primary sends");
        assert_eq!(send.your_level, Level::Send);
        assert!(send.account_id.is_some(), "hosted carries the Stalwart account");

        // A foreign workspace is masked on ALL three (no existence leak).
        for r in [
            can_read("nobody", &addr).err(),
            can_draft("nobody", &addr).err(),
            can_send("nobody", &addr).err(),
        ] {
            assert!(matches!(r, Some(ReadError::NotFound(_))), "masked");
        }
        cleanup(&owner);
    }

    #[test]
    fn linked_send_is_opt_in_off_by_default_then_reachable() {
        let owner = unique_project();
        let addr = unique_addr("linked");
        seed_linked(&owner, &addr); // seeds primary_level 'draft'
        assert!(can_read(&owner, &addr).is_ok());
        assert!(can_draft(&owner, &addr).is_ok());
        // Default linked primary is 'draft' — send is masked (opt-in off).
        assert!(matches!(can_send(&owner, &addr), Err(ReadError::NotFound(_))));
        // Raise the primary's own ceiling to 'send' — now it can send,
        // and the resolved inbox carries the linked row for the SMTP path.
        set_level(&addr, &owner, "send").expect("linked primary → send");
        let sendable = can_send(&owner, &addr).expect("linked can now send");
        assert_eq!(sendable.source, Source::Linked);
        assert!(sendable.linked.is_some(), "linked send carries the external row");
        cleanup(&owner);
    }

    #[test]
    fn grants_extend_access_across_both_sources_with_masking() {
        let owner = unique_project();
        let reader = unique_project();
        let sender = unique_project();
        let hosted = unique_addr("gh");
        let linked = unique_addr("gl");
        seed_hosted(&owner, &hosted);
        seed_linked(&owner, &linked);

        // Baseline: only the primary; others masked.
        assert!(matches!(can_read(&reader, &hosted), Err(ReadError::NotFound(_))));

        // 'read' grant on hosted → read OK, draft still masked.
        grant_access(&hosted, &reader, "read").expect("grant read");
        assert!(can_read(&reader, &hosted).is_ok());
        assert!(matches!(can_draft(&reader, &hosted), Err(ReadError::NotFound(_))));

        // 'send' grant on hosted → can_send passes.
        grant_access(&hosted, &sender, "send").expect("grant send");
        assert!(can_send(&sender, &hosted).is_ok());

        // 'draft' on linked → can_draft passes, can_send masked.
        grant_access(&linked, &sender, "draft").expect("grant draft");
        assert!(can_draft(&sender, &linked).is_ok());
        assert!(matches!(can_send(&sender, &linked), Err(ReadError::NotFound(_))));
        // §17.5: 'send' on a LINKED inbox is now ALLOWED (SMTP path) —
        // upgrading the grant lets that workspace send.
        grant_access(&linked, &sender, "send").expect("linked send grant");
        let s = can_send(&sender, &linked).expect("linked grant can send");
        assert_eq!(s.source, Source::Linked);

        // Granting the PRIMARY teaches; unknown inbox → not_found.
        assert!(matches!(grant_access(&hosted, &owner, "read"), Err(AccessError::Usage(_))));
        assert!(matches!(
            grant_access("ghost@nowhere.example", &reader, "read"),
            Err(AccessError::NotFound(_))
        ));

        // Revoke is idempotent; revoking the primary teaches.
        revoke_access(&hosted, &reader).expect("revoke");
        assert!(matches!(can_read(&reader, &hosted), Err(ReadError::NotFound(_))));
        revoke_access(&hosted, &reader).expect("idempotent");
        assert!(matches!(revoke_access(&hosted, &owner), Err(AccessError::Usage(_))));

        cleanup(&owner);
        cleanup(&reader);
        cleanup(&sender);
    }

    #[test]
    fn set_level_updates_primary_ceiling_and_grants() {
        let owner = unique_project();
        let grantee = unique_project();
        let hosted = unique_addr("sl");
        seed_hosted(&owner, &hosted);

        // Lower the primary's own ceiling to 'read' — it can no longer send.
        let (_, _, is_primary) = set_level(&hosted, &owner, "read").expect("set primary level");
        assert!(is_primary);
        assert!(can_read(&owner, &hosted).is_ok());
        assert!(matches!(can_send(&owner, &hosted), Err(ReadError::NotFound(_))));

        // set-level on a non-primary upserts a grant.
        let (_, _, is_primary) = set_level(&hosted, &grantee, "draft").expect("set grant level");
        assert!(!is_primary);
        assert!(can_draft(&grantee, &hosted).is_ok());
        // §17.5: linked primary set to 'send' is now allowed (SMTP path).
        let linked = unique_addr("sll");
        seed_linked(&owner, &linked);
        let (_, src, is_primary) = set_level(&linked, &owner, "send").expect("linked → send");
        assert!(is_primary && src == Source::Linked);
        assert!(can_send(&owner, &linked).is_ok());

        cleanup(&owner);
        cleanup(&grantee);
    }

    #[test]
    fn set_primary_transfers_and_demotes_the_old_primary_to_a_grant() {
        let owner = unique_project();
        let heir = unique_project();
        let hosted = unique_addr("sp");
        seed_hosted(&owner, &hosted);
        // heir starts with a 'read' grant.
        grant_access(&hosted, &heir, "read").expect("seed grant");

        set_primary(&hosted, &heir).expect("transfer");
        // heir now manages (its grant row is gone; it reads via primary).
        assert!(grant_level(Source::Hosted, &manage_inbox_id(&hosted), &heir).is_none());
        assert!(can_read(&heir, &hosted).is_ok());
        // old owner kept access as a grant at its prior 'send' ceiling.
        assert_eq!(
            grant_level(Source::Hosted, &manage_inbox_id(&hosted), &owner),
            Some(Level::Send)
        );
        assert!(can_send(&owner, &hosted).is_ok());
        // same-primary transfer teaches.
        assert!(matches!(set_primary(&hosted, &heir), Err(AccessError::Usage(_))));

        cleanup(&owner);
        cleanup(&heir);
    }

    fn manage_inbox_id(address: &str) -> String {
        super::manage_target(address).expect("target").inbox_id
    }

    #[test]
    fn manage_and_delete_caps_gate_and_mask() {
        let owner = unique_project();
        let helper = unique_project();
        let stranger = unique_project();
        let addr = unique_addr("mng");
        seed_hosted(&owner, &addr); // primary_level 'send'; caps default OFF

        // Caps default OFF (opt-in): even the primary is masked until granted.
        assert!(matches!(can_manage(&owner, &addr), Err(ReadError::NotFound(_))));
        assert!(matches!(can_delete(&owner, &addr), Err(ReadError::NotFound(_))));

        // Enable manage on the primary → manage OK; delete still TEACHES
        // (usage — the caller already sees the inbox, so nothing leaks).
        set_manage(&addr, &owner, true, false).expect("primary manage on");
        assert!(can_manage(&owner, &addr).is_ok());
        assert!(matches!(can_delete(&owner, &addr), Err(ReadError::Usage(_))));

        // Enable delete → delete OK.
        set_manage(&addr, &owner, true, true).expect("primary delete on");
        assert!(can_delete(&owner, &addr).is_ok());

        // canDelete REQUIRES canManage (delete=true, manage=false → usage).
        assert!(matches!(set_manage(&addr, &owner, false, true), Err(AccessError::Usage(_))));

        // Clearing manage clears delete (and masks both again).
        set_manage(&addr, &owner, false, false).expect("primary manage off");
        assert!(matches!(can_manage(&owner, &addr), Err(ReadError::NotFound(_))));
        assert!(matches!(can_delete(&owner, &addr), Err(ReadError::NotFound(_))));

        // A grantee gets manage via an UPSERTED read grant (caps are
        // orthogonal to, but never below, read visibility).
        set_manage(&addr, &helper, true, false).expect("grant manage");
        assert!(can_manage(&helper, &addr).is_ok());
        assert!(can_read(&helper, &addr).is_ok());
        assert_eq!(grant_level(Source::Hosted, &manage_inbox_id(&addr), &helper), Some(Level::Read));
        // Manage but not delete → delete teaches (usage), not masked.
        assert!(matches!(can_delete(&helper, &addr), Err(ReadError::Usage(_))));

        // A stranger with NO access is masked on both (no existence leak).
        assert!(matches!(can_manage(&stranger, &addr), Err(ReadError::NotFound(_))));
        assert!(matches!(can_delete(&stranger, &addr), Err(ReadError::NotFound(_))));

        cleanup(&owner);
        cleanup(&helper);
        cleanup(&stranger);
    }

    #[test]
    fn linked_manage_caps_resolve_the_external_row() {
        let owner = unique_project();
        let addr = unique_addr("lmng");
        seed_linked(&owner, &addr);
        set_manage(&addr, &owner, true, true).expect("linked primary caps");
        let inbox = can_delete(&owner, &addr).expect("linked delete ok");
        assert_eq!(inbox.source, Source::Linked);
        assert!(inbox.linked.is_some(), "linked manage carries the external row");
        cleanup(&owner);
    }

    #[test]
    fn catalog_exposes_manage_and_delete_caps() {
        let owner = unique_project();
        let grantee = unique_project();
        let addr = unique_addr("mcap");
        seed_hosted(&owner, &addr);
        set_manage(&addr, &owner, true, true).expect("primary caps");
        set_manage(&addr, &grantee, true, false).expect("grant caps");

        let all = catalog_json(None);
        let row = all["inboxes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["address"] == addr.as_str())
            .expect("listed");
        assert_eq!(row["primary"]["canManage"], true);
        assert_eq!(row["primary"]["canDelete"], true);
        let g = row["grants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["projectId"] == grantee)
            .expect("grant listed");
        assert_eq!(g["canManage"], true);
        assert_eq!(g["canDelete"], false);

        cleanup(&owner);
        cleanup(&grantee);
    }

    #[test]
    fn catalog_agent_view_filters_owner_view_lists_all() {
        let owner = unique_project();
        let other = unique_project();
        let stranger = unique_project();
        let hosted = unique_addr("ch");
        seed_hosted(&owner, &hosted);
        grant_access(&hosted, &other, "draft").expect("grant");

        // Owner view (None): the inbox is present with full primary+grants.
        let all = catalog_json(None);
        let row = all["inboxes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["address"] == hosted.as_str())
            .expect("listed in owner view");
        assert_eq!(row["source"], "hosted");
        assert_eq!(row["maxLevel"], "send");
        assert_eq!(row["primary"]["projectId"], owner);
        assert!(row["yourLevel"].is_null());
        assert_eq!(row["grants"][0]["level"], "draft");

        // Agent view for `other`: sees it with yourLevel=draft.
        let mine = catalog_json(Some(&other));
        let row = mine["inboxes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["address"] == hosted.as_str())
            .expect("granted workspace sees it");
        assert_eq!(row["yourLevel"], "draft");

        // Agent view for a stranger: the inbox is invisible (masked).
        let none = catalog_json(Some(&stranger));
        assert!(
            !none["inboxes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["address"] == hosted.as_str()),
            "no access → not in the catalog"
        );

        cleanup(&owner);
        cleanup(&other);
        cleanup(&stranger);
    }
}
