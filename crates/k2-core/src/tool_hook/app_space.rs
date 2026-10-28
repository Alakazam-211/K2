//! App-scoped Space membership (plan-sew-s30-s32 K31; Sew SW13, contracts
//! rev 147).
//!
//! **The rule.** A Sew Space bound to a K2 App gets its members ONLY from
//! that App's user grants: a person is a member of the App's Space while
//! they hold an enabled `kind=app` grant for that App AND an enabled
//! `chat` grant with scope `member` (their Sew account, S27). Nothing else
//! adds anyone:
//! - **No Connect-role auto-provision** (narrows K5, Rosson 2026-10-15):
//!   a Connect Owner or Admin is server control (`k2 hostchat`), never an
//!   App Space member by role. Connect logins can't hold `kind=app` grants
//!   (`skin::create_plain_grant` refuses `subjectKind connect`), and the
//!   desired set below reads `subject_kind = 'principal'` rows only.
//! - The box owner (`k2:owner`) and agents are never added here either:
//!   App Space adds are App users only. An App adds its own agents or
//!   managers through `k2 chat` (S30) where it needs them.
//! - Chat guests are left out: Sew admits a guest to a Space only through
//!   a channel add (S19), so a mgmt add would answer `not_added: guest`.
//!
//! **Bindings.** `tool_app_spaces` records which Sew Space belongs to which
//! App (one App per Space; an App may have several). K2 binds Sew's side
//! too (`POST /mgmt/v1/spaces/bind`, once) from the daemon route before it
//! writes the row here. Unbinding forgets the row; the members K2 added
//! are then removed by the next plan (nobody's grant maps there any more).
//!
//! **Ledger.** Sew has no mgmt list of a Space's members, so K2 keeps what
//! it asked for in `tool_app_space_members` (`pending_add` → `added`,
//! `held`, `pending_remove` → row gone). [`plan`] diffs the desired set
//! against it and enqueues outbox ops on the S27 worker (tool `chat`):
//! `space_member_add` (after the person's own provisioning queue, so the
//! add never races the provision) and `space_member_remove`, one person
//! per row, subject key `space:<space id>:<external id>`.
//!
//! **HC2.** A member Sew lists in `not_added` for a reason other than
//! `not_provisioned` (deactivated, assistant, guest) is recorded `held`
//! with the grant rev it was held under, and is not re-added until that rev
//! changes. `not_provisioned` is retried with backoff (the person's
//! provision is still queued).
//!
//! **Banned (Rosson 2026-10-27, ticket 411eda53).** A member Sew lists in
//! `skipped` is banned in that Space. K2 never re-adds them on its own: not
//! when their grant changes, and not after their grant goes and comes back
//! (the held row outlives the grant). Only [`clear_banned`] lets K2 add them
//! again; the Sew 0.4.0 wiring (unban, and public → private Spaces) calls it
//! once that version is pinned (slot: [`SEW_040_UNBAN_SLOT`]).
//!
//! **Sew's side of an unbind (Sew 0.5.0, flag `space_unbind`).** "Unbind"
//! means one thing everywhere (K2 2026-10-28): an App removed from the box
//! (its bindings forgotten, members kept, Rosson's unbind default) and an
//! explicit `k2 hostchat app-space unbind` both ask Sew to unbind the Space
//! too (`POST /mgmt/v1/spaces/unbind {space_id, app_id}` → `{space_id,
//! app_id?, orphaned_from?, changed}`): the Space becomes an orphan of that
//! App, read-only (adds and channel creates answer 422 `space_orphaned`, removes still work; reads and export
//! still work). The wish is kept in `tool_app_space_unbinds` and sent by
//! the S27 worker (op `space_unbind`, subject `unbind:<space id>`) once Sew
//! reports the flag, so an unbind before the upgrade lands after it, and
//! only after the Space's own membership rows are settled (an unbind with
//! `--remove-app-members` removes them first). Binding the same App again
//! before it is sent drops it. K2 never releases an orphan
//! (`spaces/release` lets another App bind it): that is the Sew install
//! owner's call.
//!
//! Tables live in `skin.db` (idempotent DDL like `tool_outbox`), so this
//! needs no main-DB migration.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::sew_wire::{
    self, ExternalId, SpaceMemberRemoveRequest, SpaceMembersAddRequest, SpaceMembersAddResponse,
};
use super::{outbox, Cause, NewOp, Op, CHAT_KIND};

