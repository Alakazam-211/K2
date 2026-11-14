//! Home channels in the K5 Space: K2's ledger and the mgmt ops that make
//! and change them (prd-home-people-channels-v1 §7, slice S1; HP-3, HP-19,
//! HP-25).
//!
//! **Why K2 holds a ledger.** The K5 Space is server-governed: only K2,
//! over mgmt, makes channels and sets who is in them, and mgmt acts as the
//! service, so Sew can't check the person (HP-19). K2 checks the caller
//! against this ledger before it queues anything (the routes), and the
//! ledger is what the level sync sends.
//!
//! **The level sync keeps the list (HP-3).** Sew's member-channels sync
//! sets a person's level AND their whole private-channel list; `[]` removes
//! them from every private channel in the Space. So every level change
//! carries the person's ledger list ([`level_body`]), and once Sew reports
//! `member_channels_level_only` it carries no list at all. K2 never sends a
//! hard-coded `[]`: an empty list goes only for a person the ledger holds
//! in no private channel, which is what Sew holds too (only K2 changes K5
//! membership).
//!
//! **Adding and removing people** (§7.3). With Sew's
//! `channel_member_add_remove` flag: one mgmt add or remove per change.
//! Without it: the full-list sync, one row per person on their own queue
//! (`admin:m:<space>:<ext>`, shared with their level syncs), so two changes
//! for one person never interleave and each body is the whole current list.
//! Open channels need no mgmt add: people join on open (HP-27).
//!
//! **Slugs** are K2's, random ([`crate::home_chat::random_slug`]); mgmt's
//! create is idempotent on the slug (HP-25).
//!
//! **Delivery.** The S27 outbox, like the rooms: a channel's create and its
//! rename/archive ride `admin:chan:<slug>`; member changes ride the
//! person's member queue.
//!
//! Tables live in `skin.db` (idempotent DDL), so this needs no main-DB
//! migration.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};

use super::admin_space::{member_subject, parse_member_subject};
use super::features::HomeChatFeatures;
use super::sew_wire::{
    self, channel_ops, ChannelCreateResponse, ChannelMemberChangeRequest, ChannelOp, ChannelsBulkResponse,
    ServerChannelCreateRequest, ServerChannelsBulkRequest, ServerMemberChannelsRequest,
};
use super::{outbox, Cause, NewOp, Op, CHAT_KIND};

/// One Home channel's queue: `admin:chan:<slug>`.
pub const CHANNEL_PREFIX: &str = "admin:chan:";
/// Attribution of Home chat's own writes.
pub const ACTOR: &str = "system:home-chat";

/// Channel states.
pub const CH_PENDING: &str = "pending";
pub const CH_ACTIVE: &str = "active";
pub const CH_ARCHIVED: &str = "archived";
/// Channel member states.
pub const M_PENDING_ADD: &str = "pending_add";
pub const M_IN: &str = "in";
pub const M_PENDING_REMOVE: &str = "pending_remove";

pub fn channel_subject(slug: &str) -> String {
    format!("{CHANNEL_PREFIX}{slug}")
}

/// Idempotent DDL, called from the Admin Space's own schema step.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_admin_chat_channels (
            slug          TEXT PRIMARY KEY NOT NULL,
            channel_id    TEXT UNIQUE,
            name          TEXT NOT NULL,
            want_name     TEXT,
            open          INTEGER NOT NULL,
            creator_ext   TEXT NOT NULL,
            allow_agents  INTEGER NOT NULL DEFAULT 0,
            want_archive  INTEGER NOT NULL DEFAULT 0,
            can_leave     INTEGER NOT NULL DEFAULT 0,
            state         TEXT NOT NULL CHECK (state IN ('pending','active','archived')),
            why           TEXT,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_admin_chat_members (
            slug          TEXT NOT NULL,
            external_id   TEXT NOT NULL,
            state         TEXT NOT NULL CHECK (state IN ('pending_add','in','pending_remove')),
            why           TEXT,
            updated_at    INTEGER NOT NULL,
            PRIMARY KEY (slug, external_id)
         );
         CREATE TABLE IF NOT EXISTS tool_admin_member_channels (
            external_id       TEXT PRIMARY KEY NOT NULL,
            channel_ids_want  TEXT NOT NULL DEFAULT '[]',
            channel_ids_sent  TEXT,
            state             TEXT NOT NULL DEFAULT 'synced',
            why               TEXT,
            updated_at        INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_home_chat_dms (
            person_ext    TEXT NOT NULL,
            target_ext    TEXT NOT NULL,
            channel_id    TEXT NOT NULL,
            updated_at    INTEGER NOT NULL,
            PRIMARY KEY (person_ext, target_ext)
         );",
    )
    .map_err(|e| format!("skin.db home channels schema: {e}"))?;
    // `can_leave` on a table an earlier build of this branch made.
    let has: bool = conn
        .query_row("SELECT COUNT(*) FROM pragma_table_info('tool_admin_chat_channels') WHERE name = 'can_leave'", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("home channels columns: {e}"))?
        > 0;
    if !has {
        conn.execute_batch("ALTER TABLE tool_admin_chat_channels ADD COLUMN can_leave INTEGER NOT NULL DEFAULT 0")
            .map_err(|e| format!("home channels can_leave: {e}"))?;
    }
    Ok(())
}

