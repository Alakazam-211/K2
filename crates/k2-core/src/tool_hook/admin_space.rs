//! The K2 Admin Space and its voice rooms (prd-k2-voice-rooms-v1 v1.9 §2,
//! §3; slices S1 and S2; Sew 0.7.0 SW-1 and SW-4).
//!
//! **What it is.** Exactly one Sew Space per box, slug `k2-admin`, owned by
//! K2's service owner `k2:server` with governance `server`: only K2, over
//! mgmt, changes its channels and members. It holds the box's voice rooms
//! and is K2's fixed default Space (the daemon sets `default_space` to it).
//!
//! **Who's in it** (§2.3): this box's users only.
//! - The box owner `k2:owner` → `moderator`.
//! - Connect logins `k2:connect:<uuid>` (never an owner-linked login, which
//!   is `k2:owner`; never a disabled one) with an enabled `connect` chat
//!   grant: Owner/Admin role → `moderator`, Member → `member`. The daemon
//!   writes the grant ahead of time ([`connect_logins_missing_grant`]), so
//!   a first join never waits on `chat_member_pending`.
//! - Agents `k2:workspace:<uuid>` with access to the Admin Space and an
//!   enabled chat grant → `member`. Agents are added through mgmt, never
//!   through their act-as client (§6.6 2.7 c), and the sweep never removes
//!   one (§2.6).
//! - App users (`k2:principal:`) and `app:` ids are never sent.
//!
//! **Rooms** (§3): `voiceRoomsCount` rooms (0–10, default 1), `room-1` …
//! `room-N`, "Room N" unless renamed. Raising the count ensures (or
//! unarchives) the missing rooms; lowering it closes rooms from the top
//! (SW-4 `closing`: empty → archived at once; live → archived when the last
//! person leaves). "Close now" archives instead (archive ends calls). A box
//! that can't host (not Pro, [`crate::voice_rooms::Hosting`]) keeps the
//! count and hosts nothing: live rooms go `closing`.
//!
//! **Delivery.** Everything rides the S27 outbox (tool `chat`), in order:
//! the Space (`admin:space`), then each person's add and level on their own
//! queue (`admin:m:<space>:<ext>`, after their provisioning queue), then the
//! rooms (`admin:rooms:<space>`, one `channels/bulk` per batch) and each
//! closing (`admin:room:<channel id>`, after the rooms queue).
//!
//! **Dark by default.** [`plan`] does nothing unless the voice rooms
//! preview is on ([`crate::voice_preview::enabled`]), the chat target is
//! enabled, and Sew reports `server_governed_space` (and, for rooms,
//! `voice_channel_closing`).
//!
//! Tables live in `skin.db` (idempotent DDL like `tool_outbox`), so this
//! needs no main-DB migration.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use super::features::VoiceFeatures;
use super::sew_wire::{
    self, channel_ops, ChannelClosingRequest, ChannelOp, ChannelsBulkResponse, ExternalId,
    ServerChannelsBulkRequest, ServerMemberChannelsRequest, ServerSpaceCreateRequest,
    SpaceMemberRemoveRequest, SpaceMembersAddRequest, SpaceMembersAddResponse,
};
use super::{outbox, Cause, NewOp, Op, CHAT_KIND};
use crate::voice_rooms::{self, Hosting};

/// The Space's own queue.
pub const SUBJECT_SPACE: &str = "admin:space";
/// A member's queue: `admin:m:<space id>:<external id>`.
pub const MEMBER_PREFIX: &str = "admin:m:";
/// The rooms' queue: `admin:rooms:<space id>`.
pub const ROOMS_PREFIX: &str = "admin:rooms:";
/// One room's closing: `admin:room:<channel id>`.
pub const ROOM_PREFIX: &str = "admin:room:";
/// Attribution of the sync's own writes (`Sew-Actor`, "reported by K2").
pub const ACTOR: &str = "system:voice-rooms";

/// Space states.
pub const SPACE_READY: &str = "ready";
pub const SPACE_BLOCKED: &str = "blocked";
/// Member ledger states.
pub const PENDING_ADD: &str = "pending_add";
pub const ADDED: &str = "added";
pub const PENDING_REMOVE: &str = "pending_remove";
/// Room states.
pub const ROOM_PENDING: &str = "pending";
pub const ROOM_ACTIVE: &str = "active";
pub const ROOM_CLOSING: &str = "closing";
pub const ROOM_ARCHIVED: &str = "archived";
/// The `why` prefix of a member whose add Sew held.
pub const HELD_MARK: &str = "held: ";

pub fn member_subject(space_id: &str, ext: &str) -> String {
    format!("{MEMBER_PREFIX}{space_id}:{ext}")
}

pub fn rooms_subject(space_id: &str) -> String {
    format!("{ROOMS_PREFIX}{space_id}")
}

pub fn room_subject(channel_id: &str) -> String {
    format!("{ROOM_PREFIX}{channel_id}")
}

/// `admin:m:<space>:<ext>` → `(space, ext)`.
pub fn parse_member_subject(subject: &str) -> Option<(String, ExternalId)> {
    let rest = subject.strip_prefix(MEMBER_PREFIX)?;
    let (space, ext) = rest.split_once(':')?;
    if !sew_wire::is_sew_id(space) {
        return None;
    }
    Some((space.to_string(), ExternalId::parse(ext).ok()?))
}