/// Ledger states.
pub const PENDING_ADD: &str = "pending_add";
pub const ADDED: &str = "added";
pub const HELD: &str = "held";
/// The `why` of a held row that Sew skipped as banned. Sticky: see the
/// module note.
pub const BANNED_WHY: &str = "skipped: banned in this Space";
/// Sew 0.4.0 slot: bans and private Spaces land there (Rosson green-lit
/// Sew building them, 2026-10-27). When K2 pins 0.4.0, the unban event (or
/// a members listing that no longer shows the ban) calls [`clear_banned`].
pub const SEW_040_UNBAN_SLOT: &str = "sew-0.4.0: unban → clear_banned";

fn is_banned(why: Option<&str>) -> bool {
    why == Some(BANNED_WHY)
}
pub const PENDING_REMOVE: &str = "pending_remove";

/// Outbox subject-key prefix for Space membership rows.
pub const SUBJECT_PREFIX: &str = "space:";

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_app_spaces (
            app_id    TEXT NOT NULL,
            space_id  TEXT NOT NULL,
            bound_at  INTEGER NOT NULL,
            bound_by  TEXT NOT NULL,
            PRIMARY KEY (app_id, space_id)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS tool_app_spaces_space
            ON tool_app_spaces(space_id);
         CREATE TABLE IF NOT EXISTS tool_app_space_members (
            space_id    TEXT NOT NULL,
            app_id      TEXT NOT NULL,
            external_id TEXT NOT NULL,
            state       TEXT NOT NULL
                CHECK (state IN ('pending_add','added','held','pending_remove')),
            grant_rev   TEXT,
            why         TEXT,
            updated_at  INTEGER NOT NULL,
            PRIMARY KEY (space_id, external_id)
         );
         CREATE TABLE IF NOT EXISTS tool_app_space_kept (
            space_id  TEXT PRIMARY KEY NOT NULL,
            app_id    TEXT NOT NULL,
            kept_at   INTEGER NOT NULL,
            kept_by   TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_app_space_unbinds (
            space_id      TEXT PRIMARY KEY NOT NULL,
            app_id        TEXT NOT NULL,
            wanted_at     INTEGER NOT NULL,
            wanted_by     TEXT NOT NULL,
            done_at       INTEGER,
            orphaned_from TEXT,
            cause         TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db app spaces schema: {e}"))
}

pub fn subject_key(space_id: &str, ext: &ExternalId) -> String {
    format!("{SUBJECT_PREFIX}{space_id}:{ext}")
}

/// One App ↔ Space binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub app_id: String,
    pub space_id: String,
    pub bound_at: i64,
    /// Attribution only ("reported by K2").
    pub bound_by: String,
}

/// Ledger counts for one Space.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub added: usize,
    pub held: usize,
    pub pending: usize,
}

/// Check an App id and a Space id before any write.
pub fn check_ids(app_id: &str, space_id: &str) -> Result<(), String> {
    if !sew_wire::is_app_id(app_id) {
        return Err(format!(
            "'{app_id}' is not an App id Sew takes (1–64 of [a-z0-9][a-z0-9._-]*)"
        ));
    }
    if app_id == crate::skin::HOST_APP_ID {
        return Err(
            "the box login ('host') is not an App with its own Space: K2's default Space is not bound"
                .into(),
        );
    }
    if !sew_wire::is_sew_id(space_id) {
        return Err(format!("'{space_id}' is not a Sew Space id"));
    }
    Ok(())
}

