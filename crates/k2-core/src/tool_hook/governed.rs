//! App-governed Spaces, K2's half (prd-app-governed-spaces-k2-v1 S4 + S5;
//! Sew 0.6.0, sew-55 v2.4 I3, I4, I6, §3a). Every path here waits for
//! Sew's feature flag [`features::APP_GOVERNED_SPACES`], never a version.
//!
//! **Governance (S5, I3).** A Space bound to an App (K31's
//! `tool_app_spaces`) is `sew`-governed until the owner or an Admin, in
//! person, switches it to `app` (`k2 hostchat app-space governance`). K2
//! keeps the wish in `tool_sew_settings` (subject `space:<space id>`, body
//! `{governance}`) and the S27 worker sends `POST
//! /mgmt/v1/spaces/{space_id}/settings` ([`super::home`] plans it). On `409
//! box_users_present` the worker hands the listed box users
//! ([`record_box_users_present`]) to K31's `space_member_remove` op and the
//! switch goes again once they are out ("leaving: box user"). Binding never
//! switches; an unbind forgets the wish (Sew resets to `sew`, AG-26).
//!
//! **The sync (S4, I4).** In a Space whose `app` governance Sew confirmed,
//! K2 computes every person's FULL private-channel list and level from the
//! App's manifest `chat` block (noted by the daemon, [`note_app_facts`]):
//! - people: principals with an enabled `kind=app` login grant on the App
//!   and an enabled `chat` member grant (their Sew account, provisioned
//!   first), never an agent-marked login, never a tied one, never a box
//!   user (the owner and Connect logins are not principals: rulings 1, 3);
//! - a person's private channels: the manifest channels whose `roles` hold
//!   their App role, or whose `rooms` meet their role's room lines, plus
//!   the App's own list for them ([`set_app_lists`], `chat.setChannels`);
//! - their level: the App's own level, else `levels[role]`, else
//!   `member`, clamped to the chat key's `max_level`;
//! - agents: a channel's `agents` (the App's own workspace or a room on one
//!   of its roles), private text channels only, always `member`, sent only
//!   once their own `coworker` member is confirmed.
//!
//! Each person is one outbox row (op `space_member_channels`, subject
//! `chan:<space id>:<ext>`, `after_subject` = their external id, so it
//! waits behind their own `provision`/`update` rows: `outbox::due_heads`).
//! The ledger (`tool_app_space_member_channels`) keeps the body K2 wants
//! and the one Sew confirmed; only a change is queued. The worker sends a
//! Space's due rows together (I4 bulk, ≤ 500) and reads each item:
//!
//! | Sew's answer for the person | K2 |
//! |---|---|
//! | applied | `synced` |
//! | `queued` (a job: over 1,000 changes) | `queued`; the next sweep sends it again to read it back |
//! | `skipped: owner` | `synced` (the owner is never touched) |
//! | `skipped: banned` | `held` for good: bans win (cleared only by [`clear_banned`]) |
//! | `skipped: deprovisioned` | `held` until their chat grant changes: never reactivated by a sync |
//! | `unknown_channel_ids` | `held` (reported); those channels are ensured again, once |
//! | 422 `member_not_provisioned` | provisioned again, then synced again ([`reprovision`]) |
//! | 409 `not_app_governed` | retried with backoff |
//! | 422 `box_user_not_allowed` / `level_above_ceiling` / `agent_level` | `held`, logged loudly (a K2 bug) |
//!
//! **Channels (I6).** The manifest's channels are `ensure`d in each
//! governed Space (op `channels_bulk`, subject `chans:<space id>`); a
//! changed name is a `rename`, a channel dropped from the manifest is
//! `archive`d (never deleted), one listed again is `unarchive`d. Slug → id
//! lives in `tool_app_space_channels`; member rows wait until every private
//! channel of the Space has its id.
//!
//! **Service owner (§3a).** App Spaces are owned by `app:<app_id>`. When an
//! App is removed, its service owner is deprovisioned after its Spaces'
//! unbinds are sent ([`want_service_owner_retired`]).
//!
//! **The chat key (S1) seam.** The sync runs only for a keyed App: an
//! enabled row in `app_chat_keys` (S1 owns that table; [`chat_key`] reads
//! it when it exists). Its `max_level` is the level ceiling.
//!
//! Tables live in `skin.db` (idempotent DDL like `tool_outbox`), so this
//! needs no main-DB migration.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::sew_wire::{
    self, ChannelOp, ChannelsBulkRequest, ChannelsBulkResponse, ExternalId, MemberChannelsRequest,
    MemberChannelsResponse, GOVERNANCE_APP, GOVERNANCE_SEW,
};
use super::{features, outbox, Cause, NewOp, Op, CHAT_KIND};
use crate::deploy::manifest::{ChatBlock, ChatChannel};

/// Outbox subject prefix of a person's channel sync row.
pub const CHAN_PREFIX: &str = "chan:";
/// Outbox subject prefix of a Space's `channels_bulk` rows.
pub const CHANS_PREFIX: &str = "chans:";
/// `tool_sew_settings` (and outbox) subject prefix of a Space's governance.
pub const SPACE_PREFIX: &str = "space:";

/// Member ledger states.
pub const PENDING: &str = "pending";
pub const SYNCED: &str = "synced";
pub const HELD: &str = "held";
pub const QUEUED: &str = "queued";

/// Held reasons ([`after_held`]). Banned is sticky.
pub const WHY_BANNED: &str = "skipped: banned in this Space";
pub const WHY_DEPROVISIONED: &str = "skipped: deprovisioned in chat (a sync never reactivates anyone)";

/// Channel states.
pub const CH_WANTED: &str = "wanted";
pub const CH_READY: &str = "ready";
pub const CH_ARCHIVED: &str = "archived";
pub const CH_FAILED: &str = "failed";

/// Default `max_level` of a chat key (PRD §2.2).
pub const DEFAULT_MAX_LEVEL: &str = "moderator";

/// Idempotent DDL, called from `skin::open_db` beside `tool_outbox`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_app_chat_facts (
            app_id     TEXT PRIMARY KEY NOT NULL,
            facts      TEXT NOT NULL,
            updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tool_app_space_channels (
            space_id     TEXT NOT NULL,
            slug         TEXT NOT NULL,
            app_id       TEXT NOT NULL,
            source       TEXT NOT NULL CHECK (source IN ('manifest','app')),
            channel_id   TEXT,
            name         TEXT NOT NULL,
            sew_name     TEXT,
            kind         TEXT NOT NULL,
            private      INTEGER NOT NULL,
            user_limit   INTEGER,
            state        TEXT NOT NULL CHECK (state IN ('wanted','ready','archived','failed')),
            why          TEXT,
            ensure_tries INTEGER NOT NULL DEFAULT 0,
            updated_at   INTEGER NOT NULL,
            PRIMARY KEY (space_id, slug)
         );
         CREATE TABLE IF NOT EXISTS tool_app_space_member_channels (
            space_id    TEXT NOT NULL,
            external_id TEXT NOT NULL,
            app_id      TEXT NOT NULL,
            body        TEXT NOT NULL,
            body_hash   TEXT NOT NULL,
            sent_hash   TEXT,
            state       TEXT NOT NULL CHECK (state IN ('pending','synced','held','queued')),
            why         TEXT,
            grant_rev   TEXT,
            updated_at  INTEGER NOT NULL,
            PRIMARY KEY (space_id, external_id)
         );
         CREATE TABLE IF NOT EXISTS tool_app_member_lists (
            app_id       TEXT NOT NULL,
            principal_id TEXT NOT NULL,
            space_id     TEXT NOT NULL,
            slugs        TEXT NOT NULL,
            level        TEXT,
            updated_at   INTEGER NOT NULL,
            updated_by   TEXT NOT NULL,
            PRIMARY KEY (app_id, principal_id, space_id)
         );
         CREATE TABLE IF NOT EXISTS tool_app_space_leaving (
            space_id    TEXT NOT NULL,
            external_id TEXT NOT NULL,
            app_id      TEXT NOT NULL,
            state       TEXT NOT NULL CHECK (state IN ('removing','gone')),
            at          INTEGER NOT NULL,
            PRIMARY KEY (space_id, external_id)
         );
         CREATE TABLE IF NOT EXISTS tool_app_service_retire (
            app_id    TEXT PRIMARY KEY NOT NULL,
            wanted_at INTEGER NOT NULL,
            wanted_by TEXT NOT NULL,
            queued_at INTEGER
         );",
    )
    .map_err(|e| format!("skin.db governed spaces schema: {e}"))
}

fn sha_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn flag_on(conn: &Connection) -> Result<bool, String> {
    features::has(conn, CHAT_KIND, features::APP_GOVERNED_SPACES)
}

// ── the chat key (S1's table, read only) ─────────────────────────────

/// What the sync needs from an App's chat key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    pub enabled: bool,
    pub max_level: String,
    pub max_members: i64,
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", params![name], |r| r.get(0))
        .map_err(|e| format!("skin.db schema read: {e}"))
}