/// The mgmt path an Admin Space row goes to.
pub fn path_for(op: Op, subject: &str) -> Option<String> {
    match op {
        Op::AdminSpaceCreate => Some(sew_wire::MGMT_SPACES_CREATE.to_string()),
        Op::AdminMemberAdd => Some(sew_wire::MGMT_SPACE_MEMBERS_ADD.to_string()),
        Op::AdminMemberRemove => Some(sew_wire::MGMT_SPACE_MEMBERS_REMOVE.to_string()),
        Op::AdminMemberLevel => parse_member_subject(subject).map(|(s, e)| sew_wire::member_channels_path(&s, &e)),
        Op::AdminRooms => Some(sew_wire::MGMT_CHANNELS_BULK.to_string()),
        Op::AdminRoomClosing => subject
            .strip_prefix(ROOM_PREFIX)
            .filter(|id| sew_wire::is_sew_id(id))
            .map(sew_wire::channel_closing_path),
        _ => None,
    }
}

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_admin_space (
            id            INTEGER PRIMARY KEY CHECK (id = 1),
            space_id      TEXT,
            name          TEXT,
            state         TEXT NOT NULL CHECK (state IN ('ready','blocked')),
            why           TEXT,
            default_from  TEXT,
            default_at    INTEGER,
            carried       TEXT,
            count_stored  INTEGER NOT NULL DEFAULT 0,
            updated_at    INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_admin_members (
            external_id   TEXT PRIMARY KEY NOT NULL,
            state         TEXT NOT NULL CHECK (state IN ('pending_add','added','pending_remove')),
            level         TEXT,
            want_level    TEXT,
            why           TEXT,
            updated_at    INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_admin_rooms (
            n             INTEGER PRIMARY KEY NOT NULL CHECK (n BETWEEN 1 AND 10),
            slug          TEXT NOT NULL UNIQUE,
            channel_id    TEXT,
            name          TEXT,
            want_name     TEXT,
            state         TEXT NOT NULL CHECK (state IN ('pending','active','closing','archived')),
            close_now     INTEGER NOT NULL DEFAULT 0,
            why           TEXT,
            updated_at    INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db admin space schema: {e}"))
}

// ── facts from outside skin.db ───────────────────────────────────────

/// A Connect login as the Admin Space needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectPerson {
    pub id: String,
    /// Owner or Admin role → `moderator`; Member → `member`.
    pub moderator: bool,
    pub disabled: bool,
    /// Linked to the owner: it is `k2:owner` in chat, never a member here.
    pub owner_linked: bool,
}

/// What the sweep knows from outside skin.db (read before the skin lock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// [`crate::voice_preview::enabled`].
    pub enabled: bool,
    /// The Space's name: the server's display name, else its label.
    pub name: String,
    pub hosting: Hosting,
    /// `voiceRoomsCount` (0–10).
    pub count: u32,
    pub connect: Vec<ConnectPerson>,
    /// Workspace ids with access to the Admin Space (`sew_chat_access`).
    pub agents: Vec<String>,
}

impl Facts {
    /// Every Connect login with its role, disabled flag and owner link.
    pub fn load_connect() -> Result<Vec<ConnectPerson>, String> {
        let linked = crate::notify_prefs::People { users: Vec::new(), linked: crate::app_settings::load().owner_accounts };
        Ok(crate::connect_users::admin_space_facts()?
            .into_iter()
            .map(|(id, username, role, disabled)| ConnectPerson {
                id,
                moderator: role >= crate::connect_users::Role::Admin,
                disabled,
                owner_linked: linked.is_linked(&username),
            })
            .collect())
    }
}

// ── the ledger ───────────────────────────────────────────────────────

/// The Admin Space as K2 holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
    pub space_id: Option<String>,
    pub name: Option<String>,
    /// `ready` | `blocked` (no row: not made yet).
    pub state: String,
    pub why: Option<String>,
    /// The default Space before the switch to `k2-admin` (§6.6 2.7 d).
    pub default_from: Option<String>,
    pub default_at: Option<i64>,
    /// Workspace ids whose access was carried over to `k2-admin`.
    pub carried: Vec<String>,
    /// `voiceRoomsCount` was written to the settings at set-up (§3.1:
    /// "written as 1 at setup").
    pub count_stored: bool,
}

pub fn space_row(conn: &Connection) -> Result<Option<SpaceRow>, String> {
    conn.query_row(
        "SELECT space_id, name, state, why, default_from, default_at, carried, count_stored FROM tool_admin_space WHERE id = 1",
        [],
        |r| {
            let carried: Option<String> = r.get(6)?;
            Ok(SpaceRow {
                space_id: r.get(0)?,
                name: r.get(1)?,
                state: r.get(2)?,
                why: r.get(3)?,
                default_from: r.get(4)?,
                default_at: r.get(5)?,
                carried: carried.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default(),
                count_stored: r.get::<_, i64>(7)? != 0,
            })
        },
    )
    .optional()
    .map_err(|e| format!("admin space read: {e}"))
}

/// The room count was written to the settings at set-up.
pub fn note_count_stored(conn: &Connection, now: i64) -> Result<(), String> {
    conn.execute("UPDATE tool_admin_space SET count_stored = 1, updated_at = ?1 WHERE id = 1", params![now])
        .map(|_| ())
        .map_err(|e| format!("admin space count stored: {e}"))
}