// ── reads ────────────────────────────────────────────────────────────

/// One Home channel as K2 holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRow {
    pub slug: String,
    pub channel_id: Option<String>,
    pub name: String,
    pub want_name: Option<String>,
    pub open: bool,
    pub creator_ext: String,
    pub allow_agents: bool,
    pub want_archive: bool,
    /// Sew holds `members_can_leave: true` for it (set at create or by
    /// `update` once Sew has the flag).
    pub can_leave: bool,
    pub state: String,
    pub why: Option<String>,
}

impl ChannelRow {
    /// The name the channel should carry.
    pub fn desired_name(&self) -> &str {
        self.want_name.as_deref().unwrap_or(&self.name)
    }
}

const CH_SELECT: &str =
    "SELECT slug, channel_id, name, want_name, open, creator_ext, allow_agents, want_archive, state, why, can_leave FROM tool_admin_chat_channels";

fn ch_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChannelRow> {
    Ok(ChannelRow {
        slug: r.get(0)?,
        channel_id: r.get(1)?,
        name: r.get(2)?,
        want_name: r.get(3)?,
        open: r.get::<_, i64>(4)? != 0,
        creator_ext: r.get(5)?,
        allow_agents: r.get::<_, i64>(6)? != 0,
        want_archive: r.get::<_, i64>(7)? != 0,
        state: r.get(8)?,
        why: r.get(9)?,
        can_leave: r.get::<_, i64>(10)? != 0,
    })
}