/// Record a binding. `Ok(false)` = it was already there (same App). A
/// Space bound to another App is refused (`space_bound`).
pub fn bind(
    conn: &Connection,
    app_id: &str,
    space_id: &str,
    actor: &str,
    now: i64,
) -> Result<bool, String> {
    check_ids(app_id, space_id)?;
    let other: Option<String> = conn
        .query_row(
            "SELECT app_id FROM tool_app_spaces WHERE space_id = ?1",
            params![space_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("app spaces read: {e}"))?;
    match other {
        Some(a) if a == app_id => Ok(false),
        Some(a) => Err(format!("space_bound: Space {space_id} belongs to App '{a}'")),
        None => {
            conn.execute(
                "INSERT INTO tool_app_spaces (app_id, space_id, bound_at, bound_by) VALUES (?1, ?2, ?3, ?4)",
                params![app_id, space_id, now, actor],
            )
            .map_err(|e| format!("app spaces insert: {e}"))?;
            // Bound again: K2 manages its members again.
            conn.execute("DELETE FROM tool_app_space_kept WHERE space_id = ?1", params![space_id])
                .map_err(|e| format!("app spaces kept: {e}"))?;
            // An unbind not sent yet must not orphan the Space it now holds.
            cancel_unbind(conn, space_id, now)?;
            Ok(true)
        }
    }
}

/// Forget a binding. `Ok(false)` = there was none.
/// Forget `app` ↔ `space` (S32 Q3, Rosson 2026-10-27: a choice at unbind
/// time). `remove_members = false` (the default; reversible) leaves every
/// member where they are and K2 stops managing that Space. `true` removes
/// the members K2 added automatically — only those: K2's ledger never holds
/// someone added by hand, so they are never touched.
pub fn unbind(conn: &Connection, app_id: &str, space_id: &str, remove_members: bool, actor: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "DELETE FROM tool_app_spaces WHERE app_id = ?1 AND space_id = ?2",
            params![app_id, space_id],
        )
        .map_err(|e| format!("app spaces delete: {e}"))?;
    if n == 0 {
        return Ok(false);
    }
    if remove_members {
        conn.execute("DELETE FROM tool_app_space_kept WHERE space_id = ?1", params![space_id])
            .map_err(|e| format!("app spaces kept: {e}"))?;
    } else {
        conn.execute(
            "INSERT INTO tool_app_space_kept (space_id, app_id, kept_at, kept_by) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(space_id) DO UPDATE SET app_id = excluded.app_id, kept_at = excluded.kept_at, kept_by = excluded.kept_by",
            params![space_id, app_id, now, actor],
        )
        .map_err(|e| format!("app spaces kept: {e}"))?;
    }
    Ok(true)
}