/// The ready Admin Space's id.
pub fn ready_space_id(conn: &Connection) -> Result<Option<String>, String> {
    Ok(space_row(conn)?.filter(|s| s.state == SPACE_READY).and_then(|s| s.space_id))
}

fn set_space(conn: &Connection, space_id: Option<&str>, name: Option<&str>, state: &str, why: Option<&str>, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_admin_space (id, space_id, name, state, why, updated_at) VALUES (1, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET space_id = COALESCE(excluded.space_id, space_id),
            name = COALESCE(excluded.name, name), state = excluded.state, why = excluded.why,
            updated_at = excluded.updated_at",
        params![space_id, name, state, why, now],
    )
    .map(|_| ())
    .map_err(|e| format!("admin space write: {e}"))
}

/// The daemon switched `default_space` to `k2-admin` (from `from`) and
/// carried `carried` agents' access over (§6.6 2.7 d).
pub fn note_default_switch(conn: &Connection, from: Option<&str>, carried: &[String], now: i64) -> Result<(), String> {
    let c = serde_json::to_string(carried).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE tool_admin_space SET default_from = ?1, default_at = ?2, carried = ?3, updated_at = ?2 WHERE id = 1",
        params![from, now, c],
    )
    .map(|_| ())
    .map_err(|e| format!("admin space default switch: {e}"))
}

/// One member as K2 holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRow {
    pub external_id: String,
    pub state: String,
    /// The level Sew confirmed.
    pub level: Option<String>,
    /// The level K2 wants (a sync is queued while it differs).
    pub want_level: Option<String>,
    pub why: Option<String>,
}

pub fn members(conn: &Connection) -> Result<Vec<MemberRow>, String> {
    let mut s = conn
        .prepare("SELECT external_id, state, level, want_level, why FROM tool_admin_members ORDER BY external_id")
        .map_err(|e| format!("admin members: {e}"))?;
    let rows = s
        .query_map([], |r| Ok(MemberRow { external_id: r.get(0)?, state: r.get(1)?, level: r.get(2)?, want_level: r.get(3)?, why: r.get(4)? }))
        .map_err(|e| format!("admin members: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("admin members row: {e}"))
}

fn put_member(conn: &Connection, ext: &str, state: &str, level: Option<&str>, want: Option<&str>, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_admin_members (external_id, state, level, want_level, why, updated_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5)
         ON CONFLICT(external_id) DO UPDATE SET state = excluded.state, level = excluded.level,
            want_level = excluded.want_level, why = NULL, updated_at = excluded.updated_at",
        params![ext, state, level, want, now],
    )
    .map(|_| ())
    .map_err(|e| format!("admin member write: {e}"))
}

/// One room as K2 holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomRow {
    pub n: u32,
    pub slug: String,
    pub channel_id: Option<String>,
    /// The name Sew holds.
    pub name: Option<String>,
    /// The owner's name for it (`None` = "Room N").
    pub want_name: Option<String>,
    pub state: String,
    pub close_now: bool,
    pub why: Option<String>,
}

impl RoomRow {
    /// The name the room should carry.
    pub fn desired_name(&self) -> String {
        self.want_name.clone().unwrap_or_else(|| voice_rooms::room_default_name(self.n))
    }
}

pub fn rooms(conn: &Connection) -> Result<Vec<RoomRow>, String> {
    let mut s = conn
        .prepare("SELECT n, slug, channel_id, name, want_name, state, close_now, why FROM tool_admin_rooms ORDER BY n")
        .map_err(|e| format!("admin rooms: {e}"))?;
    let rows = s
        .query_map([], |r| {
            Ok(RoomRow {
                n: r.get(0)?,
                slug: r.get(1)?,
                channel_id: r.get(2)?,
                name: r.get(3)?,
                want_name: r.get(4)?,
                state: r.get(5)?,
                close_now: r.get::<_, i64>(6)? != 0,
                why: r.get(7)?,
            })
        })
        .map_err(|e| format!("admin rooms: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("admin rooms row: {e}"))
}

fn set_room_state(conn: &Connection, slug: &str, state: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_admin_rooms SET state = ?2, close_now = CASE WHEN ?2 = 'archived' THEN 0 ELSE close_now END,
            why = NULL, updated_at = ?3 WHERE slug = ?1",
        params![slug, state, now],
    )
    .map(|_| ())
    .map_err(|e| format!("admin room write: {e}"))
}

// ── planning ─────────────────────────────────────────────────────────

/// What one plan saw (for `k2 voice status` and the sweep's log).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Why nothing was planned, if so: `preview_off`, `chat_off`,
    /// `sew_too_old` (no `server_governed_space`), `space_pending`,
    /// `space_blocked`.
    pub waiting: Option<String>,
    pub create: bool,
    pub adds: usize,
    pub removes: usize,
    pub levels: usize,
    pub room_ops: usize,
    pub closings: usize,
    /// Rooms aren't hosted: `not_pro` (the Pro gate) or `sew_too_old` (no
    /// `voice_channel_closing`).
    pub rooms_dormant: Option<String>,
}

