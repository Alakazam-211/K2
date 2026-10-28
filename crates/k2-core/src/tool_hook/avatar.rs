//! Sew for Apps A3 (0.46.6): the person's picture, from K2 to Sew.
//!
//! **K2 is the only source of the profile and the picture** (Rosson
//! 2026-10-28). A person's 0.46.5 profile picture (K2's own JPEG
//! re-encode, ≤ 64 KiB) reaches their Sew member through two outbox ops on
//! the S27 worker, on the member's own subject queue (so after its
//! provision):
//!
//! | Op | Sew call | Body |
//! |---|---|---|
//! | `avatar_set` | `POST /mgmt/v1/principals/avatar?external_id=…&force=true` | the raw JPEG (`image/jpeg`) |
//! | `avatar_clear` | `POST /mgmt/v1/principals/avatar/clear` | `{external_id, force: true}` |
//!
//! `force` (sent only when Sew reports the `k2_managed_avatar` flag)
//! replaces a picture the member uploaded in Sew: K2 is the source.
//!
//! The `avatar_set` row stores only the picture's hash; the worker reads
//! the bytes at send time ([`picture_for_send`]). A row whose hash is no
//! longer the person's picture is finished unsent: a newer row (set or
//! clear) for the same person is already behind it in the queue.
//!
//! **Triggers** (each in the same skin.db transaction as the K2 write):
//! the picture set or cleared (`skin::profile`), and a chat grant created,
//! enabled or changed while a picture exists. **Reconcile** (the sweep's
//! backstop): `tool_chat_avatars` remembers the hash last delivered per
//! member; a person with an enabled chat grant whose picture differs gets
//! one op. A new Sew install (purge, reinstall) forgets the table, so every
//! picture goes again.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use super::sew_wire::ExternalId;
use super::{grant_rev, Cause, NewOp, Op, CHAT_KIND};
use crate::skin::SkinGrant;

/// `POST`: the raw picture (`?external_id=&force=`).
pub const MGMT_AVATAR: &str = "/mgmt/v1/principals/avatar";
/// `POST`: clear a pushed picture.
pub const MGMT_AVATAR_CLEAR: &str = "/mgmt/v1/principals/avatar/clear";
/// The content type of every picture K2 sends (its own re-encode).
pub const CONTENT_TYPE: &str = "image/jpeg";

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_chat_avatars (
            external_id TEXT PRIMARY KEY NOT NULL,
            image_hash  TEXT,
            pushed_at   INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db chat avatars schema: {e}"))
}

/// `POST /mgmt/v1/principals/avatar?external_id=<ext>&force=<force>`
/// (`force` from the row: see [`op`]).
pub fn set_path(ext: &str, force: bool) -> String {
    format!("{MGMT_AVATAR}?external_id={ext}&force={force}")
}

/// The `force` an `avatar_set` row was queued with.
pub fn row_force(payload_raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload_raw).ok().and_then(|v| v.get("force").and_then(|f| f.as_bool())).unwrap_or(false)
}