pub fn channels(conn: &Connection) -> Result<Vec<ChannelRow>, String> {
    let mut s = conn.prepare(&format!("{CH_SELECT} ORDER BY created_at, slug")).map_err(|e| format!("home channels: {e}"))?;
    let rows = s.query_map([], ch_row).map_err(|e| format!("home channels: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("home channel row: {e}"))
}

pub fn channel_by_slug(conn: &Connection, slug: &str) -> Result<Option<ChannelRow>, String> {
    conn.query_row(&format!("{CH_SELECT} WHERE slug = ?1"), params![slug], ch_row)
        .optional()
        .map_err(|e| format!("home channel: {e}"))
}

pub fn channel_by_id(conn: &Connection, channel_id: &str) -> Result<Option<ChannelRow>, String> {
    conn.query_row(&format!("{CH_SELECT} WHERE channel_id = ?1"), params![channel_id], ch_row)
        .optional()
        .map_err(|e| format!("home channel: {e}"))
}

/// One person's place in one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMember {
    pub external_id: String,
    pub state: String,
    pub why: Option<String>,
}

pub fn members_of(conn: &Connection, slug: &str) -> Result<Vec<ChannelMember>, String> {
    let mut s = conn
        .prepare("SELECT external_id, state, why FROM tool_admin_chat_members WHERE slug = ?1 ORDER BY external_id")
        .map_err(|e| format!("home channel members: {e}"))?;
    let rows = s
        .query_map(params![slug], |r| Ok(ChannelMember { external_id: r.get(0)?, state: r.get(1)?, why: r.get(2)? }))
        .map_err(|e| format!("home channel members: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("home channel member row: {e}"))
}

/// Is `ext` in the channel (or on the way in)?
pub fn is_member(conn: &Connection, slug: &str, ext: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tool_admin_chat_members WHERE slug = ?1 AND external_id = ?2 AND state IN ('in','pending_add'))",
        params![slug, ext],
        |r| r.get(0),
    )
    .map_err(|e| format!("home channel member check: {e}"))
}

/// The person's WHOLE private-channel list as K2 wants it: every active
/// invite-only Home channel they are in or on the way into, sorted.
pub fn want_list(conn: &Connection, ext: &str) -> Result<Vec<String>, String> {
    let mut s = conn
        .prepare(
            "SELECT c.channel_id FROM tool_admin_chat_members m JOIN tool_admin_chat_channels c ON c.slug = m.slug
             WHERE m.external_id = ?1 AND m.state IN ('in','pending_add') AND c.open = 0
               AND c.state = 'active' AND c.channel_id IS NOT NULL
             ORDER BY c.channel_id",
        )
        .map_err(|e| format!("home want list: {e}"))?;
    let rows = s.query_map(params![ext], |r| r.get::<_, String>(0)).map_err(|e| format!("home want list: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("home want list row: {e}"))
}

/// The list Sew last confirmed for `ext` (`None`: never synced; Sew holds
/// no private K5 channel K2 didn't make, so that reads as empty).
pub fn sent_list(conn: &Connection, ext: &str) -> Result<Option<Vec<String>>, String> {
    let raw: Option<Option<String>> = conn
        .query_row("SELECT channel_ids_sent FROM tool_admin_member_channels WHERE external_id = ?1", params![ext], |r| r.get(0))
        .optional()
        .map_err(|e| format!("home sent list: {e}"))?;
    Ok(raw.flatten().and_then(|t| serde_json::from_str(&t).ok()))
}

fn set_sent(conn: &Connection, ext: &str, list: &[String], now: i64) -> Result<(), String> {
    let mut l: Vec<String> = list.to_vec();
    l.sort();
    l.dedup();
    let raw = serde_json::to_string(&l).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO tool_admin_member_channels (external_id, channel_ids_want, channel_ids_sent, state, updated_at)
         VALUES (?1, ?2, ?2, 'synced', ?3)
         ON CONFLICT(external_id) DO UPDATE SET channel_ids_sent = excluded.channel_ids_sent, state = 'synced', why = NULL,
            updated_at = excluded.updated_at",
        params![ext, raw, now],
    )
    .map(|_| ())
    .map_err(|e| format!("home sent list write: {e}"))
}

fn note_want(conn: &Connection, ext: &str, list: &[String], now: i64) -> Result<(), String> {
    let raw = serde_json::to_string(list).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO tool_admin_member_channels (external_id, channel_ids_want, state, updated_at) VALUES (?1, ?2, 'queued', ?3)
         ON CONFLICT(external_id) DO UPDATE SET channel_ids_want = excluded.channel_ids_want, state = 'queued',
            updated_at = excluded.updated_at",
        params![ext, raw, now],
    )
    .map(|_| ())
    .map_err(|e| format!("home want list write: {e}"))
}

/// The level sync's body for `ext` at `level` (HP-3): the whole ledger list,
/// or, once Sew reports `member_channels_level_only`, no list at all.
pub fn level_body(conn: &Connection, flags: &HomeChatFeatures, ext: &str, level: &str) -> Result<ServerMemberChannelsRequest, String> {
    let channel_ids = if flags.member_channels_level_only { None } else { Some(want_list(conn, ext)?) };
    Ok(ServerMemberChannelsRequest { channel_ids, level: level.to_string() })
}

// ── requests (the routes check the caller first) ─────────────────────

/// A new channel, `pending` until Sew answers: a fresh random slug, the
/// creator and `members` on their way in. Returns its row.
pub fn request_create(conn: &Connection, name: &str, open: bool, creator_ext: &str, members: &[String], now: i64) -> Result<ChannelRow, String> {
    let name = crate::home_chat::valid_channel_name(name)?;
    let mut people: BTreeSet<String> = members.iter().map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
    people.insert(creator_ext.to_string());
    if people.len() > sew_wire::CHANNEL_MEMBERS_MAX {
        return Err(format!("a new channel takes at most {} people; add the rest after", sew_wire::CHANNEL_MEMBERS_MAX));
    }
    let mut slug = None;
    for _ in 0..4 {
        let s = crate::home_chat::random_slug();
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO tool_admin_chat_channels (slug, name, open, creator_ext, state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)",
                params![s, name, open as i64, creator_ext, now],
            )
            .map_err(|e| format!("home channel create: {e}"))?;
        if n == 1 {
            slug = Some(s);
            break;
        }
    }
    let slug = slug.ok_or("K2 could not pick a free channel slug; try again")?;
    for p in &people {
        put_member(conn, &slug, p, M_PENDING_ADD, now)?;
    }
    channel_by_slug(conn, &slug)?.ok_or_else(|| "home channel create: row vanished".into())
}

fn put_member(conn: &Connection, slug: &str, ext: &str, state: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_admin_chat_members (slug, external_id, state, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(slug, external_id) DO UPDATE SET state = excluded.state, why = NULL, updated_at = excluded.updated_at",
        params![slug, ext, state, now],
    )
    .map(|_| ())
    .map_err(|e| format!("home channel member write: {e}"))
}