fn enabled_chat_grant(conn: &Connection, subject_kind: &str, subject_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM grants WHERE kind = ?1 AND subject_kind = ?2 AND subject_id = ?3 AND enabled = 1)",
        params![CHAT_KIND, subject_kind, subject_id],
        |r| r.get(0),
    )
    .map_err(|e| format!("admin space grant check: {e}"))
}

/// The members K2 wants in the Admin Space, with their levels.
pub fn desired_members(conn: &Connection, facts: &Facts) -> Result<BTreeMap<String, &'static str>, String> {
    let mut out = BTreeMap::new();
    out.insert(ExternalId::K2Owner.to_string(), sew_wire::LEVEL_MODERATOR);
    for c in &facts.connect {
        if c.owner_linked || c.disabled || !sew_wire::is_lower_uuid(&c.id) {
            continue;
        }
        if !enabled_chat_grant(conn, "connect", &c.id)? {
            continue;
        }
        let level = if c.moderator { sew_wire::LEVEL_MODERATOR } else { sew_wire::LEVEL_MEMBER };
        out.insert(ExternalId::K2Connect(c.id.clone()).to_string(), level);
    }
    for a in &facts.agents {
        if !sew_wire::is_lower_uuid(a) || !enabled_chat_grant(conn, "workspace", a)? {
            continue;
        }
        out.insert(ExternalId::K2Workspace(a.clone()).to_string(), sew_wire::LEVEL_MEMBER);
    }
    Ok(out)
}

fn op(o: Op, subject: String, payload: Value, after: Option<String>) -> NewOp {
    NewOp {
        tool: CHAT_KIND,
        op: o,
        subject_key: subject,
        payload,
        grant_id: None,
        grant_rev: None,
        cause: Cause::VoiceRooms,
        after_subject: after,
    }
}

fn level_op(space: &str, ext: &str, level: &str) -> Result<NewOp, String> {
    let body = ServerMemberChannelsRequest { channel_ids: Vec::new(), level: level.to_string() };
    Ok(op(Op::AdminMemberLevel, member_subject(space, ext), serde_json::to_value(body).map_err(|e| e.to_string())?, None))
}

/// Plan the Admin Space's ops for this sweep and move the ledger. MUST run
/// inside the caller's transaction; the caller enqueues the ops.
pub fn plan(conn: &Connection, facts: &Facts, busy: &HashSet<String>, now: i64) -> Result<(Vec<NewOp>, Summary), String> {
    let mut s = Summary::default();
    let mut out = Vec::new();
    if !facts.enabled {
        s.waiting = Some("preview_off".into());
        return Ok((out, s));
    }
    if !outbox::target_enabled(conn, CHAT_KIND)? {
        s.waiting = Some("chat_off".into());
        return Ok((out, s));
    }
    let flags = VoiceFeatures::read(conn, CHAT_KIND)?;
    if !flags.server_governed_space {
        s.waiting = Some("sew_too_old".into());
        return Ok((out, s));
    }
    let row = space_row(conn)?;
    let space_id = match row.as_ref() {
        Some(r) if r.state == SPACE_BLOCKED => {
            s.waiting = Some("space_blocked".into());
            return Ok((out, s));
        }
        Some(r) if r.state == SPACE_READY && r.space_id.is_some() => r.space_id.clone().expect("checked"),
        _ => {
            if !busy.contains(SUBJECT_SPACE) {
                let body = ServerSpaceCreateRequest::admin(&space_name(&facts.name));
                body.validate()?;
                out.push(op(Op::AdminSpaceCreate, SUBJECT_SPACE.into(), serde_json::to_value(body).map_err(|e| e.to_string())?, None));
                s.create = true;
            }
            s.waiting = Some("space_pending".into());
            return Ok((out, s));
        }
    };
    plan_members(conn, facts, &space_id, busy, now, &mut out, &mut s)?;
    plan_rooms(conn, facts, &flags, &space_id, busy, now, &mut out, &mut s)?;
    Ok((out, s))
}

/// 1–100 characters; a blank name falls back to "K2".
fn space_name(name: &str) -> String {
    let t: String = name.trim().chars().filter(|c| !c.is_control()).take(100).collect();
    if t.is_empty() {
        "K2".into()
    } else {
        t
    }
}