/// The App's chat key, when S1's `app_chat_keys` table exists and holds a
/// row for it (`None`: no key). A revoked key reads `enabled: false`.
pub fn chat_key(conn: &Connection, app_id: &str) -> Result<Option<KeyView>, String> {
    if !table_exists(conn, "app_chat_keys")? {
        return Ok(None);
    }
    conn.query_row(
        "SELECT enabled, max_level, max_members, revoked_at FROM app_chat_keys WHERE service_id = ?1",
        params![app_id],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        },
    )
    .optional()
    .map_err(|e| format!("chat key read: {e}"))
    .map(|o| {
        o.map(|(enabled, level, max, revoked)| KeyView {
            enabled: enabled == 1 && revoked.is_none(),
            max_level: level.filter(|l| sew_wire::level_rank(l).is_some()).unwrap_or_else(|| DEFAULT_MAX_LEVEL.into()),
            max_members: max.unwrap_or(500),
        })
    })
}

fn keyed(conn: &Connection, app_id: &str) -> Result<Option<KeyView>, String> {
    Ok(chat_key(conn, app_id)?.filter(|k| k.enabled))
}

/// `level` clamped to `ceiling` (both levels; an unknown level is
/// `member`).
pub fn clamp_level(level: &str, ceiling: &str) -> String {
    let l = sew_wire::level_rank(level).unwrap_or(0);
    let c = sew_wire::level_rank(ceiling).unwrap_or(0);
    sew_wire::LEVELS[l.min(c)].to_string()
}

/// `grants.created_by` of a chat grant K2 wrote for a keyed App's user:
/// `chat-key:<service id>` (PRD §5.3).
pub const AUTO_GRANT_PREFIX: &str = "chat-key:";

fn keyed_apps(conn: &Connection) -> Result<Vec<(String, KeyView)>, String> {
    if !table_exists(conn, "app_chat_keys")? {
        return Ok(Vec::new());
    }
    let mut st = conn.prepare("SELECT service_id FROM app_chat_keys ORDER BY service_id").map_err(|e| format!("chat keys: {e}"))?;
    let ids: Vec<String> = st
        .query_map([], |r| r.get(0))
        .map_err(|e| format!("chat keys: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("chat keys row: {e}"))?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(k) = keyed(conn, &id)? {
            out.push((id, k));
        }
    }
    Ok(out)
}