/// Spaces unbound with their members kept: K2 forgets its ledger there.
fn kept_spaces(conn: &Connection) -> Result<HashSet<String>, String> {
    let mut stmt =
        conn.prepare("SELECT space_id FROM tool_app_space_kept").map_err(|e| format!("app spaces kept: {e}"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("app spaces kept: {e}"))?;
    rows.collect::<Result<HashSet<_>, _>>().map_err(|e| format!("app spaces kept row: {e}"))
}

pub fn bindings(conn: &Connection) -> Result<Vec<Binding>, String> {
    let mut stmt = conn
        .prepare("SELECT app_id, space_id, bound_at, bound_by FROM tool_app_spaces ORDER BY app_id, space_id")
        .map_err(|e| format!("app spaces list: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Binding { app_id: r.get(0)?, space_id: r.get(1)?, bound_at: r.get(2)?, bound_by: r.get(3)? })
        })
        .map_err(|e| format!("app spaces list: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("app spaces row: {e}"))
}

pub fn counts(conn: &Connection, space_id: &str) -> Result<Counts, String> {
    let mut stmt = conn
        .prepare("SELECT state, COUNT(*) FROM tool_app_space_members WHERE space_id = ?1 GROUP BY state")
        .map_err(|e| format!("app space counts: {e}"))?;
    let rows = stmt
        .query_map(params![space_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| format!("app space counts: {e}"))?;
    let mut c = Counts::default();
    for row in rows {
        let (state, n) = row.map_err(|e| format!("app space counts row: {e}"))?;
        let n = n as usize;
        match state.as_str() {
            ADDED => c.added += n,
            HELD => c.held += n,
            _ => c.pending += n,
        }
    }
    Ok(c)
}

/// One person who should be in one App's Space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub app_id: String,
    pub space_id: String,
    pub ext: ExternalId,
    /// The App grant's id (the outbox row's `grant_id`).
    pub app_grant_id: String,
    /// `<app grant rev>|<chat grant rev>`: HC2's "until the grant changes".
    pub rev: String,
}

/// The desired set: every binding × every principal with an enabled
/// `kind=app` grant for that App AND an enabled `chat` member grant.
/// Principals only — never a Connect login, the owner or an agent.
pub fn build_desired(conn: &Connection) -> Result<Vec<Wanted>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT b.app_id, b.space_id, a.subject_id, a.id, a.scope, c.id, c.scope
             FROM tool_app_spaces b
             JOIN grants a ON a.kind = 'app' AND a.target_id = b.app_id
                          AND a.subject_kind = 'principal' AND a.enabled = 1
             JOIN grants c ON c.kind = 'chat' AND c.subject_kind = 'principal'
                          AND c.subject_id = a.subject_id AND c.enabled = 1 AND c.scope = 'member'
             ORDER BY b.space_id, a.subject_id",
        )
        .map_err(|e| format!("app space desired: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|e| format!("app space desired: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (app_id, space_id, principal, app_gid, app_scope, chat_gid, chat_scope) =
            row.map_err(|e| format!("app space desired row: {e}"))?;
        // A principal id that is not a lower uuid can't be a Sew member
        // (S27 refuses it at provision); leave it out rather than fail.
        let Ok(ext) = ExternalId::parse(&format!("k2:principal:{principal}")) else {
            continue;
        };
        out.push(Wanted {
            app_id,
            space_id,
            ext,
            rev: format!(
                "{}:{}:1|{}:{}:1",
                app_gid,
                app_scope.unwrap_or_default(),
                chat_gid,
                chat_scope.unwrap_or_default()
            ),
            app_grant_id: app_gid,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct LedgerRow {
    app_id: String,
    state: String,
    rev: Option<String>,
    why: Option<String>,
}

fn ledger(conn: &Connection) -> Result<HashMap<(String, String), LedgerRow>, String> {
    let mut stmt = conn
        .prepare("SELECT space_id, external_id, app_id, state, grant_rev, why FROM tool_app_space_members")
        .map_err(|e| format!("app space ledger: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                LedgerRow { app_id: r.get(2)?, state: r.get(3)?, rev: r.get(4)?, why: r.get(5)? },
            ))
        })
        .map_err(|e| format!("app space ledger: {e}"))?;
    rows.collect::<Result<HashMap<_, _>, _>>().map_err(|e| format!("app space ledger row: {e}"))
}

fn set_ledger(
    conn: &Connection,
    space_id: &str,
    ext: &str,
    app_id: &str,
    state: &str,
    rev: Option<&str>,
    why: Option<&str>,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_app_space_members (space_id, app_id, external_id, state, grant_rev, why, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(space_id, external_id) DO UPDATE SET
            app_id = excluded.app_id, state = excluded.state,
            grant_rev = COALESCE(excluded.grant_rev, tool_app_space_members.grant_rev),
            why = excluded.why, updated_at = excluded.updated_at",
        params![space_id, app_id, ext, state, rev, why, now],
    )
    .map_err(|e| format!("app space ledger write: {e}"))?;
    Ok(())
}

fn drop_ledger(conn: &Connection, space_id: &str, ext: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM tool_app_space_members WHERE space_id = ?1 AND external_id = ?2",
        params![space_id, ext],
    )
    .map_err(|e| format!("app space ledger delete: {e}"))?;
    Ok(())
}

fn add_op(w: &Wanted, cause: Cause) -> Result<NewOp, String> {
    let body = SpaceMembersAddRequest {
        space_id: w.space_id.clone(),
        app_id: Some(w.app_id.clone()),
        external_ids: vec![w.ext.clone()],
    };
    Ok(NewOp {
        tool: CHAT_KIND,
        op: Op::SpaceMemberAdd,
        subject_key: subject_key(&w.space_id, &w.ext),
        payload: serde_json::to_value(&body).map_err(|e| format!("space add body: {e}"))?,
        grant_id: Some(w.app_grant_id.clone()),
        grant_rev: Some(w.rev.clone()),
        cause,
        // The person's own queue (provision) goes first.
        after_subject: Some(w.ext.to_string()),
    })
}

fn remove_op(space_id: &str, app_id: &str, ext: &ExternalId, cause: Cause) -> Result<NewOp, String> {
    let body = SpaceMemberRemoveRequest {
        space_id: space_id.to_string(),
        app_id: Some(app_id.to_string()),
        external_id: ext.clone(),
    };
    Ok(NewOp {
        tool: CHAT_KIND,
        op: Op::SpaceMemberRemove,
        subject_key: subject_key(space_id, ext),
        payload: serde_json::to_value(&body).map_err(|e| format!("space remove body: {e}"))?,
        grant_id: None,
        grant_rev: None,
        cause,
        after_subject: None,
    })
}

/// What one [`plan`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub desired: usize,
    pub adds: usize,
    pub removes: usize,
    pub held_kept: usize,
    pub held_forgotten: usize,
    /// Members of a Space unbound with its members kept: forgotten, not
    /// removed.
    pub kept: usize,
}

/// Diff the desired set against the ledger, write the ledger changes and
/// return the ops to enqueue. MUST run inside the caller's transaction
/// (the caller enqueues the ops in it). `busy` = subject keys with an open
/// (`pending`/`blocked`) outbox row.
pub fn plan(
    conn: &Connection,
    busy: &HashSet<String>,
    now: i64,
) -> Result<(Vec<NewOp>, Summary), String> {
    let desired = build_desired(conn)?;
    let mut have = ledger(conn)?;
    let mut ops = Vec::new();
    let mut s = Summary { desired: desired.len(), ..Default::default() };
    for w in &desired {
        let ext = w.ext.to_string();
        let key = (w.space_id.clone(), ext.clone());
        let open = busy.contains(&subject_key(&w.space_id, &w.ext));
        let enqueue = match have.remove(&key) {
            None => true,
            Some(l) => match l.state.as_str() {
                ADDED => false,
                // Queued already; re-queue only if its row is gone (an
                // owner skip) so the person is not stuck.
                PENDING_ADD => !open,
                // The remove is queued: the add lands after it.
                PENDING_REMOVE => true,
                // Banned: never again on K2's own (only `clear_banned`).
                HELD if is_banned(l.why.as_deref()) => {
                    s.held_kept += 1;
                    false
                }
                HELD => {
                    let changed = l.rev.as_deref() != Some(w.rev.as_str());
                    if !changed {
                        s.held_kept += 1;
                    }
                    changed
                }
                _ => true,
            },
        };
        if enqueue {
            ops.push(add_op(w, Cause::Reconcile)?);
            set_ledger(conn, &w.space_id, &ext, &w.app_id, PENDING_ADD, Some(&w.rev), None, now)?;
            s.adds += 1;
        }
    }
    // Everything left in the ledger is no longer wanted. In a Space
    // unbound with its members kept, K2 just forgets them (they stay).
    let kept = kept_spaces(conn)?;
    for ((space_id, ext_s), l) in have {
        if kept.contains(&space_id) && !(l.state == HELD && is_banned(l.why.as_deref())) {
            drop_ledger(conn, &space_id, &ext_s)?;
            s.kept += 1;
            continue;
        }
        let Ok(ext) = ExternalId::parse(&ext_s) else {
            drop_ledger(conn, &space_id, &ext_s)?;
            continue;
        };
        let open = busy.contains(&subject_key(&space_id, &ext));
        match l.state.as_str() {
            // A ban outlives the grant: a later grant must not re-add them.
            HELD if is_banned(l.why.as_deref()) => {
                s.held_kept += 1;
            }
            HELD => {
                drop_ledger(conn, &space_id, &ext_s)?;
                s.held_forgotten += 1;
            }
            PENDING_REMOVE if open => {}
            _ => {
                ops.push(remove_op(&space_id, &l.app_id, &ext, Cause::Reconcile)?);
                set_ledger(conn, &space_id, &ext_s, &l.app_id, PENDING_REMOVE, None, None, now)?;
                s.removes += 1;
            }
        }
    }
    Ok((ops, s))
}

/// [`plan`] + enqueue, in one transaction (bind/unbind and tests; the
/// reconcile sweep runs [`plan`] inside its own transaction).
pub fn reconcile(conn: &Connection, actor: &str, now: i64) -> Result<Summary, String> {
    let tx = conn.unchecked_transaction().map_err(|e| format!("app space begin: {e}"))?;
    let busy = outbox::busy_subjects(&tx, CHAT_KIND)?;
    let (ops, s) = plan(&tx, &busy, now)?;
    outbox::enqueue(&tx, &ops, actor, now)?;
    tx.commit().map_err(|e| format!("app space commit: {e}"))?;
    Ok(s)
}

/// How Sew answered one `space_member_add` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddVerdict {
    /// In the Space (`added` or `already`).
    Added,
    /// Not added, and K2 must not retry until the grant changes (HC2).
    Held(String),
    /// The person has no Sew account yet: retry later.
    NotProvisioned,
    /// The answer names neither outcome for this person.
    Unknown(String),
}

/// Read a 2xx `spaces/members/add` answer for the one person the row sent.
pub fn add_verdict(payload_raw: &str, body: &str) -> AddVerdict {
    let sent: Option<SpaceMembersAddRequest> = serde_json::from_str(payload_raw).ok();
    let Some(ext) = sent.and_then(|r| r.external_ids.into_iter().next()) else {
        return AddVerdict::Unknown("the row's payload names no person".into());
    };
    let Ok(resp) = serde_json::from_str::<SpaceMembersAddResponse>(body) else {
        return AddVerdict::Unknown("Sew's answer is not a members/add response".into());
    };
    if resp.added.iter().chain(resp.already.iter()).any(|m| m.external_id == ext) {
        return AddVerdict::Added;
    }
    if let Some(n) = resp.not_added.iter().find(|n| n.external_id == ext) {
        return if n.reason == sew_wire::NOT_ADDED_NOT_PROVISIONED {
            AddVerdict::NotProvisioned
        } else {
            AddVerdict::Held(format!("not added: {}", n.reason))
        };
    }
    if !resp.skipped.is_empty() {
        return AddVerdict::Held(BANNED_WHY.into());
    }
    AddVerdict::Unknown(format!("Sew did not list {ext} in its answer"))
}

fn row_target(row: &outbox::OutboxRow) -> Option<(String, String, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(&row.payload_raw).ok()?;
    let space = v.get("space_id")?.as_str()?.to_string();
    let app = v.get("app_id").and_then(|a| a.as_str()).map(str::to_string);
    let ext = v
        .get("external_id")
        .or_else(|| v.get("external_ids").and_then(|a| a.get(0)))?
        .as_str()?
        .to_string();
    Some((space, ext, app))
}

/// Move a `pending_add` ledger row on (and nothing else: a remove queued
/// after this add has the newer wish).
fn settle_add(
    conn: &Connection,
    space: &str,
    ext: &str,
    state: &str,
    rev: Option<&str>,
    why: Option<&str>,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_app_space_members
            SET state = ?1, grant_rev = COALESCE(?2, grant_rev), why = ?3, updated_at = ?4
          WHERE space_id = ?5 AND external_id = ?6 AND state = 'pending_add'",
        params![state, rev, why, now, space, ext],
    )
    .map_err(|e| format!("app space ledger settle: {e}"))?;
    Ok(())
}