fn plan_members(
    conn: &Connection,
    facts: &Facts,
    space: &str,
    busy: &HashSet<String>,
    now: i64,
    out: &mut Vec<NewOp>,
    s: &mut Summary,
) -> Result<(), String> {
    let want = desired_members(conn, facts)?;
    let have: BTreeMap<String, MemberRow> = members(conn)?.into_iter().map(|m| (m.external_id.clone(), m)).collect();
    for (ext, level) in &want {
        let subject = member_subject(space, ext);
        if busy.contains(&subject) {
            continue;
        }
        // HC2: an add Sew held (banned, guest, deactivated) is not sent
        // again on its own; `k2 hostchat log retry` sends it.
        if have.get(ext).is_some_and(|r| r.state == PENDING_ADD && r.why.as_deref().is_some_and(|w| w.starts_with(HELD_MARK))) {
            continue;
        }
        let row = have.get(ext);
        let in_space = row.is_some_and(|r| r.state == ADDED);
        if !in_space {
            // A fresh add (or one whose row was pruned or skipped): after
            // the person's own provisioning queue.
            let body = SpaceMembersAddRequest {
                space_id: space.to_string(),
                app_id: None,
                external_ids: vec![ExternalId::parse(ext)?],
            };
            out.push(op(Op::AdminMemberAdd, subject.clone(), serde_json::to_value(body).map_err(|e| e.to_string())?, Some(ext.clone())));
            out.push(level_op(space, ext, level)?);
            put_member(conn, ext, PENDING_ADD, None, Some(level), now)?;
            s.adds += 1;
            s.levels += 1;
        } else if row.and_then(|r| r.level.as_deref()) != Some(*level) {
            out.push(level_op(space, ext, level)?);
            conn.execute(
                "UPDATE tool_admin_members SET want_level = ?2, updated_at = ?3 WHERE external_id = ?1",
                params![ext, level, now],
            )
            .map_err(|e| format!("admin member level: {e}"))?;
            s.levels += 1;
        }
    }
    for (ext, row) in &have {
        if want.contains_key(ext) || row.state == PENDING_REMOVE {
            continue;
        }
        let parsed = ExternalId::parse(ext)?;
        // §2.6: the sweep never removes k2:server or agents.
        if parsed.is_agent() || parsed == ExternalId::K2Server {
            continue;
        }
        let subject = member_subject(space, ext);
        if busy.contains(&subject) {
            continue;
        }
        let body = SpaceMemberRemoveRequest { space_id: space.to_string(), app_id: None, external_id: parsed };
        out.push(op(Op::AdminMemberRemove, subject, serde_json::to_value(body).map_err(|e| e.to_string())?, None));
        conn.execute(
            "UPDATE tool_admin_members SET state = 'pending_remove', updated_at = ?2 WHERE external_id = ?1",
            params![ext, now],
        )
        .map_err(|e| format!("admin member remove: {e}"))?;
        s.removes += 1;
    }
    Ok(())
}

