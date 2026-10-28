//! Sew for Apps A6 (0.46.6): the identity map, K2 principal ↔ Sew member.
//!
//! **One identity.** A K2 App user is a skin principal; their Sew member is
//! derived from them at provision. The stable key is the external id
//! `k2:principal:<principal uuid>` (never the email or the username: both
//! change). Sew is canonical for `member_id`; this table is K2's durable
//! mirror, so Apps can resolve either way (`chat.member`, `chat.whois`)
//! without asking Sew.
//!
//! `tool_chat_members` (skin.db, idempotent DDL like `tool_outbox`):
//! - written by the S27 worker from every delivered `provision`
//!   (`ProvisionResponse.member_id`) and `deprovision`, in the same
//!   transaction that closes the outbox row ([`after_done`]);
//! - repaired by every reconcile sweep from Sew's `GET /mgmt/v1/principals`
//!   list ([`reconcile`]);
//! - **tombstones:** a person K2 deleted keeps their row as `deleted`
//!   (their member is deactivated in Sew, history stays), so old messages
//!   still resolve to "a deleted user". A person invited again is a new
//!   principal (a new UUID: K2 never reuses one), so a new Sew member: the
//!   old history stays with the tombstone (Rosson 2026-10-28, Q11);
//! - a new Sew install (purge, reinstall) changes every `member_id`: the
//!   table is cleared ([`forget_all`]) and refilled as K2 re-provisions.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::sew_wire::{DeprovisionResponse, ExternalId, MemberKind, MgmtPrincipal, ProvisionResponse};
use super::{Cause, Op};

pub const ACTIVE: &str = "active";
pub const DEACTIVATED: &str = "deactivated";
/// The person was deleted in K2 (a tombstone; never reactivated).
pub const DELETED: &str = "deleted";

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_chat_members (
            external_id TEXT PRIMARY KEY NOT NULL,
            member_id   TEXT NOT NULL,
            kind        TEXT NOT NULL,
            username    TEXT NOT NULL DEFAULT '',
            state       TEXT NOT NULL CHECK (state IN ('active','deactivated','deleted')),
            updated_at  INTEGER NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS tool_chat_members_member ON tool_chat_members(member_id);",
    )
    .map_err(|e| format!("skin.db chat members schema: {e}"))
}