/// Members first (PRD §5.3): `(principal, app)` pairs that need a `chat`
/// member grant K2 writes, and the ids of grants K2 wrote that should go.
/// Never an agent-marked login; a tied person gets their member (they get
/// no pass and no channels until the tie drops). Within each key's
/// `max_members`. A grant goes only when the person logs in to no keyed
/// App any more and has no assistant agents in chat.
pub fn plan_auto_grants(conn: &Connection) -> Result<(Vec<(String, String)>, Vec<String>), String> {
    let apps = keyed_apps(conn)?;
    let keyed_ids: BTreeSet<String> = apps.iter().map(|(a, _)| a.clone()).collect();
    let mut add = Vec::new();
    for (app, key) in &apps {
        let made: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE kind = 'chat' AND created_by = ?1",
                params![format!("{AUTO_GRANT_PREFIX}{app}")],
                |r| r.get(0),
            )
            .map_err(|e| format!("auto grants count: {e}"))?;
        let mut room = (key.max_members - made).max(0);
        let mut st = conn
            .prepare(
                "SELECT a.subject_id FROM grants a JOIN principals p ON p.id = a.subject_id
                 WHERE a.kind = 'app' AND a.target_id = ?1 AND a.subject_kind = 'principal' AND a.enabled = 1
                   AND (p.agent_workspace IS NULL OR trim(p.agent_workspace) = '')
                   AND NOT EXISTS (SELECT 1 FROM grants c WHERE c.kind = 'chat' AND c.subject_kind = 'principal' AND c.subject_id = a.subject_id)
                 ORDER BY a.created_at, a.subject_id",
            )
            .map_err(|e| format!("auto grants: {e}"))?;
        let rows: Vec<String> = st
            .query_map(params![app], |r| r.get(0))
            .map_err(|e| format!("auto grants: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("auto grants row: {e}"))?;
        for p in rows {
            if room == 0 {
                crate::log_debug!("[grant-hook] App {app}: chat key max_members reached; {p} gets no chat yet");
                break;
            }
            if add.iter().any(|(q, _): &(String, String)| *q == p) {
                continue;
            }
            add.push((p, app.clone()));
            room -= 1;
        }
    }
    let mut drop = Vec::new();
    let mut st = conn
        .prepare("SELECT id, subject_id FROM grants WHERE kind = 'chat' AND subject_kind = 'principal' AND created_by LIKE 'chat-key:%'")
        .map_err(|e| format!("auto grants drop: {e}"))?;
    let made: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| format!("auto grants drop: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("auto grants drop row: {e}"))?;
    for (gid, principal) in made {
        let mut st = conn
            .prepare(
                "SELECT a.target_id FROM grants a JOIN principals p ON p.id = a.subject_id
                 WHERE a.kind = 'app' AND a.subject_kind = 'principal' AND a.subject_id = ?1 AND a.enabled = 1
                   AND (p.agent_workspace IS NULL OR trim(p.agent_workspace) = '')",
            )
            .map_err(|e| format!("auto grants logins: {e}"))?;
        let logins: Vec<String> = st
            .query_map(params![principal], |r| r.get(0))
            .map_err(|e| format!("auto grants logins: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("auto grants logins row: {e}"))?;
        if logins.iter().any(|a| keyed_ids.contains(a)) {
            continue;
        }
        let assistants: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE kind = 'chat' AND subject_kind = 'workspace' AND scope = ?1",
                params![format!("assistant:{principal}")],
                |r| r.get(0),
            )
            .map_err(|e| format!("auto grants assistants: {e}"))?;
        if assistants > 0 {
            continue;
        }
        drop.push(gid);
    }
    Ok((add, drop))
}

// ── App facts the daemon notes (main DB + manifest) ──────────────────

/// What the sync needs from outside skin.db about one App: its live
/// manifest's `chat` block, its own workspace, and the handles the block
/// names, resolved to workspace ids (an unknown handle is absent).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppFacts {
    pub app_id: String,
    #[serde(default)]
    pub owner_workspace: Option<String>,
    #[serde(default)]
    pub block: Option<ChatBlock>,
    #[serde(default)]
    pub handles: BTreeMap<String, String>,
}

/// Record the facts of `apps` (the daemon calls it before each sweep, from
/// each bound App's live Seed). Returns how many changed.
pub fn note_app_facts(conn: &Connection, apps: &[AppFacts], now: i64) -> Result<usize, String> {
    let mut n = 0;
    for f in apps {
        let raw = serde_json::to_string(f).map_err(|e| format!("app chat facts: {e}"))?;
        let before: Option<String> = conn
            .query_row("SELECT facts FROM tool_app_chat_facts WHERE app_id = ?1", params![f.app_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("app chat facts read: {e}"))?;
        if before.as_deref() == Some(raw.as_str()) {
            continue;
        }
        conn.execute(
            "INSERT INTO tool_app_chat_facts (app_id, facts, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(app_id) DO UPDATE SET facts = excluded.facts, updated_at = excluded.updated_at",
            params![f.app_id, raw, now],
        )
        .map_err(|e| format!("app chat facts write: {e}"))?;
        n += 1;
    }
    Ok(n)
}

pub fn facts_of(conn: &Connection, app_id: &str) -> Result<Option<AppFacts>, String> {
    let raw: Option<String> = conn
        .query_row("SELECT facts FROM tool_app_chat_facts WHERE app_id = ?1", params![app_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("app chat facts read: {e}"))?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

// ── governance (S5, I3) ──────────────────────────────────────────────

pub fn settings_subject(space_id: &str) -> String {
    format!("{SPACE_PREFIX}{space_id}")
}

/// The Space of a `space:<id>` settings subject (never a K31 member row's
/// `space:<id>:<ext>`).
pub fn space_of_settings(subject: &str) -> Option<&str> {
    subject.strip_prefix(SPACE_PREFIX).filter(|s| !s.contains(':'))
}

fn settings_bodies(conn: &Connection, space_id: &str) -> Result<Option<(Value, Option<Value>)>, String> {
    let r: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT body, sent_body FROM tool_sew_settings WHERE subject = ?1",
            params![settings_subject(space_id)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("governance read: {e}"))?;
    Ok(r.map(|(b, s)| (serde_json::from_str(&b).unwrap_or(Value::Null), s.and_then(|s| serde_json::from_str(&s).ok()))))
}

/// The governance K2 wants for a Space (`None`: never switched here).
pub fn governance_wish(conn: &Connection, space_id: &str) -> Result<Option<String>, String> {
    Ok(settings_bodies(conn, space_id)?
        .and_then(|(b, _)| b.get("governance").and_then(Value::as_str).map(str::to_string)))
}

/// The governance Sew last confirmed for a Space (`None`: nothing sent).
pub fn governance_confirmed(conn: &Connection, space_id: &str) -> Result<Option<String>, String> {
    Ok(settings_bodies(conn, space_id)?
        .and_then(|(_, s)| s)
        .and_then(|s| s.get("governance").and_then(Value::as_str).map(str::to_string)))
}

/// K2 wants `app` AND Sew confirmed `app`: the sync runs.
pub fn is_governed(conn: &Connection, space_id: &str) -> Result<bool, String> {
    Ok(governance_wish(conn, space_id)?.as_deref() == Some(GOVERNANCE_APP)
        && governance_confirmed(conn, space_id)?.as_deref() == Some(GOVERNANCE_APP))
}

/// K2 wants `app` (sent or not): K2 refuses adding box users there.
pub fn wants_app(conn: &Connection, space_id: &str) -> Result<bool, String> {
    Ok(governance_wish(conn, space_id)?.as_deref() == Some(GOVERNANCE_APP))
}

/// Store the owner's switch for a bound Space. Returns whether it changed.
/// The caller checked the person gate, the binding, the key and the flag.
pub fn set_governance(conn: &Connection, space_id: &str, value: &str, actor: &str, now: i64) -> Result<bool, String> {
    if value != GOVERNANCE_APP && value != GOVERNANCE_SEW {
        return Err(format!("governance is app or sew, not '{value}'"));
    }
    let subject = settings_subject(space_id);
    let body = json!({ "governance": value }).to_string();
    let before: Option<String> = conn
        .query_row("SELECT body FROM tool_sew_settings WHERE subject = ?1", params![subject], |r| r.get(0))
        .optional()
        .map_err(|e| format!("governance read: {e}"))?;
    if before.as_deref().and_then(|b| serde_json::from_str::<Value>(b).ok()) == Some(json!({ "governance": value })) {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO tool_sew_settings (subject, body, sent_body, updated_at, updated_by) VALUES (?1, ?2, NULL, ?3, ?4)
         ON CONFLICT(subject) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        params![subject, body, now, actor],
    )
    .map_err(|e| format!("governance write: {e}"))?;
    if value == GOVERNANCE_SEW {
        // Back to Sew's own rules: K2 stops managing private channels there.
        forget_members(conn, space_id)?;
    }
    Ok(true)
}

/// The part of a governance body Sew can take now (`None`: Sew lacks
/// `app_governed_spaces`, or the body names none).
pub fn sendable(body: &serde_json::Map<String, Value>, flags: &[String]) -> Option<Value> {
    if !flags.iter().any(|f| f == features::APP_GOVERNED_SPACES) {
        return None;
    }
    let g = body.get("governance").and_then(Value::as_str)?;
    Some(json!({ "governance": g }))
}

fn forget_members(conn: &Connection, space_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM tool_app_space_member_channels WHERE space_id = ?1", params![space_id])
        .map(|_| ())
        .map_err(|e| format!("member channels forget: {e}"))
}

/// The Space was unbound (or its App removed): Sew resets its governance
/// to `sew` (AG-26), so K2 forgets its wish, its member ledger, and the
/// box users it was removing there. Channel ids are kept: a rebind ensures
/// the same slugs and gets the same channels.
pub fn forget_space(conn: &Connection, space_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM tool_sew_settings WHERE subject = ?1", params![settings_subject(space_id)])
        .map_err(|e| format!("governance forget: {e}"))?;
    conn.execute("DELETE FROM tool_app_space_leaving WHERE space_id = ?1", params![space_id])
        .map_err(|e| format!("leaving forget: {e}"))?;
    forget_members(conn, space_id)
}

/// A new Sew install has none of the old Spaces' state.
pub fn forget_all(conn: &Connection) -> Result<(), String> {
    for t in ["tool_app_space_channels", "tool_app_space_member_channels", "tool_app_space_leaving"] {
        conn.execute(&format!("DELETE FROM {t}"), []).map_err(|e| format!("{t} reset: {e}"))?;
    }
    Ok(())
}

/// Is any K31 membership row of `space_id` open (the box users leaving)?
pub fn space_rows_busy(busy: &HashSet<String>, space_id: &str) -> bool {
    let p = format!("{}{space_id}:", super::app_space::SUBJECT_PREFIX);
    busy.iter().any(|k| k.starts_with(&p))
}

/// The switch answered `409 box_users_present`: queue K31's remove for
/// each listed box user (never anyone else), record them as leaving, and
/// leave the switch unsent so the next sweep sends it again once they are
/// out. MUST run inside the caller's transaction. Returns the ids queued.
pub fn plan_box_user_removes(conn: &Connection, space_id: &str, ids: &[String], now: i64) -> Result<Vec<NewOp>, String> {
    let app = super::app_space::bound_app(conn, space_id)?;
    let mut ops = Vec::new();
    for raw in ids {
        let Ok(ext) = ExternalId::parse(raw) else {
            crate::log_debug!("[grant-hook] box_users_present named {raw:?}, not an external id; ignored");
            continue;
        };
        if !ext.is_box_user() {
            crate::log_debug!("[grant-hook] box_users_present named {ext}, not a box user; K2 removes only box users");
            continue;
        }
        let body = sew_wire::SpaceMemberRemoveRequest { space_id: space_id.to_string(), app_id: app.clone(), external_id: ext.clone() };
        ops.push(NewOp {
            tool: CHAT_KIND,
            op: Op::SpaceMemberRemove,
            subject_key: super::app_space::subject_key(space_id, &ext),
            payload: serde_json::to_value(&body).map_err(|e| format!("box user remove body: {e}"))?,
            grant_id: None,
            grant_rev: None,
            cause: Cause::GovernanceSwitch,
            after_subject: None,
        });
        conn.execute(
            "INSERT INTO tool_app_space_leaving (space_id, external_id, app_id, state, at) VALUES (?1, ?2, ?3, 'removing', ?4)
             ON CONFLICT(space_id, external_id) DO UPDATE SET state = 'removing', at = excluded.at",
            params![space_id, ext.to_string(), app.clone().unwrap_or_default(), now],
        )
        .map_err(|e| format!("leaving write: {e}"))?;
    }
    Ok(ops)
}

/// A delivered `space_member_remove` of a box user leaving: mark it gone.
/// Returns whether it was one.
pub fn after_box_user_removed(conn: &Connection, space_id: &str, ext: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE tool_app_space_leaving SET state = 'gone', at = ?3 WHERE space_id = ?1 AND external_id = ?2 AND state = 'removing'",
            params![space_id, ext, now],
        )
        .map_err(|e| format!("leaving update: {e}"))?;
    Ok(n > 0)
}

// ── the service owner (§3a) ──────────────────────────────────────────

/// An App was removed: deprovision its service owner `app:<app_id>` once
/// its Spaces' unbinds are sent (see [`plan_service_retire`]).
pub fn want_service_owner_retired(conn: &Connection, app_id: &str, actor: &str, now: i64) -> Result<(), String> {
    if ExternalId::app_service(app_id).is_err() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO tool_app_service_retire (app_id, wanted_at, wanted_by, queued_at) VALUES (?1, ?2, ?3, NULL)
         ON CONFLICT(app_id) DO UPDATE SET wanted_at = excluded.wanted_at, wanted_by = excluded.wanted_by, queued_at = NULL",
        params![app_id, now, actor],
    )
    .map(|_| ())
    .map_err(|e| format!("service owner retire: {e}"))
}

/// The deprovisions due now: Sew reports the flag, and none of the App's
/// unbinds is still unsent. Marks them queued. MUST run inside the
/// caller's transaction; returns `(op, actor)`.
pub fn plan_service_retire(conn: &Connection, busy: &HashSet<String>, now: i64) -> Result<Vec<(NewOp, String)>, String> {
    if !flag_on(conn)? {
        return Ok(Vec::new());
    }
    let mut st = conn
        .prepare("SELECT app_id, wanted_by FROM tool_app_service_retire WHERE queued_at IS NULL ORDER BY app_id")
        .map_err(|e| format!("service retire read: {e}"))?;
    let wishes: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| format!("service retire read: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("service retire row: {e}"))?;
    let mut out = Vec::new();
    for (app, actor) in wishes {
        let open_unbinds: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_app_space_unbinds WHERE app_id = ?1 AND done_at IS NULL", params![app], |r| r.get(0))
            .map_err(|e| format!("service retire unbinds: {e}"))?;
        let ext = ExternalId::app_service(&app)?;
        if open_unbinds > 0 || busy.contains(&ext.to_string()) {
            continue;
        }
        out.push((
            NewOp {
                tool: CHAT_KIND,
                op: Op::Deprovision,
                subject_key: ext.to_string(),
                payload: serde_json::to_value(sew_wire::DeprovisionRequest { external_id: ext.clone() })
                    .map_err(|e| format!("service retire body: {e}"))?,
                grant_id: None,
                grant_rev: None,
                cause: Cause::AppRemoved,
                after_subject: None,
            },
            actor,
        ));
        conn.execute("UPDATE tool_app_service_retire SET queued_at = ?2 WHERE app_id = ?1", params![app, now])
            .map_err(|e| format!("service retire mark: {e}"))?;
    }
    Ok(out)
}

// ── channels (I6) ────────────────────────────────────────────────────

/// One channel K2 tracks in a governed Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRow {
    pub space_id: String,
    pub slug: String,
    pub app_id: String,
    /// `manifest` | `app` (made by the App's own call).
    pub source: String,
    pub channel_id: Option<String>,
    pub name: String,
    pub sew_name: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub private: bool,
    pub user_limit: Option<i64>,
    pub state: String,
    pub why: Option<String>,
    pub ensure_tries: i64,
}

fn channel_rows(conn: &Connection, space_id: &str) -> Result<Vec<ChannelRow>, String> {
    let mut st = conn
        .prepare(
            "SELECT space_id, slug, app_id, source, channel_id, name, sew_name, kind, private, user_limit, state, why, ensure_tries
             FROM tool_app_space_channels WHERE space_id = ?1 ORDER BY slug",
        )
        .map_err(|e| format!("channels read: {e}"))?;
    let rows = st
        .query_map(params![space_id], |r| {
            Ok(ChannelRow {
                space_id: r.get(0)?,
                slug: r.get(1)?,
                app_id: r.get(2)?,
                source: r.get(3)?,
                channel_id: r.get(4)?,
                name: r.get(5)?,
                sew_name: r.get(6)?,
                kind: r.get(7)?,
                private: r.get::<_, i64>(8)? == 1,
                user_limit: r.get(9)?,
                state: r.get(10)?,
                why: r.get(11)?,
                ensure_tries: r.get(12)?,
            })
        })
        .map_err(|e| format!("channels read: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("channels row: {e}"))
}

/// The channels a manifest names for one governed Space of its App:
/// those naming the Space, plus those naming none when the App governs
/// exactly one Space.
pub fn manifest_channels_for<'a>(block: &'a ChatBlock, space_id: &str, governed_count: usize) -> Vec<&'a ChatChannel> {
    block
        .channels
        .iter()
        .filter(|c| match c.space.as_deref() {
            Some(s) => s == space_id,
            None => governed_count == 1,
        })
        .collect()
}

/// The governed Spaces K2 syncs now, with their App: bound, K2 wants
/// `app`, Sew confirmed it, the App is keyed. Sorted by Space.
pub fn governed_spaces(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for b in super::app_space::bindings(conn)? {
        if is_governed(conn, &b.space_id)? && keyed(conn, &b.app_id)?.is_some() {
            out.push((b.space_id, b.app_id));
        }
    }
    Ok(out)
}

fn upsert_manifest_channel(conn: &Connection, space_id: &str, app_id: &str, c: &ChatChannel, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO tool_app_space_channels
            (space_id, slug, app_id, source, channel_id, name, sew_name, kind, private, user_limit, state, why, ensure_tries, updated_at)
         VALUES (?1, ?2, ?3, 'manifest', NULL, ?4, NULL, ?5, ?6, ?7, 'wanted', NULL, 0, ?8)
         ON CONFLICT(space_id, slug) DO UPDATE SET app_id = excluded.app_id, source = 'manifest', name = excluded.name,
            private = excluded.private, user_limit = excluded.user_limit, updated_at = excluded.updated_at",
        params![space_id, c.slug, app_id, c.name.trim(), c.kind, c.private as i64, c.user_limit.map(i64::from), now],
    )
    .map(|_| ())
    .map_err(|e| format!("channel write: {e}"))
}

fn set_channel_state(conn: &Connection, space_id: &str, slug: &str, state: &str, why: Option<&str>, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_app_space_channels SET state = ?3, why = ?4, updated_at = ?5 WHERE space_id = ?1 AND slug = ?2",
        params![space_id, slug, state, why, now],
    )
    .map(|_| ())
    .map_err(|e| format!("channel state: {e}"))
}

/// Plan the I6 ops of every governed Space whose manifest channels differ
/// from what Sew holds. MUST run inside the caller's transaction.
pub fn plan_channels(conn: &Connection, busy: &HashSet<String>, now: i64) -> Result<Vec<NewOp>, String> {
    if !flag_on(conn)? {
        return Ok(Vec::new());
    }
    let governed = governed_spaces(conn)?;
    let mut per_app: HashMap<String, usize> = HashMap::new();
    for (_, a) in &governed {
        *per_app.entry(a.clone()).or_default() += 1;
    }
    let mut out = Vec::new();
    for (space, app) in &governed {
        let subject = format!("{CHANS_PREFIX}{space}");
        if busy.contains(&subject) {
            continue;
        }
        let facts = facts_of(conn, app)?.unwrap_or_default();
        let block = facts.block.clone().unwrap_or_default();
        let wanted = manifest_channels_for(&block, space, per_app[app]);
        let have: HashMap<String, ChannelRow> = channel_rows(conn, space)?.into_iter().map(|r| (r.slug.clone(), r)).collect();
        let mut ops: Vec<ChannelOp> = Vec::new();
        let wanted_slugs: BTreeSet<&str> = wanted.iter().map(|c| c.slug.as_str()).collect();
        for c in &wanted {
            match have.get(&c.slug) {
                Some(r) if r.kind != c.kind && r.channel_id.is_some() => {
                    // Sew never changes a channel's type (`channel_type_mismatch`).
                    let why = format!("the manifest changed '{}' from {} to {}; Sew keeps a channel's type (make a new slug)", c.slug, r.kind, c.kind);
                    if r.state != CH_FAILED || r.why.as_deref() != Some(why.as_str()) {
                        set_channel_state(conn, space, &c.slug, CH_FAILED, Some(&why), now)?;
                    }
                    continue;
                }
                Some(r) if r.state == CH_FAILED => continue,
                Some(r) if r.state == CH_ARCHIVED && r.channel_id.is_some() => {
                    upsert_manifest_channel(conn, space, app, c, now)?;
                    ops.push(ChannelOp {
                        op: "unarchive".into(),
                        slug: Some(c.slug.clone()),
                        channel_id: r.channel_id.clone(),
                        name: None,
                        kind: None,
                        private: None,
                        user_limit: None,
                    });
                }
                Some(r) if r.channel_id.is_some() && r.state == CH_READY => {
                    upsert_manifest_channel(conn, space, app, c, now)?;
                    if r.sew_name.as_deref() != Some(c.name.trim()) {
                        ops.push(ChannelOp {
                            op: "rename".into(),
                            slug: Some(c.slug.clone()),
                            channel_id: r.channel_id.clone(),
                            name: Some(c.name.trim().to_string()),
                            kind: None,
                            private: None,
                            user_limit: None,
                        });
                    }
                }
                _ => {
                    upsert_manifest_channel(conn, space, app, c, now)?;
                    ops.push(ChannelOp {
                        op: "ensure".into(),
                        slug: Some(c.slug.clone()),
                        channel_id: None,
                        name: Some(c.name.trim().to_string()),
                        kind: Some(c.kind.clone()),
                        private: Some(c.private),
                        user_limit: c.user_limit,
                    });
                }
            }
        }
        // Dropped from the manifest: archived (history kept), never deleted.
        for r in have.values() {
            if r.source == "manifest" && r.state == CH_READY && !wanted_slugs.contains(r.slug.as_str()) {
                if let Some(id) = &r.channel_id {
                    ops.push(ChannelOp {
                        op: "archive".into(),
                        slug: Some(r.slug.clone()),
                        channel_id: Some(id.clone()),
                        name: None,
                        kind: None,
                        private: None,
                        user_limit: None,
                    });
                }
            }
        }
        for chunk in ops.chunks(sew_wire::CHANNELS_BULK_MAX) {
            let body = ChannelsBulkRequest { space_id: space.clone(), app_id: app.clone(), ops: chunk.to_vec() };
            out.push(NewOp {
                tool: CHAT_KIND,
                op: Op::ChannelsBulk,
                subject_key: subject.clone(),
                payload: serde_json::to_value(&body).map_err(|e| format!("channels body: {e}"))?,
                grant_id: None,
                grant_rev: None,
                cause: Cause::Reconcile,
                after_subject: None,
            });
        }
    }
    Ok(out)
}

/// A delivered `channels_bulk` row: record each op's answer (slug → id).
/// Ensures that failed once more are tried again once, then held.
pub fn after_done_channels(conn: &Connection, row_id: &str, result: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, row_id)? else { return Ok(()) };
    if Op::parse(&row.op) != Some(Op::ChannelsBulk) {
        return Ok(());
    }
    let Ok(req) = serde_json::from_str::<ChannelsBulkRequest>(&row.payload_raw) else { return Ok(()) };
    let resp: ChannelsBulkResponse = serde_json::from_str(result).unwrap_or_default();
    for (i, op) in req.ops.iter().enumerate() {
        let Some(slug) = op.slug.as_deref() else { continue };
        let res = resp.results.get(i);
        match res.and_then(|r| r.error.as_ref()) {
            Some(e) => {
                let why = format!("Sew refused {} of '{slug}': {}", op.op, e.reason());
                crate::log_debug!("[grant-hook] {why}");
                set_channel_state(conn, &req.space_id, slug, CH_FAILED, Some(&why), now)?;
            }
            None => {
                let Some(r) = res else {
                    set_channel_state(conn, &req.space_id, slug, CH_FAILED, Some("Sew's answer named no result for this op"), now)?;
                    continue;
                };
                match op.op.as_str() {
                    "ensure" => {
                        let Some(id) = r.channel_id.as_deref().filter(|i| sew_wire::is_sew_id(i)) else {
                            set_channel_state(conn, &req.space_id, slug, CH_FAILED, Some("Sew's ensure named no channel id"), now)?;
                            continue;
                        };
                        // Sew's ensure never updates; K2 renames next if the
                        // name it holds is another one.
                        conn.execute(
                            "UPDATE tool_app_space_channels SET channel_id = ?3, sew_name = ?4, state = 'ready', why = NULL,
                                ensure_tries = ensure_tries + 1, updated_at = ?5 WHERE space_id = ?1 AND slug = ?2",
                            params![req.space_id, slug, id, r.name.clone().or_else(|| op.name.clone()), now],
                        )
                        .map_err(|e| format!("channel ensure record: {e}"))?;
                    }
                    "rename" => {
                        conn.execute(
                            "UPDATE tool_app_space_channels SET sew_name = ?3, updated_at = ?4 WHERE space_id = ?1 AND slug = ?2",
                            params![req.space_id, slug, op.name, now],
                        )
                        .map_err(|e| format!("channel rename record: {e}"))?;
                    }
                    "archive" => set_channel_state(conn, &req.space_id, slug, CH_ARCHIVED, None, now)?,
                    "unarchive" => set_channel_state(conn, &req.space_id, slug, CH_READY, None, now)?,
                    _ => {}
                }
            }
        }
    }
    super::kick_reconcile();
    Ok(())
}