fn room_op(o: &str, slug: &str, channel_id: Option<&str>, name: Option<String>) -> ChannelOp {
    let ensure = o == channel_ops::ENSURE;
    ChannelOp {
        op: o.into(),
        slug: Some(slug.into()),
        channel_id: channel_id.map(str::to_string),
        name,
        kind: ensure.then(|| sew_wire::CHANNEL_TYPE_VOICE.into()),
        private: ensure.then_some(false),
        user_limit: ensure.then_some(voice_rooms::ROOM_USER_LIMIT),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_rooms(
    conn: &Connection,
    facts: &Facts,
    flags: &VoiceFeatures,
    space: &str,
    busy: &HashSet<String>,
    now: i64,
    out: &mut Vec<NewOp>,
    s: &mut Summary,
) -> Result<(), String> {
    if !flags.voice_channel_closing {
        s.rooms_dormant = Some("sew_too_old".into());
        return Ok(());
    }
    let rooms_subject = rooms_subject(space);
    if busy.contains(&rooms_subject) {
        return Ok(());
    }
    let target = match facts.hosting {
        Hosting::Ok => facts.count.min(voice_rooms::ROOMS_MAX),
        Hosting::NotPro => {
            s.rooms_dormant = Some("not_pro".into());
            0
        }
    };
    let have: BTreeMap<u32, RoomRow> = rooms(conn)?.into_iter().map(|r| (r.n, r)).collect();
    let mut ops: Vec<ChannelOp> = Vec::new();
    let mut closings: Vec<(String, bool)> = Vec::new();
    for n in 1..=target {
        let slug = voice_rooms::room_slug(n);
        match have.get(&n) {
            None => {
                ops.push(room_op(channel_ops::ENSURE, &slug, None, Some(voice_rooms::room_default_name(n))));
                conn.execute(
                    "INSERT INTO tool_admin_rooms (n, slug, state, updated_at) VALUES (?1, ?2, 'pending', ?3)",
                    params![n, slug, now],
                )
                .map_err(|e| format!("admin room insert: {e}"))?;
            }
            Some(r) => {
                let desired = r.desired_name();
                match (r.state.as_str(), r.channel_id.as_deref()) {
                    // An ensure that never landed (its row was skipped or
                    // pruned): ensure again (idempotent on the slug).
                    (ROOM_PENDING, _) | (_, None) => {
                        ops.push(room_op(channel_ops::ENSURE, &slug, None, Some(desired.clone())));
                    }
                    (ROOM_ARCHIVED, Some(id)) => ops.push(room_op(channel_ops::UNARCHIVE, &slug, Some(id), None)),
                    (ROOM_CLOSING, Some(id)) => closings.push((id.to_string(), false)),
                    _ => {}
                }
                if let Some(id) = r.channel_id.as_deref() {
                    if r.name.as_deref() != Some(desired.as_str()) && r.state != ROOM_PENDING {
                        ops.push(room_op(channel_ops::RENAME, &slug, Some(id), Some(desired)));
                    }
                }
                if r.close_now {
                    conn.execute("UPDATE tool_admin_rooms SET close_now = 0 WHERE n = ?1", params![n])
                        .map_err(|e| format!("admin room close_now: {e}"))?;
                }
            }
        }
    }
    for r in have.values().filter(|r| r.n > target) {
        let Some(id) = r.channel_id.as_deref() else { continue };
        match (r.state.as_str(), r.close_now) {
            (ROOM_ACTIVE | ROOM_CLOSING, true) => ops.push(room_op(channel_ops::ARCHIVE, &r.slug, Some(id), None)),
            (ROOM_ACTIVE, false) => closings.push((id.to_string(), true)),
            _ => {}
        }
    }
    if !ops.is_empty() {
        s.room_ops = ops.len();
        let body = ServerChannelsBulkRequest { space_id: space.to_string(), ops };
        out.push(op(Op::AdminRooms, rooms_subject.clone(), serde_json::to_value(body).map_err(|e| e.to_string())?, None));
    }
    for (id, closing) in closings {
        let subject = room_subject(&id);
        if busy.contains(&subject) {
            continue;
        }
        let body = ChannelClosingRequest { closing };
        out.push(op(Op::AdminRoomClosing, subject, serde_json::to_value(body).map_err(|e| e.to_string())?, Some(rooms_subject.clone())));
        s.closings += 1;
    }
    Ok(())
}

// ── after delivery ───────────────────────────────────────────────────

fn error_detail(result: &str, key: &str) -> Option<String> {
    let v: Value = serde_json::from_str(result).ok()?;
    v.pointer(&format!("/error/details/{key}")).and_then(Value::as_str).map(str::to_string)
}

/// A delivered Admin Space row moves the ledger on (inside
/// [`super::record_done`]'s connection). Returns whether the sweep should
/// plan again soon (the Space became ready).
pub fn after_done(conn: &Connection, id: &str, result: &str, now: i64) -> Result<bool, String> {
    let Some(row) = outbox::get(conn, id)? else { return Ok(false) };
    match Op::parse(&row.op) {
        Some(Op::AdminSpaceCreate) => {
            // 2xx: the create answer, `created: true`, or (sew-56 v1.2) a
            // replay of the same body, `created: false`: both are success.
            // 409 server_space_exists: read `details.space_id` back.
            let created: Option<sew_wire::SpaceCreateResponse> = serde_json::from_str(result).ok();
            let space_id = created.map(|c| c.space_id).or_else(|| error_detail(result, "space_id"));
            let Some(space_id) = space_id.filter(|s| sew_wire::is_sew_id(s)) else {
                set_space(conn, None, None, SPACE_BLOCKED, Some("Sew named no space_id for the K2 Admin Space"), now)?;
                return Ok(false);
            };
            let name = serde_json::from_str::<Value>(&row.payload_raw).ok().and_then(|v| v["name"].as_str().map(str::to_string));
            set_space(conn, Some(&space_id), name.as_deref(), SPACE_READY, None, now)?;
            Ok(true)
        }
        Some(Op::AdminMemberAdd) => {
            let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(false) };
            let added = serde_json::from_str::<SpaceMembersAddResponse>(result)
                .is_ok_and(|r| r.added.iter().chain(r.already.iter()).any(|m| m.external_id == ext));
            if added {
                conn.execute(
                    "UPDATE tool_admin_members SET state = 'added', why = NULL, updated_at = ?2
                     WHERE external_id = ?1 AND state = 'pending_add'",
                    params![ext.to_string(), now],
                )
                .map_err(|e| format!("admin member added: {e}"))?;
            }
            Ok(false)
        }
        Some(Op::AdminMemberRemove) => {
            let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(false) };
            conn.execute(
                "DELETE FROM tool_admin_members WHERE external_id = ?1 AND state = 'pending_remove'",
                params![ext.to_string()],
            )
            .map_err(|e| format!("admin member removed: {e}"))?;
            Ok(false)
        }
        Some(Op::AdminMemberLevel) => {
            let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(false) };
            let answer: sew_wire::MemberChannelsResponse = serde_json::from_str(result).unwrap_or_default();
            let sent = serde_json::from_str::<ServerMemberChannelsRequest>(&row.payload_raw).ok().map(|b| b.level);
            match (answer.level, answer.skipped) {
                // The sync joins the Space too: the person is in at that level.
                (Some(level), _) => conn
                    .execute(
                        "UPDATE tool_admin_members SET level = ?2, why = NULL, updated_at = ?3,
                            state = CASE WHEN state = 'pending_add' THEN 'added' ELSE state END
                         WHERE external_id = ?1 AND state != 'pending_remove'",
                        params![ext.to_string(), level, now],
                    )
                    .map_err(|e| format!("admin member level: {e}"))?,
                // Skipped (banned, deprovisioned): note it, never over a held add.
                (None, skipped) => conn
                    .execute(
                        "UPDATE tool_admin_members SET why = ?2, updated_at = ?3
                         WHERE external_id = ?1 AND (why IS NULL OR why NOT LIKE 'held: %')",
                        params![ext.to_string(), format!("level {} not applied: skipped {}", sent.unwrap_or_default(), skipped.unwrap_or_default()), now],
                    )
                    .map_err(|e| format!("admin member level: {e}"))?,
            };
            Ok(false)
        }
        Some(Op::AdminRooms) => {
            let sent: ServerChannelsBulkRequest = serde_json::from_str(&row.payload_raw).map_err(|e| format!("admin rooms payload: {e}"))?;
            let resp: ChannelsBulkResponse = serde_json::from_str(result).unwrap_or_default();
            for (i, o) in sent.ops.iter().enumerate() {
                let Some(slug) = o.slug.as_deref() else { continue };
                let Some(r) = resp.results.get(i) else { continue };
                if let Some(e) = &r.error {
                    conn.execute(
                        "UPDATE tool_admin_rooms SET why = ?2, updated_at = ?3 WHERE slug = ?1",
                        params![slug, format!("{} refused: {} ({})", o.op, e.reason(), e.hint), now],
                    )
                    .map_err(|e| format!("admin room why: {e}"))?;
                    continue;
                }
                match o.op.as_str() {
                    channel_ops::ENSURE | channel_ops::UNARCHIVE => {
                        conn.execute(
                            "UPDATE tool_admin_rooms SET channel_id = COALESCE(?2, channel_id), name = COALESCE(?3, name),
                                state = 'active', why = NULL, updated_at = ?4 WHERE slug = ?1",
                            params![slug, r.channel_id, r.name, now],
                        )
                        .map_err(|e| format!("admin room ensure: {e}"))?;
                    }
                    channel_ops::RENAME => {
                        conn.execute(
                            "UPDATE tool_admin_rooms SET name = COALESCE(?2, name), why = NULL, updated_at = ?3 WHERE slug = ?1",
                            params![slug, r.name.clone().or_else(|| o.name.clone()), now],
                        )
                        .map_err(|e| format!("admin room rename: {e}"))?;
                    }
                    channel_ops::ARCHIVE => set_room_state(conn, slug, ROOM_ARCHIVED, now)?,
                    _ => {}
                }
            }
            Ok(false)
        }
        Some(Op::AdminRoomClosing) => {
            let Some(cid) = row.subject_key.strip_prefix(ROOM_PREFIX) else { return Ok(false) };
            let state = if error_detail(result, "reason").as_deref() == Some(sew_wire::reasons::CHANNEL_ARCHIVED) {
                ROOM_ARCHIVED
            } else {
                let a: sew_wire::ChannelClosingResponse = serde_json::from_str(result).unwrap_or_default();
                match (a.archived, a.closing) {
                    (true, _) => ROOM_ARCHIVED,
                    (false, true) => ROOM_CLOSING,
                    (false, false) => ROOM_ACTIVE,
                }
            };
            note_room_state(conn, cid, state, now)?;
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// A blocked Admin Space row: a blocked create blocks the Space (its
/// reason shows in `k2 voice status`); a blocked member row notes why.
pub fn after_blocked(conn: &Connection, id: &str, error: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, id)? else { return Ok(()) };
    match Op::parse(&row.op) {
        Some(Op::AdminSpaceCreate) => set_space(conn, None, None, SPACE_BLOCKED, Some(error), now),
        Some(Op::AdminMemberAdd | Op::AdminMemberLevel | Op::AdminMemberRemove) => {
            let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(()) };
            conn.execute(
                "UPDATE tool_admin_members SET why = ?2, updated_at = ?3 WHERE external_id = ?1",
                params![ext.to_string(), error, now],
            )
            .map(|_| ())
            .map_err(|e| format!("admin member blocked: {e}"))
        }
        _ => Ok(()),
    }
}