/// The worker delivered row `id` (done): move the ledger on.
pub fn after_done(conn: &Connection, id: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, id)? else { return Ok(()) };
    let Some((space, ext, _app)) = row_target(&row) else { return Ok(()) };
    match Op::parse(&row.op) {
        Some(Op::SpaceMemberAdd) => settle_add(conn, &space, &ext, ADDED, row.grant_rev.as_deref(), None, now),
        Some(Op::SpaceMemberRemove) => {
            // Only forget a person whose latest wish is still "out" (an
            // add queued after this remove keeps its pending_add).
            conn.execute(
                "DELETE FROM tool_app_space_members
                 WHERE space_id = ?1 AND external_id = ?2 AND state = 'pending_remove'",
                params![space, ext],
            )
            .map_err(|e| format!("app space ledger delete: {e}"))?;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Let K2 add a banned person to a Space again (Sew reported the unban).
/// Returns whether there was a banned hold. The next reconcile re-adds them
/// if they still hold the App's grant.
pub fn clear_banned(conn: &Connection, space_id: &str, external_id: &str) -> Result<bool, String> {
    let n = conn
        .execute(
            "DELETE FROM tool_app_space_members WHERE space_id = ?1 AND external_id = ?2 AND state = 'held' AND why = ?3",
            params![space_id, external_id, BANNED_WHY],
        )
        .map_err(|e| format!("skin.db app space members: {e}"))?;
    Ok(n > 0)
}

/// The worker recorded row `id` held (HC2): the ledger keeps the person
/// out until the grant rev changes (banned: until [`clear_banned`]).
pub fn after_held(conn: &Connection, id: &str, why: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, id)? else { return Ok(()) };
    let Some((space, ext, _app)) = row_target(&row) else { return Ok(()) };
    settle_add(conn, &space, &ext, HELD, row.grant_rev.as_deref(), Some(why), now)
}

// ── Sew's side of an unbind (0.5.0, `space_unbind`) ───────────────────

/// Outbox subject prefix of a `space_unbind` row.
pub const UNBIND_PREFIX: &str = "unbind:";

pub fn unbind_subject(space_id: &str) -> String {
    format!("{UNBIND_PREFIX}{space_id}")
}

/// Remember that Sew should unbind `space_id` from `app_id` (sent by
/// [`plan_unbinds`] once Sew reports `space_unbind`). `cause`: the App was
/// removed ([`Cause::AppRemoved`]) or the binding was dropped by hand
/// ([`Cause::SpaceUnbound`]).
pub fn want_unbind(conn: &Connection, app_id: &str, space_id: &str, cause: Cause, actor: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_app_space_unbinds (space_id, app_id, wanted_at, wanted_by, done_at, orphaned_from, cause)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5)
         ON CONFLICT(space_id) DO UPDATE SET app_id = excluded.app_id, wanted_at = excluded.wanted_at,
            wanted_by = excluded.wanted_by, done_at = NULL, orphaned_from = NULL, cause = excluded.cause",
        params![space_id, app_id, now, actor, cause.as_str()],
    )
    .map(|_| ())
    .map_err(|e| format!("app space unbind write: {e}"))
}