/// The person's current picture hash (`principal_profiles.image_hash`).
pub fn picture_hash(conn: &Connection, principal_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT image_hash FROM principal_profiles WHERE principal_id = ?1 AND image IS NOT NULL",
        params![principal_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map(Option::flatten)
    .map_err(|e| format!("profile picture lookup: {e}"))
}

fn op(conn: &Connection, principal_id: &str, hash: Option<&str>, grant: Option<&SkinGrant>, cause: Cause) -> Result<NewOp, String> {
    let ext = ExternalId::K2Principal(principal_id.to_string()).to_string();
    // Feature flag, never a version: only a Sew that manages the avatar for
    // K2 (`k2_managed_avatar`) takes `force` (it replaces a picture the
    // member set themselves).
    let force = super::features::has(conn, CHAT_KIND, super::features::K2_MANAGED_AVATAR)?;
    let (op, payload) = match hash {
        Some(h) => (Op::AvatarSet, json!({ "external_id": ext, "image_hash": h, "force": force })),
        None => (Op::AvatarClear, json!({ "external_id": ext, "force": force })),
    };
    Ok(NewOp {
        tool: CHAT_KIND,
        op,
        subject_key: ext,
        payload,
        grant_id: grant.map(|g| g.id.clone()),
        grant_rev: grant.map(grant_rev),
        cause,
        after_subject: None,
    })
}

/// The op that makes Sew show the person's current picture (`None` when
/// they have no enabled chat grant: no member to change).
pub fn current_op(conn: &Connection, principal_id: &str, cause: Cause) -> Result<Option<NewOp>, String> {
    let Some(g) = super::chat::person_grant(conn, principal_id)? else { return Ok(None) };
    if !g.enabled {
        return Ok(None);
    }
    let hash = picture_hash(conn, principal_id)?;
    Ok(Some(op(conn, principal_id, hash.as_deref(), Some(&g), cause)?))
}

/// After a provision of `grant`'s person: push the picture they have (a
/// person without one needs nothing: a new member has none).
pub fn after_provision(conn: &Connection, grant: &SkinGrant, cause: Cause) -> Result<Option<NewOp>, String> {
    if grant.subject_kind != "principal" {
        return Ok(None);
    }
    match picture_hash(conn, &grant.subject_id)? {
        Some(h) => Ok(Some(op(conn, &grant.subject_id, Some(&h), Some(grant), cause)?)),
        None => Ok(None),
    }
}

/// The picture changed (`skin::profile`): queue its op in the caller's
/// open transaction. Returns how many rows went in (0 without chat).
pub fn on_picture_changed(conn: &Connection, principal_id: &str, actor: &str) -> Result<usize, String> {
    if super::outbox::get_target(conn, CHAT_KIND)?.is_none() {
        return Ok(0);
    }
    let Some(o) = current_op(conn, principal_id, Cause::PrincipalUpdate)? else { return Ok(0) };
    Ok(super::outbox::enqueue(conn, &[o], actor, super::outbox::now_secs())?.len())
}

/// What the worker sends for an `avatar_set` row: the picture's bytes when
/// they still have the row's hash; `None` = superseded (the picture
/// changed or went: a newer row follows).
pub fn picture_for_send(conn: &Connection, payload: &serde_json::Value) -> Result<Option<Vec<u8>>, String> {
    let ext = payload.get("external_id").and_then(|v| v.as_str()).unwrap_or("");
    let want = payload.get("image_hash").and_then(|v| v.as_str()).unwrap_or("");
    let Some(pid) = ext.strip_prefix("k2:principal:") else {
        return Err(format!("avatar row for '{ext}': only a person has a picture"));
    };
    let row: Option<(Option<Vec<u8>>, Option<String>)> = conn
        .query_row(
            "SELECT image, image_hash FROM principal_profiles WHERE principal_id = ?1",
            params![pid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("profile picture read: {e}"))?;
    Ok(match row {
        Some((Some(bytes), Some(h))) if h == want => Some(bytes),
        _ => None,
    })
}

/// A delivered avatar row: remember what Sew shows now.
pub fn after_done(conn: &Connection, row_id: &str, now: i64) -> Result<(), String> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT op, subject_key, payload FROM tool_outbox WHERE id = ?1",
            params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("avatar row read: {e}"))?;
    let Some((op, subject, payload)) = row else { return Ok(()) };
    let hash: Option<String> = match Op::parse(&op) {
        Some(Op::AvatarSet) => serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.get("image_hash").and_then(|h| h.as_str()).map(str::to_string)),
        Some(Op::AvatarClear) => None,
        _ => return Ok(()),
    };
    record(conn, &subject, hash.as_deref(), now)
}

/// Remember the hash Sew shows for `ext` (`None` = no picture).
pub fn record(conn: &Connection, ext: &str, hash: Option<&str>, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_chat_avatars (external_id, image_hash, pushed_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(external_id) DO UPDATE SET image_hash = excluded.image_hash, pushed_at = excluded.pushed_at",
        params![ext, hash, now],
    )
    .map(|_| ())
    .map_err(|e| format!("chat avatar record: {e}"))
}

/// What Sew shows for `ext`, as far as K2 knows: `None` = never pushed,
/// `Some(None)` = cleared, `Some(Some(h))` = that picture.
pub fn pushed(conn: &Connection, ext: &str) -> Result<Option<Option<String>>, String> {
    conn.query_row("SELECT image_hash FROM tool_chat_avatars WHERE external_id = ?1", params![ext], |r| {
        r.get::<_, Option<String>>(0)
    })
    .optional()
    .map_err(|e| format!("chat avatar lookup: {e}"))
}

/// A new Sew install: nothing K2 pushed is there any more.
pub fn forget_all(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM tool_chat_avatars", []).map(|_| ()).map_err(|e| format!("chat avatar reset: {e}"))
}

/// Reconcile (inside the sweep's transaction): one op per person with an
/// enabled chat grant whose picture isn't what Sew shows, skipping
/// subjects with an open row (`busy`).
pub fn plan(conn: &Connection, busy: &std::collections::HashSet<String>) -> Result<Vec<NewOp>, String> {
    let mut st = conn
        .prepare(&format!(
            "{} WHERE kind = ?1 AND subject_kind = 'principal' AND enabled = 1 ORDER BY created_at, id",
            crate::skin::GRANT_SELECT
        ))
        .map_err(|e| format!("avatar plan: {e}"))?;
    let grants: Vec<SkinGrant> = st
        .query_map(params![CHAT_KIND], crate::skin::map_grant_row)
        .map_err(|e| format!("avatar plan: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("avatar plan row: {e}"))?;
    drop(st);
    let mut out = Vec::new();
    for g in grants {
        let ext = ExternalId::K2Principal(g.subject_id.clone()).to_string();
        if busy.contains(&ext) {
            continue;
        }
        let want = picture_hash(conn, &g.subject_id)?;
        let have = pushed(conn, &ext)?;
        let differs = match (&want, &have) {
            (None, None) => false,
            (None, Some(h)) => h.is_some(),
            (Some(w), Some(Some(h))) => w != h,
            (Some(_), _) => true,
        };
        if differs {
            out.push(op(conn, &g.subject_id, want.as_deref(), Some(&g), Cause::Reconcile)?);
        }
    }
    Ok(out)
}