/// An Admin Space add Sew held (HC2: banned, or `not_added` for a reason
/// other than `not_provisioned`): the member row notes why.
pub fn after_held(conn: &Connection, id: &str, why: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, id)? else { return Ok(()) };
    if Op::parse(&row.op) != Some(Op::AdminMemberAdd) {
        return Ok(());
    }
    let Some((_, ext)) = parse_member_subject(&row.subject_key) else { return Ok(()) };
    conn.execute(
        "UPDATE tool_admin_members SET why = ?2, updated_at = ?3 WHERE external_id = ?1",
        params![ext.to_string(), format!("{HELD_MARK}{why}"), now],
    )
    .map(|_| ())
    .map_err(|e| format!("admin member held: {e}"))
}

/// A room's state as Sew reported it (the SW-4 answer, or the SW-3 feed's
/// `channel_closing` / `channel_archived`).
pub fn note_room_state(conn: &Connection, channel_id: &str, state: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_admin_rooms SET state = ?2, close_now = CASE WHEN ?2 = 'archived' THEN 0 ELSE close_now END,
            why = NULL, updated_at = ?3 WHERE channel_id = ?1",
        params![channel_id, state, now],
    )
    .map(|_| ())
    .map_err(|e| format!("admin room state: {e}"))
}

/// A blocked create may be retried: the owner fixed what blocked it (e.g.
/// renamed the Space that held `k2-admin`). Clears the blocked state so
/// the next sweep creates again. Returns whether it was blocked.
pub fn clear_blocked(conn: &Connection, now: i64) -> Result<bool, String> {
    let n = conn
        .execute("DELETE FROM tool_admin_space WHERE id = 1 AND state = 'blocked'", [])
        .map_err(|e| format!("admin space clear: {e}"))?;
    if n > 0 {
        outbox::skip_blocked_subject(conn, CHAT_KIND, SUBJECT_SPACE, now, "the K2 Admin Space create is retried")?;
    }
    Ok(n > 0)
}

// ── owner actions (through the daemon's routes, lane C) ───────────────