/// Sew didn't know these channel ids (a channel removed outside K2): K2
/// ensures each slug once more; a second miss holds it (`failed`).
pub fn forget_unknown_channels(conn: &Connection, space_id: &str, ids: &[String], now: i64) -> Result<(), String> {
    for id in ids {
        conn.execute(
            "UPDATE tool_app_space_channels
                SET channel_id = CASE WHEN ensure_tries < 2 THEN NULL ELSE channel_id END,
                    state = CASE WHEN ensure_tries < 2 THEN 'wanted' ELSE 'failed' END,
                    why = CASE WHEN ensure_tries < 2 THEN NULL ELSE 'Sew no longer knows this channel (ensured twice)' END,
                    updated_at = ?3
              WHERE space_id = ?1 AND channel_id = ?2",
            params![space_id, id, now],
        )
        .map_err(|e| format!("channel forget: {e}"))?;
    }
    Ok(())
}

/// Record channels the App made itself (`chat.channels`, Sew's I6 answer).
pub fn note_app_channels(conn: &Connection, space_id: &str, app_id: &str, req: &ChannelsBulkRequest, resp: &ChannelsBulkResponse, now: i64) -> Result<(), String> {
    for (i, op) in req.ops.iter().enumerate() {
        let (Some(slug), Some(r)) = (op.slug.as_deref(), resp.results.get(i)) else { continue };
        if r.error.is_some() {
            continue;
        }
        match op.op.as_str() {
            "ensure" => {
                let Some(id) = r.channel_id.as_deref().filter(|i| sew_wire::is_sew_id(i)) else { continue };
                conn.execute(
                    "INSERT INTO tool_app_space_channels
                        (space_id, slug, app_id, source, channel_id, name, sew_name, kind, private, user_limit, state, why, ensure_tries, updated_at)
                     VALUES (?1, ?2, ?3, 'app', ?4, ?5, ?5, ?6, ?7, ?8, 'ready', NULL, 1, ?9)
                     ON CONFLICT(space_id, slug) DO UPDATE SET channel_id = excluded.channel_id, state = 'ready', why = NULL, updated_at = excluded.updated_at",
                    params![
                        space_id,
                        slug,
                        app_id,
                        id,
                        r.name.clone().or_else(|| op.name.clone()).unwrap_or_else(|| slug.to_string()),
                        op.kind.clone().unwrap_or_else(|| "text".into()),
                        op.private.unwrap_or(false) as i64,
                        op.user_limit.map(i64::from),
                        now
                    ],
                )
                .map_err(|e| format!("app channel record: {e}"))?;
            }
            "archive" => set_channel_state(conn, space_id, slug, CH_ARCHIVED, None, now)?,
            "unarchive" => set_channel_state(conn, space_id, slug, CH_READY, None, now)?,
            "rename" => {
                conn.execute(
                    "UPDATE tool_app_space_channels SET name = ?3, sew_name = ?3, updated_at = ?4 WHERE space_id = ?1 AND slug = ?2",
                    params![space_id, slug, op.name, now],
                )
                .map_err(|e| format!("app channel rename: {e}"))?;
            }
            _ => {}
        }
    }
    super::kick_reconcile();
    Ok(())
}