/// Drop an unsent unbind of `space_id` (it was bound again), and finish
/// its queued row unsent.
fn cancel_unbind(conn: &Connection, space_id: &str, now: i64) -> Result<(), String> {
    conn.execute("DELETE FROM tool_app_space_unbinds WHERE space_id = ?1 AND done_at IS NULL", params![space_id])
        .map_err(|e| format!("app space unbind cancel: {e}"))?;
    conn.execute(
        "UPDATE tool_outbox SET state = ?1, updated_at = ?2
         WHERE tool = ?3 AND subject_key = ?4 AND op = ?5 AND state IN (?6, ?7)",
        params![
            outbox::STATE_SUPERSEDED,
            now,
            CHAT_KIND,
            unbind_subject(space_id),
            Op::SpaceUnbind.as_str(),
            outbox::STATE_PENDING,
            outbox::STATE_BLOCKED
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("app space unbind cancel: {e}"))
}

/// One unbind K2 asked Sew for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unbind {
    pub space_id: String,
    pub app_id: String,
    pub wanted_at: i64,
    pub wanted_by: String,
    /// When Sew confirmed it (`None` = not sent yet, or Sew lacks
    /// `space_unbind`).
    pub done_at: Option<i64>,
    /// The App Sew records the orphan as coming from.
    pub orphaned_from: Option<String>,
    /// `app_removed` | `space_unbound` (an unbind by hand).
    pub cause: String,
}