/// Rename room `n_or_slug` (`3` or `room-3`) to `name` (the next sweep
/// sends it). Returns the room's slug.
pub fn set_room_name(conn: &Connection, n_or_slug: &str, name: &str, now: i64) -> Result<String, String> {
    let n = n_or_slug
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|n| (1..=voice_rooms::ROOMS_MAX).contains(n))
        .or_else(|| voice_rooms::room_number(n_or_slug.trim()))
        .ok_or_else(|| format!("no room '{}': name one as 1-{} or room-<n>", n_or_slug.trim(), voice_rooms::ROOMS_MAX))?;
    let name = voice_rooms::valid_room_name(name)?;
    let want = (name != voice_rooms::room_default_name(n)).then_some(name);
    let slug = voice_rooms::room_slug(n);
    conn.execute(
        "INSERT INTO tool_admin_rooms (n, slug, want_name, state, updated_at) VALUES (?1, ?2, ?3, 'pending', ?4)
         ON CONFLICT(n) DO UPDATE SET want_name = excluded.want_name, updated_at = excluded.updated_at",
        params![n, slug, want, now],
    )
    .map_err(|e| format!("admin room rename: {e}"))?;
    Ok(slug)
}

/// "Close now" (§3.4): the rooms above `count` are archived at the next
/// plan instead of going `closing` (archive ends their calls). The caller
/// gives people the 10 s notice first, then stores the count and calls
/// this. Returns the slugs marked.
pub fn mark_close_now(conn: &Connection, count: u32, now: i64) -> Result<Vec<String>, String> {
    let marked: Vec<String> = rooms(conn)?
        .into_iter()
        .filter(|r| r.n > count && matches!(r.state.as_str(), ROOM_ACTIVE | ROOM_CLOSING))
        .map(|r| r.slug)
        .collect();
    for slug in &marked {
        conn.execute("UPDATE tool_admin_rooms SET close_now = 1, updated_at = ?2 WHERE slug = ?1", params![slug, now])
            .map_err(|e| format!("admin room close now: {e}"))?;
    }
    Ok(marked)
}

/// The rooms a lower count would close (for the notice), live first.
pub fn rooms_above(conn: &Connection, count: u32) -> Result<Vec<RoomRow>, String> {
    Ok(rooms(conn)?
        .into_iter()
        .filter(|r| r.n > count && matches!(r.state.as_str(), ROOM_ACTIVE | ROOM_CLOSING))
        .collect())
}

/// Is a level sync for `ext` still on its way (V13: hold a join until the
/// new level lands)?
pub fn level_sync_pending(conn: &Connection, ext: &str) -> Result<bool, String> {
    let Some(space) = ready_space_id(conn)? else { return Ok(false) };
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tool_outbox WHERE tool = ?1 AND subject_key = ?2 AND op IN ('admin_member_add','admin_member_level')
                 AND state IN ('pending','blocked'))",
        params![CHAT_KIND, member_subject(&space, ext)],
        |r| r.get(0),
    )
    .map_err(|e| format!("admin level pending: {e}"))
}

/// Everything `k2 voice status` shows about the Admin Space.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub space: Option<SpaceRow>,
    pub members: Vec<MemberRow>,
    pub rooms: Vec<RoomRow>,
    /// Open outbox rows of the Admin Space (`pending` / `blocked`) by
    /// subject, oldest first: what holds the sync.
    pub holds: Vec<Hold>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hold {
    pub id: String,
    pub op: String,
    pub subject: String,
    pub state: String,
    pub error: Option<String>,
}

pub fn view(conn: &Connection) -> Result<View, String> {
    let mut s = conn
        .prepare(
            "SELECT id, op, subject_key, state, last_error FROM tool_outbox
             WHERE tool = ?1 AND subject_key LIKE 'admin:%' AND state IN ('pending','blocked') ORDER BY seq",
        )
        .map_err(|e| format!("admin holds: {e}"))?;
    let holds = s
        .query_map(params![CHAT_KIND], |r| Ok(Hold { id: r.get(0)?, op: r.get(1)?, subject: r.get(2)?, state: r.get(3)?, error: r.get(4)? }))
        .map_err(|e| format!("admin holds: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("admin holds row: {e}"))?;
    Ok(View { space: space_row(conn)?, members: members(conn)?, rooms: rooms(conn)?, holds })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_and_paths_round_trip() {
        let u = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let s = member_subject("7001", &format!("k2:connect:{u}"));
        assert_eq!(s, format!("admin:m:7001:k2:connect:{u}"));
        let (space, ext) = parse_member_subject(&s).expect("parses");
        assert_eq!((space.as_str(), ext.clone()), ("7001", ExternalId::K2Connect(u.into())));
        assert_eq!(
            path_for(Op::AdminMemberLevel, &s).as_deref(),
            Some(format!("/mgmt/v1/spaces/7001/members/k2%3Aconnect%3A{u}/channels").as_str())
        );
        assert_eq!(path_for(Op::AdminRoomClosing, &room_subject("901")).as_deref(), Some("/mgmt/v1/channels/901/closing"));
        assert_eq!(path_for(Op::AdminSpaceCreate, SUBJECT_SPACE).as_deref(), Some("/mgmt/v1/spaces/create"));
        assert_eq!(path_for(Op::AdminRooms, &rooms_subject("7001")).as_deref(), Some("/mgmt/v1/channels/bulk"));
        assert_eq!(path_for(Op::Provision, "k2:owner"), None);
        assert!(parse_member_subject("admin:m:bad space:k2:owner").is_none());
        assert_eq!(space_name("  "), "K2");
        assert_eq!(space_name(&"x".repeat(150)).chars().count(), 100);
    }
}