/// The channel id of `slug` in `space_id`, when K2 knows it.
pub fn channel_id(conn: &Connection, space_id: &str, slug: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT channel_id FROM tool_app_space_channels WHERE space_id = ?1 AND slug = ?2",
        params![space_id, slug],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map(|o| o.flatten())
    .map_err(|e| format!("channel read: {e}"))
}

// ── the App's own lists (`chat.setChannels`) ─────────────────────────

/// One person's App-managed list in one Space: private channel slugs and
/// an optional level (it replaces the role's level, clamped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppList {
    pub principal_id: String,
    pub space_id: String,
    pub slugs: Vec<String>,
    pub level: Option<String>,
}

/// Set people's App-managed lists in full (each replaces that person's
/// list; an empty list with no level clears it). Returns how many changed.
pub fn set_app_lists(conn: &Connection, app_id: &str, lists: &[AppList], actor: &str, now: i64) -> Result<usize, String> {
    let mut n = 0;
    for l in lists {
        let mut slugs: Vec<String> = l.slugs.clone();
        slugs.sort();
        slugs.dedup();
        let before: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT slugs, level FROM tool_app_member_lists WHERE app_id = ?1 AND principal_id = ?2 AND space_id = ?3",
                params![app_id, l.principal_id, l.space_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("app list read: {e}"))?;
        let raw = serde_json::to_string(&slugs).map_err(|e| e.to_string())?;
        if slugs.is_empty() && l.level.is_none() {
            if before.is_some() {
                conn.execute(
                    "DELETE FROM tool_app_member_lists WHERE app_id = ?1 AND principal_id = ?2 AND space_id = ?3",
                    params![app_id, l.principal_id, l.space_id],
                )
                .map_err(|e| format!("app list delete: {e}"))?;
                n += 1;
            }
            continue;
        }
        if before.as_ref() == Some(&(raw.clone(), l.level.clone())) {
            continue;
        }
        conn.execute(
            "INSERT INTO tool_app_member_lists (app_id, principal_id, space_id, slugs, level, updated_at, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(app_id, principal_id, space_id) DO UPDATE SET slugs = excluded.slugs, level = excluded.level,
                updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![app_id, l.principal_id, l.space_id, raw, l.level, now, actor],
        )
        .map_err(|e| format!("app list write: {e}"))?;
        n += 1;
    }
    Ok(n)
}

fn app_lists(conn: &Connection, app_id: &str, space_id: &str) -> Result<HashMap<String, (Vec<String>, Option<String>)>, String> {
    let mut st = conn
        .prepare("SELECT principal_id, slugs, level FROM tool_app_member_lists WHERE app_id = ?1 AND space_id = ?2")
        .map_err(|e| format!("app lists read: {e}"))?;
    let rows = st
        .query_map(params![app_id, space_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))
        .map_err(|e| format!("app lists read: {e}"))?;
    let mut out = HashMap::new();
    for row in rows {
        let (p, s, l) = row.map_err(|e| format!("app lists row: {e}"))?;
        out.insert(p, (serde_json::from_str(&s).unwrap_or_default(), l));
    }
    Ok(out)
}

// ── the wanted set (I4) ──────────────────────────────────────────────

/// One person (or agent) the sync wants in one governed Space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WantedSync {
    pub space_id: String,
    pub app_id: String,
    pub ext: ExternalId,
    pub body: MemberChannelsRequest,
    /// The chat grant's id (the outbox row's `grant_id`); `None` for an
    /// agent's coworker grant we don't track here.
    pub grant_id: Option<String>,
    /// `<chat grant id>:<scope>:<enabled>`: "the grant changed" for a
    /// deprovisioned hold.
    pub grant_rev: String,
}