/// Add `ext` (`added`), or say it is there (`already`).
pub fn request_add(conn: &Connection, slug: &str, ext: &str, now: i64) -> Result<&'static str, String> {
    let row: Option<String> = conn
        .query_row("SELECT state FROM tool_admin_chat_members WHERE slug = ?1 AND external_id = ?2", params![slug, ext], |r| r.get(0))
        .optional()
        .map_err(|e| format!("home channel member read: {e}"))?;
    match row.as_deref() {
        Some(M_IN) | Some(M_PENDING_ADD) => Ok("already"),
        Some(M_PENDING_REMOVE) => {
            // Still in Sew's list (the remove hasn't gone): keep them.
            put_member(conn, slug, ext, M_IN, now)?;
            Ok("added")
        }
        _ => {
            put_member(conn, slug, ext, M_PENDING_ADD, now)?;
            Ok("added")
        }
    }
}

/// Take `ext` out (leave, or a removal). Returns whether they were in.
pub fn request_remove(conn: &Connection, slug: &str, ext: &str, now: i64) -> Result<bool, String> {
    let row: Option<String> = conn
        .query_row("SELECT state FROM tool_admin_chat_members WHERE slug = ?1 AND external_id = ?2", params![slug, ext], |r| r.get(0))
        .optional()
        .map_err(|e| format!("home channel member read: {e}"))?;
    match row.as_deref() {
        None | Some(M_PENDING_REMOVE) => Ok(false),
        Some(_) => {
            conn.execute(
                "UPDATE tool_admin_chat_members SET state = 'pending_remove', why = NULL, updated_at = ?3 WHERE slug = ?1 AND external_id = ?2",
                params![slug, ext, now],
            )
            .map_err(|e| format!("home channel member remove: {e}"))?;
            // §12: K2 archives a channel whose last person leaves.
            let left: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tool_admin_chat_members WHERE slug = ?1 AND state IN ('in','pending_add')",
                    params![slug],
                    |r| r.get(0),
                )
                .map_err(|e| format!("home channel member count: {e}"))?;
            if left == 0 {
                request_archive(conn, slug, now)?;
            }
            Ok(true)
        }
    }
}

pub fn request_rename(conn: &Connection, slug: &str, name: &str, now: i64) -> Result<(), String> {
    let name = crate::home_chat::valid_channel_name(name)?;
    conn.execute("UPDATE tool_admin_chat_channels SET want_name = ?2, updated_at = ?3 WHERE slug = ?1", params![slug, name, now])
        .map(|_| ())
        .map_err(|e| format!("home channel rename: {e}"))
}

/// Delete = archive (history stays in Sew).
pub fn request_archive(conn: &Connection, slug: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_admin_chat_channels SET want_archive = 1,
            state = CASE WHEN state = 'pending' AND channel_id IS NULL THEN 'archived' ELSE state END, updated_at = ?2
         WHERE slug = ?1",
        params![slug, now],
    )
    .map(|_| ())
    .map_err(|e| format!("home channel archive: {e}"))
}

pub fn set_allow_agents(conn: &Connection, slug: &str, on: bool, now: i64) -> Result<(), String> {
    conn.execute("UPDATE tool_admin_chat_channels SET allow_agents = ?2, updated_at = ?3 WHERE slug = ?1", params![slug, on as i64, now])
        .map(|_| ())
        .map_err(|e| format!("home channel allow agents: {e}"))
}

/// Someone joined an open channel themselves (HP-27): they are in.
pub fn note_joined(conn: &Connection, slug: &str, ext: &str, now: i64) -> Result<(), String> {
    put_member(conn, slug, ext, M_IN, now)
}

/// Someone left an open channel through Sew's `/leave`: they are out.
pub fn note_left(conn: &Connection, slug: &str, ext: &str) -> Result<(), String> {
    conn.execute("DELETE FROM tool_admin_chat_members WHERE slug = ?1 AND external_id = ?2", params![slug, ext])
        .map(|_| ())
        .map_err(|e| format!("home channel member left: {e}"))
}

// ── DMs (HP-29) ──────────────────────────────────────────────────────

/// The DM channel `person` opened with `target`, if K2 kept it.
pub fn dm_channel(conn: &Connection, person: &str, target: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT channel_id FROM tool_home_chat_dms WHERE person_ext = ?1 AND target_ext = ?2", params![person, target], |r| r.get(0))
        .optional()
        .map_err(|e| format!("home dm read: {e}"))
}