/// One mapped member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRow {
    pub external_id: String,
    pub member_id: String,
    /// `person` | `agent`.
    pub kind: String,
    pub username: String,
    pub state: String,
    pub updated_at: i64,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemberRow> {
    Ok(MemberRow {
        external_id: r.get(0)?,
        member_id: r.get(1)?,
        kind: r.get(2)?,
        username: r.get(3)?,
        state: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

const SELECT: &str = "SELECT external_id, member_id, kind, username, state, updated_at FROM tool_chat_members";

pub fn by_external_id(conn: &Connection, ext: &str) -> Result<Option<MemberRow>, String> {
    conn.query_row(&format!("{SELECT} WHERE external_id = ?1"), params![ext], row)
        .optional()
        .map_err(|e| format!("chat member lookup: {e}"))
}

pub fn by_member_id(conn: &Connection, member_id: &str) -> Result<Option<MemberRow>, String> {
    conn.query_row(&format!("{SELECT} WHERE member_id = ?1"), params![member_id], row)
        .optional()
        .map_err(|e| format!("chat member lookup: {e}"))
}

fn kind_str(k: MemberKind) -> &'static str {
    match k {
        MemberKind::Person => "person",
        MemberKind::Agent => "agent",
    }
}

/// Upsert `ext ↔ member_id`. A row holding the same `member_id` under
/// another external id is dropped first (Sew is canonical). A tombstone
/// stays a tombstone.
pub fn upsert(conn: &Connection, ext: &str, member_id: &str, kind: &str, username: Option<&str>, state: &str, now: i64) -> Result<(), String> {
    conn.execute("DELETE FROM tool_chat_members WHERE member_id = ?1 AND external_id != ?2", params![member_id, ext])
        .map_err(|e| format!("chat member upsert: {e}"))?;
    conn.execute(
        "INSERT INTO tool_chat_members (external_id, member_id, kind, username, state, updated_at)
         VALUES (?1, ?2, ?3, COALESCE(?4, ''), ?5, ?6)
         ON CONFLICT(external_id) DO UPDATE SET
           member_id = excluded.member_id, kind = excluded.kind,
           username = COALESCE(?4, tool_chat_members.username),
           state = CASE WHEN tool_chat_members.state = 'deleted' THEN 'deleted' ELSE excluded.state END,
           updated_at = excluded.updated_at",
        params![ext, member_id, kind, username, state, now],
    )
    .map(|_| ())
    .map_err(|e| format!("chat member upsert: {e}"))
}

/// Mark `ext` (if mapped) with `state`.
fn set_state(conn: &Connection, ext: &str, state: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_chat_members SET state = CASE WHEN state = 'deleted' THEN 'deleted' ELSE ?2 END, updated_at = ?3
         WHERE external_id = ?1",
        params![ext, state, now],
    )
    .map(|_| ())
    .map_err(|e| format!("chat member state: {e}"))
}

/// Make `ext` a tombstone (the person was deleted in K2).
pub fn tombstone(conn: &Connection, ext: &str, now: i64) -> Result<(), String> {
    conn.execute("UPDATE tool_chat_members SET state = 'deleted', updated_at = ?2 WHERE external_id = ?1", params![ext, now])
        .map(|_| ())
        .map_err(|e| format!("chat member tombstone: {e}"))
}

/// A delivered row (inside the transaction that closes it): a provision
/// maps its member; a deprovision deactivates it (a tombstone when the
/// cause was the person's deletion).
pub fn after_done(conn: &Connection, row_id: &str, result: &str, now: i64) -> Result<(), String> {
    let r: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT op, subject_key, payload, cause FROM tool_outbox WHERE id = ?1",
            params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| format!("chat member row read: {e}"))?;
    let Some((op, subject, payload, cause)) = r else { return Ok(()) };
    match Op::parse(&op) {
        Some(Op::Provision) => {
            let Ok(resp) = serde_json::from_str::<ProvisionResponse>(result) else {
                crate::log_debug!("[grant-hook] provision of {subject} answered no member_id; the sweep maps it");
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
            let kind = p.get("kind").and_then(|k| k.as_str()).unwrap_or("person").to_string();
            let username = p.pointer("/profile/username").and_then(|u| u.as_str());
            upsert(conn, &subject, &resp.member_id, &kind, username, ACTIVE, now)
        }
        Some(Op::Deprovision) => {
            let state = if cause == Cause::PrincipalDelete.as_str() { DELETED } else { DEACTIVATED };
            if let Ok(resp) = serde_json::from_str::<DeprovisionResponse>(result) {
                if by_external_id(conn, &subject)?.is_none() {
                    let kind = if subject.starts_with("k2:workspace:") { "agent" } else { "person" };
                    upsert(conn, &subject, &resp.member_id, kind, None, state, now)?;
                }
            }
            if state == DELETED {
                tombstone(conn, &subject, now)
            } else {
                set_state(conn, &subject, state, now)
            }
        }
        _ => Ok(()),
    }
}

/// Reconcile: Sew's member list is canonical for `member_id`. Every K2
/// external id Sew lists is mapped with its state; a person K2 no longer
/// has becomes a tombstone (`principal_exists` = false).
pub fn reconcile(conn: &Connection, actual: &[MgmtPrincipal], now: i64) -> Result<usize, String> {
    let mut n = 0;
    for m in actual {
        let ext = m.external_id.to_string();
        let principal = match &m.external_id {
            ExternalId::K2Principal(id) => Some(id.clone()),
            ExternalId::K2Workspace(_) | ExternalId::K2Connect(_) | ExternalId::K2Owner => None,
            ExternalId::Local(_) | ExternalId::Other(_) => continue,
        };
        let state = if m.deactivated { DEACTIVATED } else { ACTIVE };
        upsert(conn, &ext, &m.member_id, kind_str(m.kind), Some(&m.username), state, now)?;
        if let Some(pid) = principal {
            let gone: bool = conn
                .query_row("SELECT NOT EXISTS(SELECT 1 FROM principals WHERE id = ?1)", params![pid], |r| r.get(0))
                .map_err(|e| format!("chat member principal check: {e}"))?;
            if gone {
                tombstone(conn, &ext, now)?;
            }
        }
        n += 1;
    }
    Ok(n)
}

/// A new Sew install: every `member_id` K2 knew is gone.
pub fn forget_all(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM tool_chat_members", []).map(|_| ()).map_err(|e| format!("chat members reset: {e}"))
}