/// Why a governed Space's people were not planned this round.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    /// Private manifest channels with no Sew id yet.
    pub channels: Vec<String>,
    /// Agents a channel names that are not provisioned in chat yet.
    pub agents: Vec<String>,
    /// Handles a channel names that this App may not put in its channels
    /// (not its own workspace, not a room on one of its roles), or that
    /// name no workspace.
    pub refused_agents: Vec<String>,
}

struct Person {
    principal: String,
    role: Option<String>,
    rooms: BTreeSet<String>,
    chat_grant: String,
    chat_rev: String,
}

fn people(conn: &Connection, app_id: &str) -> Result<Vec<Person>, String> {
    let mut st = conn
        .prepare(
            "SELECT a.subject_id, r.name, r.rooms, c.id, c.scope, c.enabled
             FROM grants a
             JOIN principals p ON p.id = a.subject_id
             LEFT JOIN roles r ON r.id = a.role_id
             JOIN grants c ON c.kind = 'chat' AND c.subject_kind = 'principal' AND c.subject_id = a.subject_id
                          AND c.enabled = 1 AND c.scope = 'member'
             WHERE a.kind = 'app' AND a.target_id = ?1 AND a.subject_kind = 'principal' AND a.enabled = 1
               AND (p.agent_workspace IS NULL OR trim(p.agent_workspace) = '')
               AND NOT EXISTS (SELECT 1 FROM principal_agent_ties t WHERE t.principal_id = a.subject_id)
             ORDER BY a.subject_id",
        )
        .map_err(|e| format!("governed people: {e}"))?;
    let rows = st
        .query_map(params![app_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| format!("governed people: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (principal, role, rooms, gid, scope, enabled) = row.map_err(|e| format!("governed people row: {e}"))?;
        out.push(Person {
            principal,
            role: role.map(|r| r.trim().to_ascii_lowercase()),
            rooms: crate::skin::parse_rooms_json(rooms.as_deref()).into_iter().collect(),
            chat_rev: format!("{gid}:{}:{enabled}", scope.unwrap_or_default()),
            chat_grant: gid,
        });
    }
    Ok(out)
}

/// Every workspace that is a room on one of this App's roles.
fn role_rooms_of_app(conn: &Connection, app_id: &str) -> Result<BTreeSet<String>, String> {
    let mut st = conn
        .prepare(
            "SELECT DISTINCT r.rooms FROM roles r
             WHERE r.app_id = ?1
                OR r.id IN (SELECT role_id FROM grants WHERE kind = 'app' AND target_id = ?1 AND role_id IS NOT NULL)",
        )
        .map_err(|e| format!("app role rooms: {e}"))?;
    let rows = st
        .query_map(params![app_id], |r| r.get::<_, Option<String>>(0))
        .map_err(|e| format!("app role rooms: {e}"))?;
    let mut out = BTreeSet::new();
    for row in rows {
        out.extend(crate::skin::parse_rooms_json(row.map_err(|e| format!("app role rooms row: {e}"))?.as_deref()));
    }
    Ok(out)
}

fn agent_provisioned(conn: &Connection, ws: &str) -> Result<bool, String> {
    Ok(super::members::by_external_id(conn, &format!("k2:workspace:{ws}"))?.is_some_and(|m| m.state == super::members::ACTIVE))
}

/// The wanted set of one governed Space (its App keyed, the flag on), or
/// why it waits. Never a box user, an agent-marked login or a tied one.
pub fn build_wanted(conn: &Connection, space_id: &str, app_id: &str, governed_count: usize) -> Result<(Vec<WantedSync>, Waiting), String> {
    let mut waiting = Waiting::default();
    let Some(key) = keyed(conn, app_id)? else { return Ok((Vec::new(), waiting)) };
    let facts = facts_of(conn, app_id)?.unwrap_or_default();
    let block = facts.block.clone().unwrap_or_default();
    let defs = manifest_channels_for(&block, space_id, governed_count);
    let rows: HashMap<String, ChannelRow> = channel_rows(conn, space_id)?.into_iter().map(|r| (r.slug.clone(), r)).collect();
    // Every private manifest channel needs its id before anyone is sent:
    // a partial list would take people out of channels they belong in.
    for c in defs.iter().filter(|c| c.private) {
        match rows.get(&c.slug) {
            Some(r) if r.state == CH_READY && r.channel_id.is_some() => {}
            Some(r) if r.state == CH_FAILED => {}
            _ => waiting.channels.push(c.slug.clone()),
        }
    }
    if !waiting.channels.is_empty() {
        return Ok((Vec::new(), waiting));
    }
    let ready_private = |slug: &str| -> Option<String> {
        rows.get(slug).filter(|r| r.private && r.state == CH_READY).and_then(|r| r.channel_id.clone())
    };
    let room_ids = |handles: &[String]| -> BTreeSet<String> {
        handles.iter().filter_map(|h| facts.handles.get(&h.trim().to_ascii_lowercase()).cloned()).collect()
    };
    let lists = app_lists(conn, app_id, space_id)?;
    let mut out = Vec::new();
    for p in people(conn, app_id)? {
        let Ok(ext) = ExternalId::parse(&format!("k2:principal:{}", p.principal)) else { continue };
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for c in defs.iter().filter(|c| c.private) {
            let by_role = p.role.as_ref().is_some_and(|r| c.roles.iter().any(|x| x.trim().eq_ignore_ascii_case(r)));
            let by_room = !room_ids(&c.rooms).is_disjoint(&p.rooms);
            if by_role || by_room {
                if let Some(id) = ready_private(&c.slug) {
                    ids.insert(id);
                }
            }
        }
        let mut level = p
            .role
            .as_ref()
            .and_then(|r| block.levels.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(r)).map(|(_, v)| v.clone()))
            .unwrap_or_else(|| "member".into());
        if let Some((slugs, l)) = lists.get(&p.principal) {
            for s in slugs {
                if let Some(id) = ready_private(s) {
                    ids.insert(id);
                }
            }
            if let Some(l) = l {
                level = l.clone();
            }
        }
        let body = MemberChannelsRequest {
            app_id: app_id.to_string(),
            channel_ids: ids.into_iter().collect(),
            level: clamp_level(&level, &key.max_level),
        };
        out.push(WantedSync { space_id: space_id.into(), app_id: app_id.into(), ext, body, grant_id: Some(p.chat_grant), grant_rev: p.chat_rev });
    }
    // Agents: private text channels only, always `member`, and only the
    // App's own workspace or a room on one of its roles.
    let allowed = {
        let mut a = role_rooms_of_app(conn, app_id)?;
        if let Some(w) = &facts.owner_workspace {
            a.insert(w.clone());
        }
        a
    };
    let mut agent_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in defs.iter().filter(|c| c.private && c.kind == "text") {
        for h in &c.agents {
            let key_h = h.trim().to_ascii_lowercase();
            match facts.handles.get(&key_h) {
                Some(ws) if allowed.contains(ws) => {
                    let e = agent_ids.entry(ws.clone()).or_default();
                    if let Some(id) = ready_private(&c.slug) {
                        e.insert(id);
                    }
                }
                _ => {
                    if !waiting.refused_agents.contains(&key_h) {
                        waiting.refused_agents.push(key_h);
                    }
                }
            }
        }
    }
    // An agent the ledger holds but no channel names any more gets an empty
    // list (out of every private channel; it stays a Space member).
    for (ext_s, _) in member_ledger(conn, space_id)? {
        if let Some(ws) = ext_s.strip_prefix("k2:workspace:") {
            agent_ids.entry(ws.to_string()).or_default();
        }
    }
    for (ws, ids) in agent_ids {
        let Ok(ext) = ExternalId::parse(&format!("k2:workspace:{ws}")) else { continue };
        if !agent_provisioned(conn, &ws)? {
            waiting.agents.push(ws);
            continue;
        }
        out.push(WantedSync {
            space_id: space_id.into(),
            app_id: app_id.into(),
            ext,
            body: MemberChannelsRequest { app_id: app_id.to_string(), channel_ids: ids.into_iter().collect(), level: "member".into() },
            grant_id: None,
            grant_rev: "agent".into(),
        });
    }
    Ok((out, waiting))
}

#[derive(Debug, Clone)]
struct LedgerRow {
    body_hash: String,
    sent_hash: Option<String>,
    state: String,
    why: Option<String>,
    grant_rev: Option<String>,
}

fn member_ledger(conn: &Connection, space_id: &str) -> Result<Vec<(String, LedgerRow)>, String> {
    let mut st = conn
        .prepare("SELECT external_id, body_hash, sent_hash, state, why, grant_rev FROM tool_app_space_member_channels WHERE space_id = ?1")
        .map_err(|e| format!("member channels ledger: {e}"))?;
    let rows = st
        .query_map(params![space_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                LedgerRow { body_hash: r.get(1)?, sent_hash: r.get(2)?, state: r.get(3)?, why: r.get(4)?, grant_rev: r.get(5)? },
            ))
        })
        .map_err(|e| format!("member channels ledger: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("member channels ledger row: {e}"))
}

pub fn chan_subject(space_id: &str, ext: &ExternalId) -> String {
    format!("{CHAN_PREFIX}{space_id}:{ext}")
}

/// `(space id, external id)` of a `chan:` subject.
pub fn parse_chan_subject(subject: &str) -> Option<(String, ExternalId)> {
    let rest = subject.strip_prefix(CHAN_PREFIX)?;
    let (space, ext) = rest.split_once(':')?;
    Some((space.to_string(), ExternalId::parse(ext).ok()?))
}

/// The hash of a sync body (what the ledger compares).
pub fn body_hash(body: &MemberChannelsRequest) -> String {
    sha_hex(&serde_json::to_string(body).unwrap_or_default())
}

fn sync_op(w: &WantedSync, cause: Cause) -> Result<NewOp, String> {
    Ok(NewOp {
        tool: CHAT_KIND,
        op: Op::SpaceMemberChannels,
        subject_key: chan_subject(&w.space_id, &w.ext),
        payload: serde_json::to_value(&w.body).map_err(|e| format!("sync body: {e}"))?,
        grant_id: w.grant_id.clone(),
        grant_rev: Some(w.grant_rev.clone()),
        cause,
        // Provision first, then sync: the person's own queue goes first.
        after_subject: Some(w.ext.to_string()),
    })
}

/// What one [`plan_members`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub spaces: usize,
    pub wanted: usize,
    pub queued: usize,
    pub held_kept: usize,
    pub forgotten: usize,
    pub waiting: BTreeMap<String, Waiting>,
}