/// Keep the DM channel Sew's `POST /api/v1/dms` answered.
pub fn set_dm_channel(conn: &Connection, person: &str, target: &str, channel_id: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_home_chat_dms (person_ext, target_ext, channel_id, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(person_ext, target_ext) DO UPDATE SET channel_id = excluded.channel_id, updated_at = excluded.updated_at",
        params![person, target, channel_id, now],
    )
    .map(|_| ())
    .map_err(|e| format!("home dm write: {e}"))
}

/// The other person of a DM channel `person` keeps (`None`: not one of theirs).
pub fn dm_target(conn: &Connection, person: &str, channel_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT target_ext FROM tool_home_chat_dms WHERE person_ext = ?1 AND channel_id = ?2", params![person, channel_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("home dm read: {e}"))
}

/// The other person of DM channel `channel_id` as `me` sees it, whichever of
/// the two opened it through K2 (`None`: K2 never kept it). The summary names
/// a DM's person by it, so a person row finds its badge without opening the DM.
pub fn dm_other(conn: &Connection, me: &str, channel_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT target_ext FROM tool_home_chat_dms WHERE person_ext = ?1 AND channel_id = ?2
         UNION ALL
         SELECT person_ext FROM tool_home_chat_dms WHERE target_ext = ?1 AND channel_id = ?2
         LIMIT 1",
        params![me, channel_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| format!("home dm read: {e}"))
}

// ── planning ─────────────────────────────────────────────────────────

/// What one plan queued (for the sweep's log).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub creates: usize,
    pub channel_ops: usize,
    pub member_adds: usize,
    pub member_removes: usize,
    pub full_lists: usize,
}

fn new_op(o: Op, subject: String, payload: Value) -> NewOp {
    NewOp {
        tool: CHAT_KIND,
        op: o,
        subject_key: subject,
        payload,
        grant_id: None,
        grant_rev: None,
        cause: Cause::VoiceRooms,
        after_subject: None,
    }
}

fn to_value<T: Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Plan the Home channels' ops (after the Space's members, inside the same
/// transaction). `levels` is each Space member's wanted level; a person not
/// in it (not in the Space yet) waits. `out` already holds this sweep's
/// Space ops: a member whose level sync is in it is not planned again (that
/// sync carries the ledger list already).
pub fn plan(
    conn: &Connection,
    space: &str,
    flags: &HomeChatFeatures,
    levels: &BTreeMap<String, &'static str>,
    busy: &HashSet<String>,
    now: i64,
    out: &mut Vec<NewOp>,
) -> Result<Summary, String> {
    let mut s = Summary::default();
    let all = channels(conn)?;
    for c in &all {
        let subject = channel_subject(&c.slug);
        if busy.contains(&subject) {
            continue;
        }
        if c.state == CH_PENDING && c.channel_id.is_none() && !c.want_archive {
            let members: Vec<String> = members_of(conn, &c.slug)?
                .into_iter()
                .filter(|m| m.state == M_PENDING_ADD)
                .map(|m| m.external_id)
                .collect();
            let body = ServerChannelCreateRequest {
                space_id: space.to_string(),
                slug: c.slug.clone(),
                name: c.name.clone(),
                private: !c.open,
                kind: None,
                members,
                members_can_leave: flags.channel_members_can_leave.then_some(true),
            };
            out.push(new_op(Op::AdminChannelCreate, subject, to_value(&body)?));
            s.creates += 1;
            continue;
        }
        let (Some(id), CH_ACTIVE) = (c.channel_id.as_deref(), c.state.as_str()) else { continue };
        let mut ops = Vec::new();
        if c.want_archive {
            ops.push(ChannelOp { op: channel_ops::ARCHIVE.into(), slug: Some(c.slug.clone()), channel_id: Some(id.into()), name: None, kind: None, private: None, user_limit: None, members_can_leave: None });
        } else if c.want_name.as_deref().is_some_and(|w| w != c.name) {
            ops.push(ChannelOp {
                op: channel_ops::RENAME.into(),
                slug: Some(c.slug.clone()),
                channel_id: Some(id.into()),
                name: c.want_name.clone(),
                kind: None,
                private: None,
                user_limit: None,
                members_can_leave: None,
            });
        }
        // HOME1: a channel made before Sew had the flag gets it once.
        if flags.channel_members_can_leave && !c.can_leave && !c.want_archive {
            ops.push(ChannelOp {
                op: channel_ops::UPDATE.into(),
                slug: Some(c.slug.clone()),
                channel_id: Some(id.into()),
                name: None,
                kind: None,
                private: None,
                user_limit: None,
                members_can_leave: Some(true),
            });
        }
        if !ops.is_empty() {
            s.channel_ops += ops.len();
            let body = ServerChannelsBulkRequest { space_id: space.to_string(), ops };
            out.push(new_op(Op::AdminChannelOps, subject, to_value(&body)?));
        }
    }
    plan_members(conn, space, flags, levels, busy, now, &all, out, &mut s)?;
    Ok(s)
}