pub fn unbinds(conn: &Connection) -> Result<Vec<Unbind>, String> {
    let mut st = conn
        .prepare("SELECT space_id, app_id, wanted_at, wanted_by, done_at, orphaned_from, cause FROM tool_app_space_unbinds ORDER BY space_id")
        .map_err(|e| format!("app space unbinds: {e}"))?;
    let rows = st
        .query_map([], |r| {
            Ok(Unbind {
                space_id: r.get(0)?,
                app_id: r.get(1)?,
                wanted_at: r.get(2)?,
                wanted_by: r.get(3)?,
                done_at: r.get(4)?,
                orphaned_from: r.get(5)?,
                cause: r.get(6)?,
            })
        })
        .map_err(|e| format!("app space unbinds: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("app space unbinds row: {e}"))
}

/// One `space_unbind` op per unsent wish, once Sew reports `space_unbind`,
/// with the actor who asked for it. Skipped while the unbind has an open
/// row, and while any membership row of that Space is open (an unbind
/// with `--remove-app-members` removes them first; Sew 0.5.0 still takes
/// removes on an orphan, but a queued ADD there would answer 422
/// `space_orphaned`, so K2 settles its own rows first).
pub fn plan_unbinds(conn: &Connection, busy: &HashSet<String>) -> Result<Vec<(NewOp, String)>, String> {
    if !super::features::has(conn, CHAT_KIND, super::features::SPACE_UNBIND)? {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for u in unbinds(conn)?.into_iter().filter(|u| u.done_at.is_none()) {
        let subject = unbind_subject(&u.space_id);
        let members = format!("{SUBJECT_PREFIX}{}:", u.space_id);
        if busy.contains(&subject) || busy.iter().any(|k| k.starts_with(&members)) {
            continue;
        }
        let body = sew_wire::SpaceUnbindRequest { space_id: u.space_id.clone(), app_id: u.app_id.clone() };
        out.push((
            NewOp {
                tool: CHAT_KIND,
                op: Op::SpaceUnbind,
                subject_key: subject,
                payload: serde_json::to_value(&body).map_err(|e| format!("space unbind body: {e}"))?,
                grant_id: None,
                grant_rev: None,
                cause: if u.cause == Cause::AppRemoved.as_str() { Cause::AppRemoved } else { Cause::SpaceUnbound },
                after_subject: None,
            },
            u.wanted_by,
        ));
    }
    Ok(out)
}

/// Queue every unbind [`plan_unbinds`] allows now, each under the actor
/// who asked for it. MUST run inside the caller's transaction. Returns how
/// many went in.
pub fn queue_unbinds(conn: &Connection, now: i64) -> Result<usize, String> {
    let busy = outbox::busy_subjects(conn, CHAT_KIND)?;
    let mut n = 0;
    for (op, actor) in plan_unbinds(conn, &busy)? {
        n += outbox::enqueue(conn, std::slice::from_ref(&op), &actor, now)?.len();
    }
    Ok(n)
}

/// An App was removed from the box: forget each of its bindings (members
/// stay; K2 stops managing them) and want Sew's unbind for each Space.
/// MUST run inside the caller's transaction. Returns the Spaces.
pub fn app_removed(conn: &Connection, app_id: &str, actor: &str, now: i64) -> Result<Vec<String>, String> {
    let spaces: Vec<String> = bindings(conn)?.into_iter().filter(|b| b.app_id == app_id).map(|b| b.space_id).collect();
    for space in &spaces {
        unbind(conn, app_id, space, false, actor, now)?;
        want_unbind(conn, app_id, space, Cause::AppRemoved, actor, now)?;
    }
    Ok(spaces)
}

/// A delivered `space_unbind`: Sew holds the orphan now (a 404, no such
/// Space, is done too: its answer names no orphan).
pub fn after_done_unbind(conn: &Connection, row_id: &str, result: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, row_id)? else { return Ok(()) };
    if Op::parse(&row.op) != Some(Op::SpaceUnbind) {
        return Ok(());
    }
    let Some(space) = row.subject_key.strip_prefix(UNBIND_PREFIX) else { return Ok(()) };
    let orphaned_from = serde_json::from_str::<sew_wire::SpaceUnbindResponse>(result).ok().and_then(|r| r.orphaned_from);
    conn.execute(
        "UPDATE tool_app_space_unbinds SET done_at = ?2, orphaned_from = ?3 WHERE space_id = ?1",
        params![space, now, orphaned_from],
    )
    .map(|_| ())
    .map_err(|e| format!("app space unbind done: {e}"))
}

/// A new Sew install has none of the old Spaces: nothing to unbind there.
pub fn forget_unbinds(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM tool_app_space_unbinds", []).map(|_| ()).map_err(|e| format!("app space unbinds reset: {e}"))
}

// ── direct adds (Sew A9: `chat.createChannel`) ───────────────────────

/// The App `space_id` is bound to here (`None`: not bound).
pub fn bound_app(conn: &Connection, space_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT app_id FROM tool_app_spaces WHERE space_id = ?1", params![space_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("app spaces read: {e}"))
}

/// What the ledger holds for one person in one Space: `(state, banned)`.
pub fn ledger_state(conn: &Connection, space_id: &str, ext: &str) -> Result<Option<(String, bool)>, String> {
    conn.query_row(
        "SELECT state, why FROM tool_app_space_members WHERE space_id = ?1 AND external_id = ?2",
        params![space_id, ext],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .optional()
    .map(|o| o.map(|(state, why)| { let banned = state == HELD && is_banned(why.as_deref()); (state, banned) }))
    .map_err(|e| format!("app space ledger: {e}"))
}

/// A wanted person K2 added to their App's Space directly (a channel
/// create adds its members to the Space first, outside the outbox). Sew
/// answered them `added`/`already` (`in_space`) or `skipped` (banned: held
/// for good, as the worker would). A queued outbox add for them later
/// answers `already` (or `skipped`) and leaves this row as it is.
pub fn note_direct_add(conn: &Connection, w: &Wanted, in_space: bool, now: i64) -> Result<(), String> {
    if in_space {
        set_ledger(conn, &w.space_id, &w.ext.to_string(), &w.app_id, ADDED, Some(&w.rev), None, now)
    } else {
        set_ledger(conn, &w.space_id, &w.ext.to_string(), &w.app_id, HELD, Some(&w.rev), Some(BANNED_WHY), now)
    }
}

#[cfg(test)]
#[path = "app_space_tests.rs"]
mod tests;