/// Diff every governed Space's wanted set against the ledger, write the
/// ledger and return the rows to enqueue. MUST run inside the caller's
/// transaction. `busy` = subjects with an open outbox row.
pub fn plan_members(conn: &Connection, busy: &HashSet<String>, now: i64) -> Result<(Vec<NewOp>, Summary), String> {
    let mut s = Summary::default();
    if !flag_on(conn)? {
        return Ok((Vec::new(), s));
    }
    let governed = governed_spaces(conn)?;
    let mut per_app: HashMap<String, usize> = HashMap::new();
    for (_, a) in &governed {
        *per_app.entry(a.clone()).or_default() += 1;
    }
    let governed_set: HashSet<String> = governed.iter().map(|(sp, _)| sp.clone()).collect();
    let mut ops = Vec::new();
    for (space, app) in &governed {
        s.spaces += 1;
        let (wanted, waiting) = build_wanted(conn, space, app, per_app[app])?;
        if waiting != Waiting::default() {
            s.waiting.insert(space.clone(), waiting.clone());
        }
        if !waiting.channels.is_empty() {
            continue;
        }
        let mut have: HashMap<String, LedgerRow> = member_ledger(conn, space)?.into_iter().collect();
        let waiting_agents: HashSet<String> = waiting.agents.iter().map(|w| format!("k2:workspace:{w}")).collect();
        for w in &wanted {
            s.wanted += 1;
            let ext = w.ext.to_string();
            let hash = body_hash(&w.body);
            let open = busy.contains(&chan_subject(space, &w.ext));
            let send = match have.remove(&ext) {
                None => true,
                Some(l) => match l.state.as_str() {
                    HELD if l.why.as_deref() == Some(WHY_BANNED) => {
                        s.held_kept += 1;
                        false
                    }
                    HELD if l.why.as_deref() == Some(WHY_DEPROVISIONED) => {
                        let changed = l.grant_rev.as_deref() != Some(w.grant_rev.as_str());
                        if !changed {
                            s.held_kept += 1;
                        }
                        changed
                    }
                    HELD => {
                        let changed = l.body_hash != hash;
                        if !changed {
                            s.held_kept += 1;
                        }
                        changed
                    }
                    // A Sew job: send again (a fresh key) to read it back.
                    QUEUED => !open,
                    PENDING => !open || l.body_hash != hash,
                    _ => l.sent_hash.as_deref() != Some(hash.as_str()),
                },
            };
            if send {
                ops.push(sync_op(w, Cause::Reconcile)?);
                s.queued += 1;
            }
            let state_now: Option<&str> = if send { Some(PENDING) } else { None };
            conn.execute(
                "INSERT INTO tool_app_space_member_channels (space_id, external_id, app_id, body, body_hash, sent_hash, state, why, grant_rev, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL, ?7, ?8)
                 ON CONFLICT(space_id, external_id) DO UPDATE SET app_id = excluded.app_id, body = excluded.body,
                    body_hash = excluded.body_hash, grant_rev = excluded.grant_rev,
                    state = COALESCE(?9, tool_app_space_member_channels.state),
                    why = CASE WHEN ?9 IS NULL THEN tool_app_space_member_channels.why ELSE NULL END,
                    updated_at = excluded.updated_at",
                params![
                    space,
                    ext,
                    app,
                    serde_json::to_string(&w.body).map_err(|e| e.to_string())?,
                    hash,
                    PENDING,
                    w.grant_rev,
                    now,
                    state_now
                ],
            )
            .map_err(|e| format!("member channels write: {e}"))?;
        }
        // No longer wanted: K31 takes people out of the Space; K2 forgets
        // them here. (An agent waiting on its own provision stays.) A
        // banned hold outlives everything, like K31's.
        for (ext, l) in have {
            if waiting_agents.contains(&ext) || (l.state == HELD && l.why.as_deref() == Some(WHY_BANNED)) {
                continue;
            }
            conn.execute("DELETE FROM tool_app_space_member_channels WHERE space_id = ?1 AND external_id = ?2", params![space, ext])
                .map_err(|e| format!("member channels forget: {e}"))?;
            s.forgotten += 1;
        }
    }
    // Spaces no longer governed (switched back, key revoked, unbound):
    // forget their ledgers.
    let mut st = conn
        .prepare("SELECT DISTINCT space_id FROM tool_app_space_member_channels")
        .map_err(|e| format!("member channels spaces: {e}"))?;
    let spaces: Vec<String> = st
        .query_map([], |r| r.get(0))
        .map_err(|e| format!("member channels spaces: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("member channels spaces row: {e}"))?;
    for sp in spaces {
        if !governed_set.contains(&sp) && !wants_app(conn, &sp)? {
            forget_members(conn, &sp)?;
        }
    }
    Ok((ops, s))
}

// ── Sew's answers ────────────────────────────────────────────────────

/// What one person's I4 answer means for K2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Applied (or the owner, which a sync never touches).
    Synced,
    /// Ran as a Sew job: read back on the next sweep.
    Queued,
    /// Held: not re-sent until the reason says (banned: never on K2's own).
    Held(String),
    /// 422 `member_not_provisioned`: provision them again, then sync again.
    Reprovision(String),
    /// Retry with backoff (Sew says the Space isn't App-governed yet).
    Retry(String),
    /// Anything else: blocked, loudly.
    Blocked(String),
}

/// Read one person's 2xx answer.
pub fn verdict_ok(resp: &MemberChannelsResponse) -> Verdict {
    match resp.skipped.as_deref() {
        Some(sew_wire::skipped::BANNED) => return Verdict::Held(WHY_BANNED.into()),
        Some(sew_wire::skipped::DEPROVISIONED) => return Verdict::Held(WHY_DEPROVISIONED.into()),
        Some(sew_wire::skipped::OWNER) => return Verdict::Synced,
        Some(other) => return Verdict::Held(format!("skipped: {other}")),
        None => {}
    }
    if !resp.unknown_channel_ids.is_empty() {
        return Verdict::Held(format!("unknown channels: {}", resp.unknown_channel_ids.join(",")));
    }
    if resp.queued {
        return Verdict::Queued;
    }
    Verdict::Synced
}

/// Read one person's refusal (`status`, `code`, `details.reason`).
pub fn verdict_err(status: u16, code: &str, reason: &str, line: &str) -> Verdict {
    let is = |w: &str| reason == w || code == w;
    if is(sew_wire::reasons::MEMBER_NOT_PROVISIONED) {
        return Verdict::Reprovision(format!("{status} {line}"));
    }
    if is(sew_wire::reasons::NOT_APP_GOVERNED) {
        return Verdict::Retry(format!("{status} {line}: Sew doesn't hold this Space as App-governed yet"));
    }
    for r in [sew_wire::reasons::BOX_USER_NOT_ALLOWED, sew_wire::reasons::LEVEL_ABOVE_CEILING, sew_wire::reasons::AGENT_LEVEL] {
        if is(r) {
            crate::log_debug!("[grant-hook] K2 BUG: Sew refused a channel sync item with {r} ({line}); K2 must never send that");
            return Verdict::Held(format!("refused: {r} (a K2 bug, reported)"));
        }
    }
    Verdict::Blocked(format!("{status} {line}"))
}

/// A delivered sync row (its own answer, or its item of a bulk answer):
/// move the ledger on. A row older than the ledger's newest body leaves it
/// pending (the newer row is queued).
pub fn after_done(conn: &Connection, row_id: &str, result: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, row_id)? else { return Ok(()) };
    if Op::parse(&row.op) != Some(Op::SpaceMemberChannels) {
        return Ok(());
    }
    let Some((space, ext)) = parse_chan_subject(&row.subject_key) else { return Ok(()) };
    let Ok(body) = serde_json::from_str::<MemberChannelsRequest>(&row.payload_raw) else { return Ok(()) };
    let hash = body_hash(&body);
    let resp: MemberChannelsResponse = serde_json::from_str(result).unwrap_or_default();
    let state = if verdict_ok(&resp) == Verdict::Queued { QUEUED } else { SYNCED };
    conn.execute(
        "UPDATE tool_app_space_member_channels SET sent_hash = ?3, state = CASE WHEN body_hash = ?3 THEN ?4 ELSE state END,
            why = NULL, updated_at = ?5 WHERE space_id = ?1 AND external_id = ?2",
        params![space, ext.to_string(), hash, state, now],
    )
    .map(|_| ())
    .map_err(|e| format!("member channels done: {e}"))
}