#[allow(clippy::too_many_arguments)]
fn plan_members(
    conn: &Connection,
    space: &str,
    flags: &HomeChatFeatures,
    levels: &BTreeMap<String, &'static str>,
    busy: &HashSet<String>,
    now: i64,
    all: &[ChannelRow],
    out: &mut Vec<NewOp>,
    s: &mut Summary,
) -> Result<(), String> {
    let by_slug: BTreeMap<&str, &ChannelRow> = all.iter().map(|c| (c.slug.as_str(), c)).collect();
    // Who has a change waiting in a live channel.
    let mut changes: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    {
        let mut st = conn
            .prepare("SELECT slug, external_id, state FROM tool_admin_chat_members WHERE state IN ('pending_add','pending_remove') ORDER BY external_id, slug")
            .map_err(|e| format!("home member changes: {e}"))?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .map_err(|e| format!("home member changes: {e}"))?;
        for r in rows {
            let (slug, ext, state) = r.map_err(|e| format!("home member change row: {e}"))?;
            changes.entry(ext).or_default().push((slug, state));
        }
    }
    let planned: HashSet<String> = out.iter().map(|o| o.subject_key.clone()).collect();
    for (ext, list) in changes {
        let mut full_list = false;
        for (slug, state) in &list {
            let Some(c) = by_slug.get(slug.as_str()) else { continue };
            if c.state == CH_ARCHIVED {
                // Nothing to change in an archived channel: drop the change.
                if state == M_PENDING_REMOVE {
                    note_left(conn, slug, &ext)?;
                }
                continue;
            }
            let (Some(id), CH_ACTIVE) = (c.channel_id.as_deref(), c.state.as_str()) else { continue };
            if c.open && !flags.channel_member_add_remove {
                // Open channels: people join on open (HP-27); a leave is
                // their own `/leave`. Nothing goes over mgmt.
                if state == M_PENDING_ADD {
                    put_member(conn, slug, &ext, M_IN, now)?;
                } else {
                    note_left(conn, slug, &ext)?;
                }
                continue;
            }
            if flags.channel_member_add_remove {
                let subject = member_subject(space, &ext);
                if busy.contains(&subject) || !levels.contains_key(&ext) {
                    continue;
                }
                let o = if state == M_PENDING_ADD {
                    s.member_adds += 1;
                    Op::AdminChannelMemberAdd
                } else {
                    s.member_removes += 1;
                    Op::AdminChannelMemberRemove
                };
                // HOME1: the same body both ways; no `app_id` in K2's server Space.
                let body = to_value(&ChannelMemberChangeRequest { app_id: None, external_id: ext.clone() })?;
                out.push(new_op(o, subject, json!({ "channel_id": id, "body": body })));
            } else {
                full_list = true;
            }
        }
        if full_list {
            let subject = member_subject(space, &ext);
            let Some(level) = levels.get(&ext) else { continue };
            if busy.contains(&subject) || planned.contains(&subject) {
                continue;
            }
            let body = level_body(conn, flags, &ext, level)?;
            if let Some(l) = &body.channel_ids {
                note_want(conn, &ext, l, now)?;
            }
            out.push(new_op(Op::AdminMemberLevel, subject, to_value(&body)?));
            s.full_lists += 1;
        }
    }
    Ok(())
}

// ── after delivery ───────────────────────────────────────────────────

/// The mgmt path and body a Home channel row goes out as (`None`: not one
/// of these rows). Member rows carry `{channel_id, body}`: Sew gets `body`.
pub fn wire(op: Op, payload: &str) -> Option<Result<(String, Vec<u8>), String>> {
    match op {
        Op::AdminChannelCreate => Some(Ok((sew_wire::MGMT_CHANNELS_CREATE.to_string(), payload.as_bytes().to_vec()))),
        Op::AdminChannelOps => Some(sew_wire::channels_bulk_wire(payload).map(|b| (sew_wire::MGMT_CHANNELS_BULK.to_string(), b))),
        Op::AdminChannelMemberAdd | Op::AdminChannelMemberRemove => Some((|| {
            let v: Value = serde_json::from_str(payload).map_err(|e| format!("home member row: {e}"))?;
            let id = v["channel_id"].as_str().filter(|i| sew_wire::is_sew_id(i)).ok_or("home member row names no channel")?;
            let path = if op == Op::AdminChannelMemberAdd {
                sew_wire::channel_members_add_path(id)
            } else {
                sew_wire::channel_members_remove_path(id)
            };
            Ok((path, serde_json::to_vec(&v["body"]).map_err(|e| e.to_string())?))
        })()),
        _ => None,
    }
}