/// A held sync row: record why (banned is sticky; deprovisioned until the
/// chat grant changes; unknown channels are ensured again once).
pub fn after_held(conn: &Connection, row_id: &str, why: &str, result: &str, now: i64) -> Result<(), String> {
    let Some(row) = outbox::get(conn, row_id)? else { return Ok(()) };
    if Op::parse(&row.op) != Some(Op::SpaceMemberChannels) {
        return Ok(());
    }
    let Some((space, ext)) = parse_chan_subject(&row.subject_key) else { return Ok(()) };
    let resp: MemberChannelsResponse = serde_json::from_str(result).unwrap_or_default();
    if !resp.unknown_channel_ids.is_empty() {
        forget_unknown_channels(conn, &space, &resp.unknown_channel_ids, now)?;
        super::kick_reconcile();
    }
    conn.execute(
        "UPDATE tool_app_space_member_channels SET state = 'held', why = ?3, grant_rev = COALESCE(?4, grant_rev), updated_at = ?5
         WHERE space_id = ?1 AND external_id = ?2",
        params![space, ext.to_string(), why, row.grant_rev, now],
    )
    .map(|_| ())
    .map_err(|e| format!("member channels held: {e}"))
}

/// Let K2 sync a banned person again (Sew reported the unban). Returns
/// whether there was a banned hold.
pub fn clear_banned(conn: &Connection, space_id: &str, external_id: &str) -> Result<bool, String> {
    let n = conn
        .execute(
            "DELETE FROM tool_app_space_member_channels WHERE space_id = ?1 AND external_id = ?2 AND state = 'held' AND why = ?3",
            params![space_id, external_id, WHY_BANNED],
        )
        .map_err(|e| format!("member channels unban: {e}"))?;
    Ok(n > 0)
}

/// 422 `member_not_provisioned`: finish the row unsent, queue the person's
/// provision again (their chat grant, as a reconcile would) and the same
/// sync behind it. MUST run inside the caller's transaction. `Ok(false)`:
/// they have no chat grant any more (the row is held instead).
pub fn reprovision(conn: &Connection, row_id: &str, ctx: &super::HookCtx, why: &str, now: i64) -> Result<bool, String> {
    let Some(row) = outbox::get(conn, row_id)? else { return Ok(false) };
    let Some((_space, ext)) = parse_chan_subject(&row.subject_key) else { return Ok(false) };
    let (kind, id) = match &ext {
        ExternalId::K2Principal(id) => ("principal", id.clone()),
        ExternalId::K2Workspace(id) => ("workspace", id.clone()),
        _ => return Ok(false),
    };
    let grant = conn
        .query_row(
            &format!("{} WHERE kind = ?1 AND subject_kind = ?2 AND subject_id = ?3 AND enabled = 1", crate::skin::GRANT_SELECT),
            params![CHAT_KIND, kind, id],
            crate::skin::map_grant_row,
        )
        .optional()
        .map_err(|e| format!("reprovision grant: {e}"))?;
    conn.execute(
        "UPDATE tool_outbox SET state = ?2, last_error = ?3, updated_at = ?4 WHERE id = ?1",
        params![row_id, outbox::STATE_SUPERSEDED, why, now],
    )
    .map_err(|e| format!("reprovision finish: {e}"))?;
    let Some(grant) = grant else { return Ok(false) };
    let prov = super::chat::provision_op(conn, &grant, ctx, Cause::Reconcile)?;
    let again = NewOp {
        tool: CHAT_KIND,
        op: Op::SpaceMemberChannels,
        subject_key: row.subject_key.clone(),
        payload: serde_json::from_str(&row.payload_raw).map_err(|e| format!("reprovision body: {e}"))?,
        grant_id: row.grant_id.clone(),
        grant_rev: row.grant_rev.clone(),
        cause: Cause::Reconcile,
        after_subject: Some(ext.to_string()),
    };
    outbox::enqueue(conn, &[prov, again], super::ACTOR_RECONCILE, now)?;
    Ok(true)
}

// ── the read-only view (`k2 app chat <app>`) ─────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonView {
    pub external_id: String,
    pub username: Option<String>,
    pub level: String,
    pub channels: Vec<String>,
    pub state: String,
    pub why: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeavingView {
    pub external_id: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceView {
    pub space_id: String,
    /// What K2 wants (`None`: never switched: Sew-governed).
    pub governance: Option<String>,
    /// What Sew last confirmed.
    pub governance_confirmed: Option<String>,
    pub channels: Vec<ChannelRow>,
    pub people: Vec<PersonView>,
    pub leaving: Vec<LeavingView>,
    pub waiting: Option<Waiting>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    pub app_id: String,
    pub feature: bool,
    pub key: Option<KeyView>,
    pub manifest_channels: usize,
    pub spaces: Vec<SpaceView>,
}

/// Everything `k2 app chat <app>` shows, from skin.db alone.
pub fn view(conn: &Connection, app_id: &str) -> Result<AppView, String> {
    let facts = facts_of(conn, app_id)?;
    let bound: Vec<String> =
        super::app_space::bindings(conn)?.into_iter().filter(|b| b.app_id == app_id).map(|b| b.space_id).collect();
    let governed_count = bound.iter().filter(|s| is_governed(conn, s).unwrap_or(false)).count();
    let mut spaces = Vec::new();
    for space in bound {
        let channels = channel_rows(conn, &space)?;
        let by_id: HashMap<String, String> =
            channels.iter().filter_map(|c| c.channel_id.clone().map(|id| (id, c.slug.clone()))).collect();
        let mut st = conn
            .prepare(
                "SELECT m.external_id, m.body, m.state, m.why, m.updated_at, p.username
                 FROM tool_app_space_member_channels m
                 LEFT JOIN principals p ON m.external_id = 'k2:principal:' || p.id
                 WHERE m.space_id = ?1 ORDER BY m.external_id",
            )
            .map_err(|e| format!("app chat view: {e}"))?;
        let people = st
            .query_map(params![space], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)?, r.get::<_, Option<String>>(5)?))
            })
            .map_err(|e| format!("app chat view: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("app chat view row: {e}"))?
            .into_iter()
            .map(|(ext, body, state, why, at, username)| {
                let b: Option<MemberChannelsRequest> = serde_json::from_str(&body).ok();
                PersonView {
                    external_id: ext,
                    username,
                    level: b.as_ref().map(|b| b.level.clone()).unwrap_or_default(),
                    channels: b
                        .map(|b| b.channel_ids.iter().map(|id| by_id.get(id).cloned().unwrap_or_else(|| id.clone())).collect())
                        .unwrap_or_default(),
                    state,
                    why,
                    updated_at: at,
                }
            })
            .collect();
        let mut lst = conn
            .prepare("SELECT external_id, state FROM tool_app_space_leaving WHERE space_id = ?1 ORDER BY external_id")
            .map_err(|e| format!("app chat leaving: {e}"))?;
        let leaving = lst
            .query_map(params![space], |r| Ok(LeavingView { external_id: r.get(0)?, state: r.get(1)? }))
            .map_err(|e| format!("app chat leaving: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("app chat leaving row: {e}"))?;
        let waiting = if is_governed(conn, &space)? && keyed(conn, app_id)?.is_some() {
            Some(build_wanted(conn, &space, app_id, governed_count.max(1))?.1).filter(|w| *w != Waiting::default())
        } else {
            None
        };
        spaces.push(SpaceView {
            governance: governance_wish(conn, &space)?,
            governance_confirmed: governance_confirmed(conn, &space)?,
            space_id: space,
            channels,
            people,
            leaving,
            waiting,
        });
    }
    Ok(AppView {
        app_id: app_id.to_string(),
        feature: flag_on(conn)?,
        key: chat_key(conn, app_id)?,
        manifest_channels: facts.and_then(|f| f.block).map(|b| b.channels.len()).unwrap_or(0),
        spaces,
    })
}

#[cfg(test)]
#[path = "governed_tests.rs"]
mod tests;