/// A delivered Home channel row moves the ledger on (inside
/// [`super::record_done`]'s connection). The level sync's own Space
/// bookkeeping stays in [`super::admin_space::after_done`]; here it only
/// records the list Sew now holds.
pub fn after_done(conn: &Connection, id: &str, result: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, id)? else { return Ok(()) };
    match Op::parse(&row.op) {
        Some(Op::AdminChannelCreate) => {
            let Some(slug) = row.subject_key.strip_prefix(CHANNEL_PREFIX) else { return Ok(()) };
            let Ok(a) = serde_json::from_str::<ChannelCreateResponse>(result) else {
                return note_channel_why(conn, slug, "Sew's create answer could not be read", now);
            };
            let can_leave = serde_json::from_str::<Value>(&row.payload_raw).is_ok_and(|p| p["members_can_leave"] == json!(true));
            conn.execute(
                "UPDATE tool_admin_chat_channels SET channel_id = ?2, name = ?3, state = 'active', why = NULL, updated_at = ?4,
                    can_leave = CASE WHEN ?5 THEN 1 ELSE can_leave END WHERE slug = ?1",
                params![slug, a.channel_id, a.name, now, can_leave],
            )
            .map_err(|e| format!("home channel created: {e}"))?;
            let open = channel_by_slug(conn, slug)?.is_some_and(|c| c.open);
            for m in a.added.iter().chain(a.already.iter()) {
                let ext = m.external_id.to_string();
                conn.execute(
                    "UPDATE tool_admin_chat_members SET state = 'in', why = NULL, updated_at = ?3 WHERE slug = ?1 AND external_id = ?2 AND state = 'pending_add'",
                    params![slug, ext, now],
                )
                .map_err(|e| format!("home channel member in: {e}"))?;
                if !open {
                    let mut l = sent_list(conn, &ext)?.unwrap_or_default();
                    l.push(a.channel_id.clone());
                    set_sent(conn, &ext, &l, now)?;
                }
            }
            for n in &a.not_added {
                conn.execute(
                    "UPDATE tool_admin_chat_members SET why = ?3, updated_at = ?4 WHERE slug = ?1 AND external_id = ?2",
                    params![slug, n.member, format!("not added at create: {}", n.reason), now],
                )
                .map_err(|e| format!("home channel member why: {e}"))?;
            }
            Ok(())
        }
        Some(Op::AdminChannelOps) => {
            let sent: ServerChannelsBulkRequest = serde_json::from_str(&row.payload_raw).map_err(|e| format!("home channel ops payload: {e}"))?;
            let resp: ChannelsBulkResponse = serde_json::from_str(result).unwrap_or_default();
            for (i, o) in sent.ops.iter().enumerate() {
                let Some(slug) = o.slug.as_deref() else { continue };
                if let Some(e) = resp.results.get(i).and_then(|r| r.error.as_ref()) {
                    note_channel_why(conn, slug, &format!("{} refused: {} ({})", o.op, e.reason(), e.hint), now)?;
                    continue;
                }
                match o.op.as_str() {
                    channel_ops::RENAME => conn
                        .execute(
                            "UPDATE tool_admin_chat_channels SET name = COALESCE(?2, name), want_name = NULL, why = NULL, updated_at = ?3 WHERE slug = ?1",
                            params![slug, o.name, now],
                        )
                        .map(|_| ())
                        .map_err(|e| format!("home channel renamed: {e}"))?,
                    channel_ops::ARCHIVE => conn
                        .execute("UPDATE tool_admin_chat_channels SET state = 'archived', why = NULL, updated_at = ?2 WHERE slug = ?1", params![slug, now])
                        .map(|_| ())
                        .map_err(|e| format!("home channel archived: {e}"))?,
                    channel_ops::UPDATE if o.members_can_leave == Some(true) => conn
                        .execute("UPDATE tool_admin_chat_channels SET can_leave = 1, why = NULL, updated_at = ?2 WHERE slug = ?1", params![slug, now])
                        .map(|_| ())
                        .map_err(|e| format!("home channel can leave: {e}"))?,
                    _ => {}
                }
            }
            Ok(())
        }
        Some(o @ (Op::AdminChannelMemberAdd | Op::AdminChannelMemberRemove)) => {
            let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(()) };
            let ext = ext.to_string();
            let v: Value = serde_json::from_str(&row.payload_raw).map_err(|e| format!("home member row: {e}"))?;
            let Some(cid) = v["channel_id"].as_str() else { return Ok(()) };
            let Some(c) = channel_by_id(conn, cid)? else { return Ok(()) };
            let mut l = sent_list(conn, &ext)?.unwrap_or_default();
            if o == Op::AdminChannelMemberAdd {
                put_member(conn, &c.slug, &ext, M_IN, now)?;
                if !c.open {
                    l.push(cid.to_string());
                }
            } else {
                note_left(conn, &c.slug, &ext)?;
                l.retain(|x| x != cid);
            }
            set_sent(conn, &ext, &l, now)
        }
        Some(Op::AdminMemberLevel) => {
            // The level sync carried a list (no list: level only, nothing moved).
            let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(()) };
            let ext = ext.to_string();
            let Ok(body) = serde_json::from_str::<ServerMemberChannelsRequest>(&row.payload_raw) else { return Ok(()) };
            let Some(list) = body.channel_ids else { return Ok(()) };
            set_sent(conn, &ext, &list, now)?;
            let have: HashSet<&str> = list.iter().map(String::as_str).collect();
            let mut st = conn
                .prepare(
                    "SELECT m.slug, m.state, c.channel_id FROM tool_admin_chat_members m JOIN tool_admin_chat_channels c ON c.slug = m.slug
                     WHERE m.external_id = ?1 AND c.open = 0 AND c.channel_id IS NOT NULL AND m.state IN ('pending_add','pending_remove')",
                )
                .map_err(|e| format!("home member settle: {e}"))?;
            let rows: Vec<(String, String, String)> = st
                .query_map(params![ext], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| format!("home member settle: {e}"))?
                .collect::<Result<_, _>>()
                .map_err(|e| format!("home member settle row: {e}"))?;
            for (slug, state, cid) in rows {
                match (state.as_str(), have.contains(cid.as_str())) {
                    (M_PENDING_ADD, true) => put_member(conn, &slug, &ext, M_IN, now)?,
                    (M_PENDING_REMOVE, false) => note_left(conn, &slug, &ext)?,
                    _ => {}
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn note_channel_why(conn: &Connection, slug: &str, why: &str, now: i64) -> Result<(), String> {
    conn.execute("UPDATE tool_admin_chat_channels SET why = ?2, updated_at = ?3 WHERE slug = ?1", params![slug, why, now])
        .map(|_| ())
        .map_err(|e| format!("home channel why: {e}"))
}

/// A blocked Home channel row notes why on its channel or member.
pub fn after_blocked(conn: &Connection, id: &str, error: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, id)? else { return Ok(()) };
    match Op::parse(&row.op) {
        Some(Op::AdminChannelCreate | Op::AdminChannelOps) => match row.subject_key.strip_prefix(CHANNEL_PREFIX) {
            Some(slug) => note_channel_why(conn, slug, error, now),
            None => Ok(()),
        },
        Some(Op::AdminChannelMemberAdd | Op::AdminChannelMemberRemove) => {
            let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(()) };
            let v: Value = serde_json::from_str(&row.payload_raw).unwrap_or(Value::Null);
            let Some(c) = v["channel_id"].as_str().map(|c| channel_by_id(conn, c)).transpose()?.flatten() else { return Ok(()) };
            conn.execute(
                "UPDATE tool_admin_chat_members SET why = ?3, updated_at = ?4 WHERE slug = ?1 AND external_id = ?2",
                params![c.slug, ext.to_string(), error, now],
            )
            .map(|_| ())
            .map_err(|e| format!("home channel member blocked: {e}"))
        }
        _ => Ok(()),
    }
}

/// The K5 Space was lost and K2 re-linked to another (the Admin Space's
/// check): every Home channel named an id Sew no longer holds. They are
/// archived in the ledger with the reason, open rows of theirs skipped, and
/// the per-person lists forgotten (nothing private exists in the new Space).
pub fn relink(conn: &Connection, why: &str, now: i64) -> Result<(), String> {
    outbox::skip_open_prefix(conn, CHAT_KIND, CHANNEL_PREFIX, now, why)?;
    conn.execute(
        "UPDATE tool_admin_chat_channels SET state = 'archived', why = ?1, updated_at = ?2 WHERE state != 'archived'",
        params![why, now],
    )
    .map_err(|e| format!("home channels relink: {e}"))?;
    conn.execute("DELETE FROM tool_admin_member_channels", []).map_err(|e| format!("home lists relink: {e}"))?;
    conn.execute("DELETE FROM tool_home_chat_dms", []).map_err(|e| format!("home dms relink: {e}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "admin_channels_tests.rs"]
mod tests;
