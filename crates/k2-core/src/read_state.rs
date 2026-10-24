//! Unread counts for Threads and project chats (`thread-read-v1`, 0.46.1,
//! Rosson 2026-10-17, ticket 8d8d3d37). Daemon-owned: the desktop and the
//! phone render this truth. Shaped like Sew's read model (S14); research
//! `.k2/prds/research-unread-model-v1.md`.
//!
//! **Cursor, not counters.** One row per (person, conversation) in
//! `read_cursors` (migration 0190): `last_read_seq`, `muted`,
//! `muted_until`. Counts are derived from the cursor when read and never
//! stored. A cursor is a comparator, never a foreign key: a cursor past a
//! deleted or trimmed item still works, and a cursor past the head reads
//! as nothing unread.
//!
//! **Who.** `person` is [`People::person_key`]: the owner token and every
//! login the owner linked are `owner`; any other Connect login is its
//! username. An App guest is the person `guest:<principalId>` and an App's
//! own pass the bookmark `app:<serviceId>` (keyed by the daemon's
//! `read_routes::PassReader`, 0.46.3; [`Person::app_guest`]); neither can
//! collide with a person key, which never holds `:`. Agents have no read
//! state.
//!
//! **Moves.** A cursor only moves forward ([`ack`]), except when a
//! conversation move renumbers a Thread: [`remap_thread_move`] rewrites it
//! in the move's own savepoint, rounding toward read (an old unread is
//! never brought back).
//!
//! **Backfill.** Seeing a person ([`ensure_person`]) writes a cursor at
//! the head of every conversation that exists then, so everything starts
//! read. A conversation created later has no row, which reads as cursor 0
//! (its first message is unread, like Sew's new DM).
//!
//! **What counts.**
//! - `unread`: items past the cursor that someone else wrote (an agent or
//!   another person), not imported history, not a collected line, and not
//!   a card that is waiting (that one is in `waiting` instead). Capped at
//!   [`UNREAD_CAP`] ("99+" on clients).
//! - `waiting`: pending choice / secret cards in a Thread whose time has
//!   not passed. Reading never clears it; an answer, a dismiss/void or the
//!   card's expiry does. Project chats have no cards (0).
//! - `muted`: keeps its `unread` (a grey dot) but leaves the badge total.
//! - Totals: `total` = unread across unmuted conversations + every
//!   waiting card (muted or not) = the badge; `waitingTotal` = waiting
//!   across everything.
//! - **Action cards** (0.46.3): an App guest never sees one (keep-as-mine,
//!   permission cards), so for a guest it is neither waiting nor unread:
//!   counting it would tell the guest a card exists. A **permission card**
//!   (prd-agent-settings-cards-v1 rev 3.1 §4.6, C10) waits only for people
//!   at its floor (`view.approvers.floor`: `owner`, or `admin` = an Owner or
//!   Admin login); for every other person it is an ordinary unread line.
//!   The push `waitingTotal` reads these totals, so it follows the same
//!   rule. Every other card is unchanged.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::notify_prefs::{People, OWNER};
use crate::overlay::{self, OverlayDoc, SeqRemap};

/// `kind` of a Thread conversation (`id` = its conversationId).
pub const KIND_THREAD: &str = "thread";
/// `kind` of a project chat (`id` = the project group id).
pub const KIND_PROJECT: &str = "project";
/// Unread counts stop here; clients show "99+" above 99.
pub const UNREAD_CAP: i64 = 100;

/// A known conversation kind.
pub fn valid_kind(kind: &str) -> bool {
    kind == KIND_THREAD || kind == KIND_PROJECT
}

/// Why a read-state call failed. Routes map: `NotFound` → 404, `Usage` →
/// 400, `Store` → 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    NotFound(String),
    Usage(String),
    Store(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) | Self::Usage(s) | Self::Store(s) => f.write_str(s),
        }
    }
}

impl From<String> for ReadError {
    fn from(s: String) -> Self {
        Self::Store(s)
    }
}

fn store_err(e: rusqlite::Error) -> ReadError {
    ReadError::Store(format!("read state: {e}"))
}

/// One person: their person key and every login that is them (to tell
/// their own posts from others').
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub key: String,
    /// Lowercased login names that are this person (`owner` plus the
    /// linked accounts for the owner person).
    logins: Vec<String>,
    /// An App guest or App pass (App parity, 0.46.3): the username its
    /// posts carry as a `guest` author. `None` for people.
    guest: Option<String>,
}

impl Person {
    /// The person a login identity is (`"owner"` for the owner token, else
    /// a Connect username).
    pub fn of_login(people: &People, login: &str) -> Self {
        let key = people.person_key(login);
        let mut logins = vec![key.to_ascii_lowercase()];
        if key == OWNER {
            logins.extend(people.owner_aliases().iter().map(|l| l.to_ascii_lowercase()));
            logins.extend(people.linked.iter().map(|l| l.trim().to_ascii_lowercase()));
        }
        logins.sort();
        logins.dedup();
        Self { key, logins, guest: None }
    }

    /// An App guest (or an App's own pass) as a reader: its own cursors
    /// under `key` (never a person key: callers prefix it, `guest:` /
    /// `app:`), and its own posts are the `guest` author `username`.
    pub fn app_guest(key: &str, username: &str) -> Self {
        Self { key: key.to_string(), logins: Vec::new(), guest: Some(username.trim().to_string()) }
    }

    fn is_guest_login(&self, login: &str) -> bool {
        self.guest.as_deref().is_some_and(|g| g.eq_ignore_ascii_case(login.trim()))
    }

    fn is_login(&self, login: &str) -> bool {
        let l = login.trim().to_ascii_lowercase();
        self.logins.iter().any(|x| *x == l)
    }

    /// True when this person wrote this Thread doc (the credential stamp
    /// `author`, never `from`).
    pub fn wrote_thread_doc(&self, doc: &OverlayDoc) -> bool {
        if let Some(g) = &self.guest {
            return doc.author.as_ref().is_some_and(|a| a.kind == overlay::doc::AUTHOR_GUEST && a.login == *g);
        }
        match &doc.author {
            Some(a) if a.kind == overlay::doc::AUTHOR_OWNER => self.key == OWNER,
            Some(a) if a.kind == overlay::doc::AUTHOR_USER => self.is_login(&a.login),
            _ => false,
        }
    }

    /// True when this person wrote a project chat line. A guest line's
    /// author is `guest:<username>` ([`crate::project_groups::guest_author`]).
    pub fn wrote_project_message(&self, author_kind: &str, author: &str) -> bool {
        use crate::project_groups::{AUTHOR_KIND_GUEST, AUTHOR_KIND_PERSON};
        if author_kind == AUTHOR_KIND_PERSON {
            return self.is_login(author);
        }
        author_kind == AUTHOR_KIND_GUEST && author.strip_prefix("guest:").is_some_and(|u| self.is_guest_login(u))
    }
}

/// One conversation's read state for one person. Wire (camelCase):
/// `{kind, id, projectId?, unread, waiting, muted, mutedUntil?,
/// lastReadSeq, lastSeq}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadItem {
    pub kind: String,
    pub id: String,
    /// A Thread's workspace (`projects.id`); absent for a project chat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub unread: i64,
    pub waiting: i64,
    pub muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<i64>,
    pub last_read_seq: i64,
    pub last_seq: i64,
}

/// A person's badge numbers on this server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    /// Unread across unmuted conversations plus every waiting card.
    pub total: i64,
    /// Waiting cards across everything, muted or not.
    pub waiting_total: i64,
}

impl Totals {
    fn add(&mut self, item: &ReadItem) {
        if !item.muted {
            self.total += item.unread;
        }
        self.total += item.waiting;
        self.waiting_total += item.waiting;
    }
}

/// A person's whole read state: totals plus every conversation that has
/// something unread, something waiting, or is muted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub totals: Totals,
    pub items: Vec<ReadItem>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Cursor {
    last_read_seq: i64,
    muted: bool,
    muted_until: Option<i64>,
}

impl Cursor {
    fn muted_at(&self, now: i64) -> bool {
        self.muted && self.muted_until.is_none_or(|u| u > now)
    }
}

/// Unix seconds now.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True when migration 0190 has run (the `thread-read-v1` feature gate).
pub fn tables_exist(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('read_cursors', 'read_people')",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n == 2)
    .unwrap_or(false)
}

/// See a person: the first time, write a cursor at the head of every
/// conversation that exists now (everything starts read) and remember
/// when. `Ok(true)` when the person was new.
pub fn ensure_person(conn: &Connection, person: &str) -> Result<bool, ReadError> {
    let person = person.trim();
    if person.is_empty() {
        return Err(ReadError::Usage("read state belongs to a person".into()));
    }
    let known: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM read_people WHERE person = ?1", params![person], |r| r.get(0))
        .map_err(store_err)?;
    if known {
        return Ok(false);
    }
    let now = now_secs();
    conn.execute_batch("SAVEPOINT read_person").map_err(store_err)?;
    let res = (|| -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO read_people (person, baseline_at) VALUES (?1, ?2)",
            params![person, now],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO read_cursors (person, kind, conv_id, last_read_seq, updated_at) \
             SELECT ?1, 'thread', conversation_id, last_thread_seq, ?2 FROM overlay_conversations",
            params![person, now],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO read_cursors (person, kind, conv_id, last_read_seq, updated_at) \
             SELECT ?1, 'project', id, last_msg_seq, ?2 FROM project_groups",
            params![person, now],
        )?;
        Ok(())
    })();
    match res {
        Ok(()) => {
            conn.execute_batch("RELEASE read_person").map_err(store_err)?;
            Ok(true)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO read_person; RELEASE read_person");
            Err(store_err(e))
        }
    }
}

/// See every person this server knows (boot): the owner and each Connect
/// login's person. Returns how many were new.
pub fn ensure_people(conn: &Connection, people: &People) -> Result<usize, ReadError> {
    let mut keys = vec![OWNER.to_string()];
    keys.extend(people.users.iter().map(|u| people.person_key(&u.username)));
    keys.sort();
    keys.dedup();
    let mut n = 0;
    for k in keys {
        if ensure_person(conn, &k)? {
            n += 1;
        }
    }
    Ok(n)
}

/// A conversation's head seq, `None` when there is no such conversation.
pub fn head(conn: &Connection, kind: &str, id: &str) -> Result<Option<i64>, ReadError> {
    let sql = match kind {
        KIND_THREAD => "SELECT last_thread_seq FROM overlay_conversations WHERE conversation_id = ?1",
        KIND_PROJECT => "SELECT last_msg_seq FROM project_groups WHERE id = ?1",
        other => return Err(ReadError::Usage(format!("kind must be 'thread' or 'project', got '{other}'"))),
    };
    conn.query_row(sql, params![id], |r| r.get(0)).optional().map_err(store_err)
}

fn cursor(conn: &Connection, person: &str, kind: &str, id: &str) -> Result<Cursor, ReadError> {
    conn.query_row(
        "SELECT last_read_seq, muted, muted_until FROM read_cursors WHERE person = ?1 AND kind = ?2 AND conv_id = ?3",
        params![person, kind, id],
        |r| Ok(Cursor { last_read_seq: r.get(0)?, muted: r.get::<_, i64>(1)? != 0, muted_until: r.get(2)? }),
    )
    .optional()
    .map_err(store_err)
    .map(|c| c.unwrap_or_default())
}

fn require_head(conn: &Connection, kind: &str, id: &str) -> Result<i64, ReadError> {
    head(conn, kind, id)?.ok_or_else(|| ReadError::NotFound(format!("no {kind} conversation '{id}'")))
}

/// Mark read through `seq` (`None` = the newest item now). Forward only:
/// a `seq` at or behind the cursor changes nothing; a `seq` past the head
/// is clamped to the head. `Ok(true)` when the cursor moved.
pub fn ack(conn: &Connection, person: &str, kind: &str, id: &str, seq: Option<i64>) -> Result<bool, ReadError> {
    if let Some(s) = seq {
        if s < 0 {
            return Err(ReadError::Usage(format!("seq must be >= 0, got {s}")));
        }
    }
    ensure_person(conn, person)?;
    let head = require_head(conn, kind, id)?;
    let target = seq.unwrap_or(head).min(head);
    let cur = cursor(conn, person, kind, id)?;
    if target <= cur.last_read_seq {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO read_cursors (person, kind, conv_id, last_read_seq, updated_at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(person, kind, conv_id) DO UPDATE SET last_read_seq = excluded.last_read_seq, \
         updated_at = excluded.updated_at WHERE excluded.last_read_seq > read_cursors.last_read_seq",
        params![person, kind, id, target, now_secs()],
    )
    .map_err(store_err)?;
    Ok(true)
}

/// Mute or unmute a conversation for a person. `until` (unix secs) ends a
/// mute on its own; `None` = until unmuted. Unmuting clears `until`.
/// `Ok(true)` when the stored state changed.
pub fn set_mute(
    conn: &Connection,
    person: &str,
    kind: &str,
    id: &str,
    muted: bool,
    until: Option<i64>,
) -> Result<bool, ReadError> {
    ensure_person(conn, person)?;
    require_head(conn, kind, id)?;
    let until = if muted { until } else { None };
    let cur = cursor(conn, person, kind, id)?;
    if cur.muted == muted && cur.muted_until == until {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO read_cursors (person, kind, conv_id, last_read_seq, muted, muted_until, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(person, kind, conv_id) DO UPDATE SET muted = excluded.muted, \
         muted_until = excluded.muted_until, updated_at = excluded.updated_at",
        params![person, kind, id, cur.last_read_seq, muted as i64, until, now_secs()],
    )
    .map_err(store_err)?;
    Ok(true)
}

/// True when `person` has muted this conversation right now (pushes stop).
pub fn is_muted(conn: &Connection, person: &str, kind: &str, id: &str, now: i64) -> Result<bool, ReadError> {
    Ok(cursor(conn, person, kind, id)?.muted_at(now))
}

/// Does this Thread doc count as unread for `person` (when past the
/// cursor)? Someone else's post, not history, not a card waiting on them,
/// and never an action card for an App guest (it can't see one).
fn thread_doc_counts(person: &Person, doc: &OverlayDoc, now: i64) -> bool {
    if person.guest.is_some() && doc.is_action_card() {
        return false;
    }
    !person.wrote_thread_doc(doc)
        && doc.imported.is_none()
        && doc.external.is_none()
        && doc.via.as_deref() != Some("import")
        && !(overlay::is_waiting_now(doc, now) && card_waits_for(person, doc))
}

/// A permission card's floor (`choice.action.view.approvers.floor`), or
/// `None` for any other doc. A permission card without a readable floor
/// waits for the owner only (the strictest floor).
fn permission_floor(doc: &OverlayDoc) -> Option<&str> {
    let action = doc.choice.as_ref()?.action.as_ref()?;
    if action.id != crate::permission::CARD_ACTION_PERMISSION && action.id != crate::permission::CARD_ACTION_BATCH {
        return None;
    }
    let floor = action
        .view
        .as_ref()
        .and_then(|v| v.get("approvers"))
        .and_then(|a| a.get("floor"))
        .and_then(serde_json::Value::as_str);
    Some(match floor {
        Some("admin") => "admin",
        _ => "owner",
    })
}

/// Whether a waiting card is waiting on `person` (§4.6): never an action
/// card for an App guest; a permission card only for people at its floor
/// (the owner person, or a Connect login whose role meets it); every other
/// card for everyone, as before.
fn card_waits_for(person: &Person, doc: &OverlayDoc) -> bool {
    if person.guest.is_some() {
        return !doc.is_action_card();
    }
    let Some(floor) = permission_floor(doc) else { return true };
    if person.key == OWNER {
        return true;
    }
    match crate::connect_users::role_for_user(&person.key) {
        Some(crate::connect_users::Role::Owner) => true,
        Some(crate::connect_users::Role::Admin) => floor == "admin",
        _ => false,
    }
}

/// The waiting cards in one conversation (or every one) that wait on
/// `person`, as `(conversation, card id)`.
fn waiting_for(person: &Person, conversation_id: Option<&str>, now: i64) -> Result<Vec<(String, String)>, ReadError> {
    let mut out = Vec::new();
    for (conv, card) in overlay::waiting_cards(conversation_id, now)? {
        match overlay::get_doc(&card)? {
            Some(doc) if !card_waits_for(person, &doc) => {}
            _ => out.push((conv, card)),
        }
    }
    Ok(out)
}

fn thread_unread(person: &Person, conv: &str, after: i64, head: i64, now: i64) -> Result<i64, ReadError> {
    if head <= after {
        return Ok(0);
    }
    let page = overlay::read_thread_page(conv, Some(after), None, UNREAD_CAP as usize)?;
    if page.has_more {
        return Ok(UNREAD_CAP);
    }
    Ok(page.items.iter().filter(|i| thread_doc_counts(person, &i.doc, now)).count() as i64)
}

fn project_unread(conn: &Connection, person: &Person, group: &str, after: i64, head: i64) -> Result<i64, ReadError> {
    if head <= after {
        return Ok(0);
    }
    let mut stmt = conn
        .prepare(
            "SELECT author_kind, author FROM project_group_messages WHERE group_id = ?1 AND seq > ?2 \
             ORDER BY seq DESC LIMIT ?3",
        )
        .map_err(store_err)?;
    let rows: Vec<(String, String)> = stmt
        .query_map(params![group, after, UNREAD_CAP + 1], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(store_err)?
        .collect::<Result<_, _>>()
        .map_err(store_err)?;
    if rows.len() as i64 > UNREAD_CAP {
        return Ok(UNREAD_CAP);
    }
    Ok(rows.iter().filter(|(k, a)| !person.wrote_project_message(k, a)).count() as i64)
}

fn thread_waiting(person: &Person, conv: &str, now: i64) -> Result<i64, ReadError> {
    Ok(waiting_for(person, Some(conv), now)?.len() as i64)
}

fn thread_project_id(conn: &Connection, conv: &str) -> Result<Option<String>, ReadError> {
    conn.query_row(
        "SELECT project_id FROM overlay_conversations WHERE conversation_id = ?1",
        params![conv],
        |r| r.get(0),
    )
    .optional()
    .map_err(store_err)
}

/// One conversation's state for `person`.
pub fn item(conn: &Connection, person: &Person, kind: &str, id: &str, now: i64) -> Result<ReadItem, ReadError> {
    ensure_person(conn, &person.key)?;
    let head = require_head(conn, kind, id)?;
    let cur = cursor(conn, &person.key, kind, id)?;
    let (unread, waiting, project_id) = match kind {
        KIND_THREAD => (
            thread_unread(person, id, cur.last_read_seq, head, now)?,
            thread_waiting(person, id, now)?,
            thread_project_id(conn, id)?,
        ),
        _ => (project_unread(conn, person, id, cur.last_read_seq, head)?, 0, None),
    };
    let muted = cur.muted_at(now);
    Ok(ReadItem {
        kind: kind.to_string(),
        id: id.to_string(),
        project_id,
        unread,
        waiting,
        muted,
        muted_until: if muted { cur.muted_until } else { None },
        last_read_seq: cur.last_read_seq,
        last_seq: head,
    })
}

/// A person's totals (the badge numbers).
pub fn totals(conn: &Connection, person: &Person, now: i64) -> Result<Totals, ReadError> {
    Ok(summary(conn, person, now)?.totals)
}

/// A person's whole read state. Reads only the conversations whose head
/// is past the cursor, that hold a waiting card, or that are muted.
pub fn summary(conn: &Connection, person: &Person, now: i64) -> Result<Summary, ReadError> {
    ensure_person(conn, &person.key)?;
    let mut waiting: HashMap<String, i64> = HashMap::new();
    for (conv, _) in waiting_for(person, None, now)? {
        *waiting.entry(conv).or_default() += 1;
    }
    let mut out = Summary::default();

    // Threads: one catalog pass. A conversation with no catalog row (a
    // removed workspace's, a collected `gb:` one) is not counted.
    let mut stmt = conn
        .prepare(
            "SELECT c.conversation_id, c.project_id, c.last_thread_seq, COALESCE(r.last_read_seq, 0), \
             COALESCE(r.muted, 0), r.muted_until \
             FROM overlay_conversations c LEFT JOIN read_cursors r \
             ON r.person = ?1 AND r.kind = 'thread' AND r.conv_id = c.conversation_id",
        )
        .map_err(store_err)?;
    type Row = (String, String, i64, Cursor);
    let rows: Vec<Row> = stmt
        .query_map(params![person.key], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                Cursor { last_read_seq: r.get(3)?, muted: r.get::<_, i64>(4)? != 0, muted_until: r.get(5)? },
            ))
        })
        .map_err(store_err)?
        .collect::<Result<_, _>>()
        .map_err(store_err)?;
    for (conv, project_id, head, cur) in rows {
        let w = waiting.get(&conv).copied().unwrap_or(0);
        let muted = cur.muted_at(now);
        if head <= cur.last_read_seq && w == 0 && !muted {
            continue;
        }
        let unread = thread_unread(person, &conv, cur.last_read_seq, head, now)?;
        if unread == 0 && w == 0 && !muted {
            continue;
        }
        let item = ReadItem {
            kind: KIND_THREAD.to_string(),
            id: conv,
            project_id: Some(project_id),
            unread,
            waiting: w,
            muted,
            muted_until: if muted { cur.muted_until } else { None },
            last_read_seq: cur.last_read_seq,
            last_seq: head,
        };
        out.totals.add(&item);
        out.items.push(item);
    }

    // Project chats.
    let mut stmt = conn
        .prepare(
            "SELECT g.id, g.last_msg_seq, COALESCE(r.last_read_seq, 0), COALESCE(r.muted, 0), r.muted_until \
             FROM project_groups g LEFT JOIN read_cursors r \
             ON r.person = ?1 AND r.kind = 'project' AND r.conv_id = g.id \
             WHERE g.last_msg_seq > COALESCE(r.last_read_seq, 0) OR COALESCE(r.muted, 0) = 1",
        )
        .map_err(store_err)?;
    let rows: Vec<(String, i64, Cursor)> = stmt
        .query_map(params![person.key], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                Cursor { last_read_seq: r.get(2)?, muted: r.get::<_, i64>(3)? != 0, muted_until: r.get(4)? },
            ))
        })
        .map_err(store_err)?
        .collect::<Result<_, _>>()
        .map_err(store_err)?;
    for (group, head, cur) in rows {
        let muted = cur.muted_at(now);
        let unread = project_unread(conn, person, &group, cur.last_read_seq, head)?;
        if unread == 0 && !muted {
            continue;
        }
        let item = ReadItem {
            kind: KIND_PROJECT.to_string(),
            id: group,
            project_id: None,
            unread,
            waiting: 0,
            muted,
            muted_until: if muted { cur.muted_until } else { None },
            last_read_seq: cur.last_read_seq,
            last_seq: head,
        };
        out.totals.add(&item);
        out.items.push(item);
    }
    Ok(out)
}

/// One workspace's Threads summed for one person (agent-unread-tickets-v1,
/// 0.46.4): the numbers an agent row's Thread badge shows, the same fold
/// as the desktop's `threadSumForProject`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectThreadSum {
    /// Unread across the workspace's Threads, muted ones included, capped
    /// at [`UNREAD_CAP`].
    pub unread: i64,
    /// Cards waiting on the person across those Threads.
    pub waiting: i64,
    /// Every Thread that has unread is muted (a grey badge).
    pub muted: bool,
}

/// Fold a person's read items onto their workspaces: every Thread with
/// something unread or waiting counts on its `project_id`. Project chats
/// and Threads with no workspace are skipped. Pure.
pub fn thread_sums_by_project(items: &[ReadItem]) -> std::collections::BTreeMap<String, ProjectThreadSum> {
    let mut out: std::collections::BTreeMap<String, (ProjectThreadSum, i64)> = std::collections::BTreeMap::new();
    for i in items {
        if i.kind != KIND_THREAD || (i.unread <= 0 && i.waiting <= 0) {
            continue;
        }
        let Some(pid) = i.project_id.as_deref().filter(|p| !p.is_empty()) else { continue };
        let (sum, unmuted_unread) = out.entry(pid.to_string()).or_default();
        sum.unread += i.unread.max(0);
        sum.waiting += i.waiting.max(0);
        if !i.muted {
            *unmuted_unread += i.unread.max(0);
        }
    }
    out.into_iter()
        .map(|(pid, (mut sum, unmuted))| {
            sum.muted = unmuted == 0 && sum.unread > 0;
            sum.unread = sum.unread.min(UNREAD_CAP);
            (pid, sum)
        })
        .collect()
}

/// This server's badge salt (`read_server.badge_salt`, minted once by
/// migration 0190). Never leaves the daemon.
fn badge_salt(conn: &Connection) -> Result<String, ReadError> {
    conn.query_row("SELECT badge_salt FROM read_server WHERE id = 1", [], |r| r.get::<_, String>(0))
        .optional()
        .map_err(store_err)?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ReadError::Store("read state: no badge salt (migration 0190)".into()))
}

/// The person's `badgeKey` on this server ([`crate::push::badge_key`] of
/// the server's badge salt and the person key). Stable for the same person
/// on the same server across restarts, upgrades, token refreshes, re-logins
/// and device re-registration; different for every person. `person` is a
/// person key ([`People::person_key`]): the owner token and every linked
/// login give the same key.
pub fn badge_key_for(conn: &Connection, person: &str) -> Result<String, ReadError> {
    let person = person.trim();
    if person.is_empty() {
        return Err(ReadError::Usage("a badge key belongs to a person".into()));
    }
    Ok(crate::push::badge_key(&badge_salt(conn)?, person))
}

/// [`badge_key_for`] on the shared DB, for a person key or a login (a
/// login is mapped to its person). `""` when it can't be computed (no DB
/// or no salt): a push then carries no key rather than a name.
pub fn badge_key_for_shared(login_or_person: &str) -> String {
    let person = match People::load() {
        Ok(p) => p.person_key(login_or_person),
        Err(_) => login_or_person.trim().to_string(),
    };
    let Some(db) = crate::db::try_shared() else { return String::new() };
    let conn = db.lock();
    badge_key_for(&conn, &person).unwrap_or_else(|e| {
        crate::log_debug!("[read] badge key: {e}");
        String::new()
    })
}

/// Every person this server has seen (they get frames and remaps).
pub fn known_people(conn: &Connection) -> Result<Vec<String>, ReadError> {
    let mut stmt = conn.prepare("SELECT person FROM read_people ORDER BY person").map_err(store_err)?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(store_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store_err)?;
    Ok(rows)
}

/// The cursor a person has on `to` after a move or split renumbered its
/// rows: the highest new seq among the rows they had read in their source
/// (`from` rows up to `from_seq`, rows already on `to` up to `to_seq`).
/// Rounds toward read: a row below that which was unread becomes read,
/// but no read row ever becomes unread again.
pub fn remapped_cursor(remap: &[SeqRemap], from_seq: i64, to_seq: i64) -> i64 {
    remap
        .iter()
        .filter(|r| if r.from_old { r.old_seq <= from_seq } else { r.old_seq <= to_seq })
        .map(|r| r.new_seq)
        .max()
        .unwrap_or(0)
}

/// A Thread move (`split = false`: `from` merged into `to`, `from` gone)
/// or split (`split = true`: some of `from`'s rows moved to `to`, `from`
/// keeps the rest and its seqs) renumbered `to`'s rows: rewrite every
/// known person's cursor on `to`, carry a mute over, and (move) drop
/// `from`'s rows. Runs inside the caller's savepoint. A no-op before
/// migration 0190.
pub fn remap_thread_move(
    conn: &Connection,
    from: &str,
    to: &str,
    remap: &[SeqRemap],
    split: bool,
) -> Result<(), String> {
    if remap.is_empty() || !tables_exist(conn) {
        return Ok(());
    }
    let people = known_people(conn).map_err(|e| e.to_string())?;
    let now = now_secs();
    for person in people {
        let c_from = cursor(conn, &person, KIND_THREAD, from).map_err(|e| e.to_string())?;
        let c_to = cursor(conn, &person, KIND_THREAD, to).map_err(|e| e.to_string())?;
        let seq = remapped_cursor(remap, c_from.last_read_seq, c_to.last_read_seq);
        let (muted, until) = if c_to.muted {
            (true, c_to.muted_until)
        } else if c_from.muted {
            (true, c_from.muted_until)
        } else {
            (false, None)
        };
        conn.execute(
            "INSERT INTO read_cursors (person, kind, conv_id, last_read_seq, muted, muted_until, updated_at) \
             VALUES (?1, 'thread', ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(person, kind, conv_id) DO UPDATE SET last_read_seq = excluded.last_read_seq, \
             muted = excluded.muted, muted_until = excluded.muted_until, updated_at = excluded.updated_at",
            params![person, to, seq, muted as i64, until, now],
        )
        .map_err(|e| format!("read state remap: {e}"))?;
    }
    if !split {
        delete_conversation(conn, KIND_THREAD, from)?;
    }
    Ok(())
}

/// Drop every cursor and mute of a conversation (it was deleted).
pub fn delete_conversation(conn: &Connection, kind: &str, id: &str) -> Result<(), String> {
    if !tables_exist(conn) {
        return Ok(());
    }
    conn.execute("DELETE FROM read_cursors WHERE kind = ?1 AND conv_id = ?2", params![kind, id])
        .map_err(|e| format!("read state delete: {e}"))?;
    Ok(())
}

/// Forget a removed Connect user's read state (TP11). The owner person is
/// never touched.
pub fn delete_person(conn: &Connection, person: &str) -> Result<(), String> {
    let person = person.trim().to_ascii_lowercase();
    if person.is_empty() || person == OWNER || !tables_exist(conn) {
        return Ok(());
    }
    conn.execute("DELETE FROM read_cursors WHERE person = ?1", params![person])
        .map_err(|e| format!("read state delete: {e}"))?;
    conn.execute("DELETE FROM read_people WHERE person = ?1", params![person])
        .map_err(|e| format!("read state delete: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{PostAuthor, SecretProposal};

    fn people() -> People {
        People::default()
    }

    fn owner() -> Person {
        Person::of_login(&people(), "owner")
    }

    fn alice() -> Person {
        Person::of_login(&people(), "alice")
    }

    /// A fresh migrated DB on this thread with one workspace.
    fn db() -> (crate::db::TestDbGuard, String) {
        let g = crate::db::scoped_for_test();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let h = g.handle();
            let conn = h.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                params![id, format!("rs{}", &id[..6]), format!("/tmp/rs-{id}")],
            )
            .expect("seed project");
        }
        (g, id)
    }

    fn conv() -> String {
        format!("rs-conv-{}", uuid::Uuid::new_v4())
    }

    fn agent_post(conn: &Connection, conv: &str, project: &str, text: &str) -> i64 {
        overlay::post_thread(conn, conv, project, "rs-agent", "rs-agent", text, "thread").expect("post").0.seq
    }

    fn person_post(conn: &Connection, conv: &str, project: &str, author: PostAuthor, text: &str) -> i64 {
        overlay::post_thread_with_origin(conn, conv, project, &author.login.clone(), "rs-agent", text, "compose", None, Some(author))
            .expect("person post")
            .0
            .seq
    }

    fn ask(conn: &Connection, conv: &str, project: &str) -> (String, i64) {
        let (item, _) = overlay::post_choice(conn, conv, project, "rs-agent", "rs-agent", "Ship?", vec!["Go".into(), "Wait".into()], false, None)
            .expect("ask");
        (item.id, item.seq)
    }

    #[test]
    fn migration_0190_creates_tables_and_backfills_the_owner_at_heads() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        assert!(tables_exist(&conn));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM read_people WHERE person = 'owner'", [], |r| r.get(0))
            .expect("read_people");
        assert_eq!(n, 1, "the owner is seen by the migration");
        // A conversation created after the migration: no owner row, so its
        // first message is unread (cursor 0).
        let c = conv();
        agent_post(&conn, &c, &project, "hello");
        let it = item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item");
        assert_eq!((it.unread, it.last_read_seq, it.last_seq), (1, 0, 1), "{it:?}");
        assert_eq!(it.project_id.as_deref(), Some(project.as_str()));
    }

    #[test]
    fn a_new_person_starts_fully_read_then_counts_later_posts() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        for i in 0..3 {
            agent_post(&conn, &c, &project, &format!("old {i}"));
        }
        // Alice is first seen now: the three old posts are read.
        assert!(ensure_person(&conn, "alice").expect("ensure"));
        assert!(!ensure_person(&conn, "alice").expect("ensure again"), "seen once");
        let it = item(&conn, &alice(), KIND_THREAD, &c, now_secs()).expect("item");
        assert_eq!((it.unread, it.last_read_seq), (0, 3), "{it:?}");
        agent_post(&conn, &c, &project, "new");
        let it = item(&conn, &alice(), KIND_THREAD, &c, now_secs()).expect("item");
        assert_eq!(it.unread, 1, "{it:?}");
        // The owner was seen at migration time, before this conversation
        // existed: all four are unread for them.
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread, 4);
    }

    #[test]
    fn ack_is_monotonic_and_clamps_to_the_head() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        for i in 0..5 {
            agent_post(&conn, &c, &project, &format!("m{i}"));
        }
        assert!(ack(&conn, "owner", KIND_THREAD, &c, Some(3)).expect("ack 3"));
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread, 2);
        assert!(!ack(&conn, "owner", KIND_THREAD, &c, Some(2)).expect("ack back"), "never backwards");
        assert!(!ack(&conn, "owner", KIND_THREAD, &c, Some(3)).expect("same"), "same seq is a no-op");
        let it = item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item");
        assert_eq!((it.last_read_seq, it.unread), (3, 2), "{it:?}");
        assert!(ack(&conn, "owner", KIND_THREAD, &c, Some(999)).expect("past head"));
        let it = item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item");
        assert_eq!((it.last_read_seq, it.unread), (5, 0), "clamped to the head: {it:?}");
        agent_post(&conn, &c, &project, "m5");
        assert_eq!(
            item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread,
            1,
            "a clamp never hides a later post"
        );
        assert!(ack(&conn, "owner", KIND_THREAD, &c, None).expect("ack newest"));
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread, 0);
        assert_eq!(ack(&conn, "owner", KIND_THREAD, &c, Some(-1)), Err(ReadError::Usage("seq must be >= 0, got -1".into())));
        assert!(matches!(ack(&conn, "owner", KIND_THREAD, "no-such-conv", None), Err(ReadError::NotFound(_))));
        assert!(matches!(ack(&conn, "owner", "dm", &c, None), Err(ReadError::Usage(_))));
    }

    #[test]
    fn own_posts_never_count_others_do() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        ensure_person(&conn, "alice").expect("alice");
        person_post(&conn, &c, &project, PostAuthor::owner(), "from the owner");
        person_post(&conn, &c, &project, PostAuthor::user("alice"), "from alice");
        agent_post(&conn, &c, &project, "from the agent");
        person_post(&conn, &c, &project, PostAuthor::guest("guest1"), "from an app guest");
        let now = now_secs();
        // The conversation is newer than both people: every line is past
        // their cursor; each skips only their own.
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now).expect("owner").unread, 3);
        assert_eq!(item(&conn, &alice(), KIND_THREAD, &c, now).expect("alice").unread, 3);
        // A linked login is the owner person: alice's post is the owner's own.
        let linked = People { users: Vec::new(), linked: vec!["alice".into()] };
        let owner_linked = Person::of_login(&linked, "owner");
        assert_eq!(Person::of_login(&linked, "alice"), owner_linked, "one person");
        assert_eq!(item(&conn, &owner_linked, KIND_THREAD, &c, now).expect("linked").unread, 2);
    }

    #[test]
    fn imported_and_collected_lines_never_count() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        overlay::post_thread(&conn, &c, &project, "rs-agent", "rs-agent", "clone overflow line", "import").expect("import via");
        agent_post(&conn, &c, &project, "live");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread, 1, "via=import is history");
    }

    #[test]
    fn unread_counts_stop_at_the_cap() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        for i in 0..(UNREAD_CAP + 5) {
            agent_post(&conn, &c, &project, &format!("flood {i}"));
        }
        let it = item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item");
        assert_eq!(it.unread, UNREAD_CAP, "{it:?}");
        ack(&conn, "owner", KIND_THREAD, &c, Some(10)).expect("ack");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread, 95);
    }

    /// An action card on the Thread (keep-as-mine, or a permission card
    /// with `view.approvers.floor`).
    fn action_card(conn: &Connection, conv: &str, project: &str, id: &str, floor: Option<&str>) -> String {
        let mut action = serde_json::json!({ "id": id, "params": {}, "dedupe": uuid::Uuid::new_v4().to_string() });
        if let Some(f) = floor {
            action["view"] = serde_json::json!({ "approvers": { "floor": f, "label": "x", "names": [] } });
        }
        let action: overlay::CardAction = serde_json::from_value(action).expect("action");
        let (item, _) = overlay::post_choice_with_action(
            conn, conv, project, "rs-agent", "rs-agent", "Ask", vec!["Allow".into(), "Deny".into()], false, None, Some(action),
        )
        .expect("action card");
        item.id
    }

    /// prd-agent-settings-cards-v1 rev 3.1 §4.6 (C10): a permission card is
    /// waiting only for people at its floor; for everyone else it is an
    /// ordinary unread line, and the totals (the push's `waitingTotal`)
    /// follow. Every other card is unchanged for people.
    #[test]
    fn a_permission_card_waits_only_for_people_at_its_floor() {
        let _home = crate::test_env::TempHome::new();
        crate::connect_users::add_user("rsmaya", "a-long-password-1").expect("maya");
        crate::connect_users::set_role("rsmaya", crate::connect_users::Role::Admin).expect("admin");
        crate::connect_users::add_user("rsdana", "a-long-password-2").expect("dana");
        crate::connect_users::set_role("rsdana", crate::connect_users::Role::Member).expect("member");
        crate::connect_users::add_user("rsboss", "a-long-password-3").expect("boss");
        crate::connect_users::set_role("rsboss", crate::connect_users::Role::Owner).expect("owner");
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let (maya, dana, boss) =
            (Person::of_login(&people(), "rsmaya"), Person::of_login(&people(), "rsdana"), Person::of_login(&people(), "rsboss"));
        for p in [&owner(), &maya, &dana, &boss] {
            ensure_person(&conn, &p.key).expect("seen");
        }
        let now = now_secs();

        let admin_floor = conv();
        action_card(&conn, &admin_floor, &project, crate::permission::CARD_ACTION_PERMISSION, Some("admin"));
        let owner_floor = conv();
        action_card(&conn, &owner_floor, &project, crate::permission::CARD_ACTION_BATCH, Some("owner"));
        let no_floor = conv();
        action_card(&conn, &no_floor, &project, crate::permission::CARD_ACTION_PERMISSION, None);

        let st = |p: &Person, c: &str| {
            let it = item(&conn, p, KIND_THREAD, c, now).expect("item");
            (it.unread, it.waiting)
        };
        // Admin floor: the owner and an Admin wait on it; a Member reads it.
        assert_eq!(st(&owner(), &admin_floor), (0, 1));
        assert_eq!(st(&boss, &admin_floor), (0, 1), "an Owner login");
        assert_eq!(st(&maya, &admin_floor), (0, 1), "an Admin login");
        assert_eq!(st(&dana, &admin_floor), (1, 0), "a Member: an ordinary unread line");
        // Owner floor (and a card with no readable floor): owners only.
        for c in [&owner_floor, &no_floor] {
            assert_eq!(st(&owner(), c), (0, 1));
            assert_eq!(st(&boss, c), (0, 1));
            assert_eq!(st(&maya, c), (1, 0), "an Admin below an owner floor");
            assert_eq!(st(&dana, c), (1, 0));
        }
        // The totals (badge, push `waitingTotal`) follow.
        assert_eq!(summary(&conn, &owner(), now).expect("s").totals, Totals { total: 3, waiting_total: 3 });
        assert_eq!(summary(&conn, &maya, now).expect("s").totals, Totals { total: 3, waiting_total: 1 });
        assert_eq!(summary(&conn, &dana, now).expect("s").totals, Totals { total: 3, waiting_total: 0 });

        // Keep-as-mine and plain asks are unchanged: they wait for everyone.
        let other = conv();
        action_card(&conn, &other, &project, "zen.widget.keep", None);
        ask(&conn, &other, &project);
        assert_eq!(st(&dana, &other), (0, 2));
    }

    /// K2's rule (2026-10-21): an App guest never sees an action card, so
    /// none counts for it — not as waiting (that would say a card exists)
    /// and not as unread once answered. Its other lines count as before.
    #[test]
    fn an_app_guest_never_counts_an_action_card() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let guest = Person::app_guest("guest:pid-ac", "ana");
        ensure_person(&conn, &guest.key).expect("guest");
        let c = conv();
        let now = now_secs();
        let perm = action_card(&conn, &c, &project, crate::permission::CARD_ACTION_PERMISSION, Some("admin"));
        action_card(&conn, &c, &project, "zen.widget.keep", None);
        let it = item(&conn, &guest, KIND_THREAD, &c, now).expect("item");
        assert_eq!((it.unread, it.waiting), (0, 0), "a guest learns nothing: {it:?}");
        assert_eq!(summary(&conn, &guest, now).expect("s").totals, Totals { total: 0, waiting_total: 0 });
        // The owner still waits on both.
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now).expect("owner").waiting, 2);
        // Answered: still never unread for the guest; a plain post and a
        // plain ask still count.
        overlay::answer_card(&c, &project, &perm, Some("Allow"), None, None).expect("answer");
        agent_post(&conn, &c, &project, "hi");
        ask(&conn, &c, &project);
        let it = item(&conn, &guest, KIND_THREAD, &c, now).expect("item");
        assert_eq!((it.unread, it.waiting), (1, 1), "{it:?}");
    }

    #[test]
    fn waiting_is_cleared_only_by_an_answer_not_by_reading() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        // Bob is seen before the conversation exists: all of it is new to him.
        ensure_person(&conn, "bob").expect("bob");
        let bob = Person::of_login(&people(), "bob");
        agent_post(&conn, &c, &project, "before");
        let (card, _) = ask(&conn, &c, &project);
        let now = now_secs();
        let it = item(&conn, &owner(), KIND_THREAD, &c, now).expect("item");
        assert_eq!((it.unread, it.waiting), (1, 1), "the card is waiting, not unread: {it:?}");
        let s = summary(&conn, &owner(), now).expect("summary");
        assert_eq!(s.totals, Totals { total: 2, waiting_total: 1 });
        // Reading through the card leaves it waiting.
        ack(&conn, "owner", KIND_THREAD, &c, None).expect("ack");
        let it = item(&conn, &owner(), KIND_THREAD, &c, now).expect("item");
        assert_eq!((it.unread, it.waiting), (0, 1), "reading never clears waiting: {it:?}");
        assert_eq!(summary(&conn, &owner(), now).expect("summary").totals, Totals { total: 1, waiting_total: 1 });
        // Another person who never read sees the card as waiting too.
        let b = item(&conn, &bob, KIND_THREAD, &c, now).expect("bob");
        assert_eq!((b.unread, b.waiting), (1, 1), "{b:?}");
        // The answer clears it for everyone.
        overlay::answer_card(&c, &project, &card, Some("Go"), None, Some(PostAuthor::owner())).expect("answer");
        let it = item(&conn, &owner(), KIND_THREAD, &c, now).expect("item");
        assert_eq!(it.waiting, 0, "{it:?}");
        assert_eq!(summary(&conn, &owner(), now).expect("summary").totals, Totals::default());
        // For bob (cursor behind the card) the answered card is now plain unread.
        let b = item(&conn, &bob, KIND_THREAD, &c, now).expect("bob");
        assert_eq!((b.unread, b.waiting), (2, 0), "{b:?}");
    }

    #[test]
    fn void_expiry_and_due_cards_stop_waiting_and_secret_cards_wait() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        let (voided, _) = ask(&conn, &c, &project);
        let (expired, _) = ask(&conn, &c, &project);
        let (secret, _) = overlay::post_secret(&conn, &c, &project, "rs-agent", "rs-agent", "API_KEY", None, SecretProposal::default(), None)
            .expect("secret");
        let (due, _) = overlay::post_choice(&conn, &c, &project, "rs-agent", "rs-agent", "Soon?", vec!["Y".into()], false, Some(now_secs() + 60))
            .expect("due card");
        let now = now_secs();
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now).expect("item").waiting, 4);
        overlay::void_card(&c, &project, &voided, None).expect("void");
        overlay::expire_card(&c, &expired).expect("expire");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now).expect("item").waiting, 2);
        // A card whose time passed stops waiting even before the sweep.
        let it = item(&conn, &owner(), KIND_THREAD, &c, now + 120).expect("item later");
        assert_eq!(it.waiting, 1, "the secret card still waits: {it:?}");
        overlay::mark_secret(&c, &secret.id, "set", Some("vault:x".into()), "secret set".into(), None).expect("set");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now).expect("item").waiting, 1, "only {due:?} left");
    }

    #[test]
    fn mute_keeps_the_dot_drops_the_badge_but_not_waiting() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let c = conv();
        agent_post(&conn, &c, &project, "one");
        agent_post(&conn, &c, &project, "two");
        ask(&conn, &c, &project);
        let now = now_secs();
        assert_eq!(summary(&conn, &owner(), now).expect("s").totals, Totals { total: 3, waiting_total: 1 });
        assert!(set_mute(&conn, "owner", KIND_THREAD, &c, true, None).expect("mute"));
        assert!(!set_mute(&conn, "owner", KIND_THREAD, &c, true, None).expect("mute again"), "no change");
        let s = summary(&conn, &owner(), now).expect("s");
        assert_eq!(s.totals, Totals { total: 1, waiting_total: 1 }, "muted unread leaves the badge, waiting stays");
        let it = s.items.iter().find(|i| i.id == c).expect("muted item listed");
        assert_eq!((it.unread, it.waiting, it.muted), (2, 1, true), "the grey dot keeps its count: {it:?}");
        assert!(is_muted(&conn, "owner", KIND_THREAD, &c, now).expect("muted"));
        // Muting never moves the cursor.
        assert_eq!(it.last_read_seq, 0);
        // A timed mute ends on its own.
        assert!(set_mute(&conn, "owner", KIND_THREAD, &c, true, Some(now + 60)).expect("timed"));
        assert!(is_muted(&conn, "owner", KIND_THREAD, &c, now).expect("still muted"));
        assert!(!is_muted(&conn, "owner", KIND_THREAD, &c, now + 61).expect("ended"));
        assert_eq!(summary(&conn, &owner(), now + 61).expect("s").totals.total, 3);
        // Unmute clears `until`; a muted-but-empty conversation is still listed.
        assert!(set_mute(&conn, "owner", KIND_THREAD, &c, false, Some(now + 999)).expect("unmute"));
        let it = item(&conn, &owner(), KIND_THREAD, &c, now).expect("item");
        assert_eq!((it.muted, it.muted_until), (false, None));
        ack(&conn, "owner", KIND_THREAD, &c, None).expect("ack");
        set_mute(&conn, "owner", KIND_THREAD, &c, true, None).expect("mute");
        // Mutes are per person.
        ensure_person(&conn, "alice").expect("alice");
        assert!(!is_muted(&conn, "alice", KIND_THREAD, &c, now).expect("alice not muted"));
        assert!(matches!(set_mute(&conn, "owner", KIND_THREAD, "nope", true, None), Err(ReadError::NotFound(_))));
    }

    #[test]
    fn project_chat_counts_others_by_seq_and_own_posts_never() {
        let (g, _) = db();
        let h = g.handle();
        let gid = crate::project_groups::create_group(&format!("rs-proj-{}", uuid::Uuid::new_v4())).expect("group").id;
        let conn = h.lock();
        ensure_person(&conn, "alice").expect("alice");
        let m1 = crate::project_groups::post_message_as(&gid, "owner", "person", "kickoff").expect("m1");
        let m2 = crate::project_groups::post_message_as(&gid, "scout", "agent", "ack").expect("m2");
        let m3 = crate::project_groups::post_message_as(&gid, "alice", "person", "hi").expect("m3");
        assert_eq!((m1.seq, m2.seq, m3.seq), (1, 2, 3));
        assert_eq!(m3.author_kind, "person");
        let now = now_secs();
        // The group is older than alice's first sight (empty then): 3 lines past
        // both cursors, each skips their own.
        assert_eq!(item(&conn, &owner(), KIND_PROJECT, &gid, now).expect("owner").unread, 2);
        assert_eq!(item(&conn, &alice(), KIND_PROJECT, &gid, now).expect("alice").unread, 2);
        ack(&conn, "alice", KIND_PROJECT, &gid, Some(2)).expect("ack");
        let it = item(&conn, &alice(), KIND_PROJECT, &gid, now).expect("alice");
        assert_eq!((it.unread, it.waiting, it.last_read_seq, it.last_seq), (0, 0, 2, 3), "{it:?}");
        let s = summary(&conn, &owner(), now).expect("summary");
        assert!(s.items.iter().any(|i| i.kind == KIND_PROJECT && i.id == gid && i.unread == 2), "{s:?}");
        // Deleting the group drops its cursors.
        drop(conn);
        crate::project_groups::delete_group(&gid).expect("delete");
        let conn = h.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM read_cursors WHERE conv_id = ?1", params![gid], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0);
    }

    /// 0.46.3 App project chat: an App guest is its own person
    /// (`guest:<principalId>`), its own `guest:` lines never count for it, and
    /// a guest named like a Connect user shares nothing with that user.
    #[test]
    fn an_app_guest_is_its_own_person_and_skips_its_own_lines() {
        let (g, _) = db();
        let h = g.handle();
        let gid = crate::project_groups::create_group(&format!("rs-guest-{}", uuid::Uuid::new_v4())).expect("group").id;
        let conn = h.lock();
        let guest = Person::app_guest("guest:pid-1", "alice");
        assert_eq!(guest.key, "guest:pid-1");
        ensure_person(&conn, &guest.key).expect("guest");
        ensure_person(&conn, "alice").expect("alice");
        let author = crate::project_groups::guest_author("alice");
        assert_eq!(author, "guest:alice");
        let m1 = crate::project_groups::post_message_as(&gid, &author, crate::project_groups::AUTHOR_KIND_GUEST, "from the app")
            .expect("guest line");
        assert_eq!(m1.author_kind, "guest");
        crate::project_groups::post_message_as(&gid, "alice", "person", "from connect alice").expect("person line");
        crate::project_groups::post_message_as(&gid, "scout", "agent", "ack").expect("agent line");
        let now = now_secs();
        assert_eq!(item(&conn, &guest, KIND_PROJECT, &gid, now).expect("guest item").unread, 2, "its own guest line is never unread");
        assert_eq!(item(&conn, &alice(), KIND_PROJECT, &gid, now).expect("alice item").unread, 2, "the guest's line is unread for Connect alice");
        // Another guest with the same username in a different principal is not it.
        assert!(!Person::app_guest("guest:pid-2", "bob").wrote_project_message("guest", "guest:alice"));
        assert!(!guest.wrote_project_message("person", "alice"), "Connect alice's line is not the guest's");
        assert!(!guest.wrote_project_message("guest", "alice"), "an unprefixed author is never a guest line");
        // Cursors are per person.
        ack(&conn, &guest.key, KIND_PROJECT, &gid, None).expect("ack");
        assert_eq!(item(&conn, &guest, KIND_PROJECT, &gid, now).expect("guest").unread, 0);
        assert_eq!(item(&conn, &alice(), KIND_PROJECT, &gid, now).expect("alice").unread, 2);
        assert!(matches!(
            crate::project_groups::post_message_as(&gid, "x", "robot", "no"),
            Err(e) if e.starts_with("usage:")
        ));
    }

    #[test]
    fn a_cursor_past_deleted_or_trimmed_items_still_works() {
        let (g, project) = db();
        let h = g.handle();
        let gid = crate::project_groups::create_group(&format!("rs-trim-{}", uuid::Uuid::new_v4())).expect("group").id;
        let conn = h.lock();
        for i in 0..5 {
            crate::project_groups::post_message_as(&gid, "scout", "agent", &format!("l{i}")).expect("post");
        }
        ack(&conn, "owner", KIND_PROJECT, &gid, Some(3)).expect("ack");
        // Trim lines 1..=4, including the one the cursor names.
        conn.execute("DELETE FROM project_group_messages WHERE group_id = ?1 AND seq <= 4", params![gid]).expect("trim");
        let it = item(&conn, &owner(), KIND_PROJECT, &gid, now_secs()).expect("item");
        assert_eq!((it.unread, it.last_read_seq), (1, 3), "only the live line past the cursor: {it:?}");
        // A Thread cursor past the catalog head (a restore) reads as nothing unread.
        let c = conv();
        agent_post(&conn, &c, &project, "a");
        agent_post(&conn, &c, &project, "b");
        ack(&conn, "owner", KIND_THREAD, &c, None).expect("ack");
        conn.execute("UPDATE overlay_conversations SET last_thread_seq = 1 WHERE conversation_id = ?1", params![c]).expect("rewind");
        let it = item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item");
        assert_eq!((it.unread, it.last_read_seq, it.last_seq), (0, 2, 1), "{it:?}");
    }

    #[test]
    fn remapped_cursor_rounds_toward_read() {
        let r = |from_old, old_seq, new_seq| SeqRemap { from_old, old_seq, new_seq };
        // from: 1,2,3 ; to: 1,2 ; interleaved by time.
        let remap = [r(true, 1, 1), r(false, 1, 2), r(true, 2, 3), r(false, 2, 4), r(true, 3, 5)];
        assert_eq!(remapped_cursor(&remap, 0, 0), 0, "nothing read stays nothing read");
        assert_eq!(remapped_cursor(&remap, 3, 2), 5, "all read stays all read");
        assert_eq!(remapped_cursor(&remap, 2, 0), 3, "from 1..2 read: through new 3 (new 2 rounds to read)");
        assert_eq!(remapped_cursor(&remap, 0, 2), 4);
        assert_eq!(remapped_cursor(&[], 9, 9), 0);
    }

    #[test]
    fn a_conversation_move_remaps_cursors_through_the_renumber() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let from = conv();
        let to = conv();
        // `to` exists first with two rows (older), then `from` gets three.
        agent_post(&conn, &to, &project, "to-1");
        agent_post(&conn, &to, &project, "to-2");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        for i in 1..=3 {
            agent_post(&conn, &from, &project, &format!("from-{i}"));
        }
        // Owner read all of `to` and 2 of `from`; alice is seen now (all read).
        ack(&conn, "owner", KIND_THREAD, &to, None).expect("ack to");
        ack(&conn, "owner", KIND_THREAD, &from, Some(2)).expect("ack from");
        ensure_person(&conn, "alice").expect("alice");
        set_mute(&conn, "owner", KIND_THREAD, &from, true, None).expect("mute from");
        let mv = overlay::move_conversation(&conn, &from, &to).expect("move").expect("moved");
        assert_eq!(mv.thread_total, 5);
        let now = now_secs();
        let o = item(&conn, &owner(), KIND_THREAD, &to, now).expect("owner on to");
        assert_eq!((o.last_read_seq, o.last_seq, o.unread), (4, 5, 1), "to-1,to-2,from-1,from-2 read; from-3 unread: {o:?}");
        assert!(o.muted, "the mute moved with the conversation");
        let a = item(&conn, &alice(), KIND_THREAD, &to, now).expect("alice on to");
        assert_eq!((a.last_read_seq, a.unread), (5, 0), "{a:?}");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM read_cursors WHERE kind = 'thread' AND conv_id = ?1", params![from], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0, "the old key's cursors are gone");
    }

    #[test]
    fn a_split_hands_the_taken_rows_read_state_to_the_new_key() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let from = conv();
        let to = conv();
        let ids: Vec<i64> = (1..=4).map(|i| agent_post(&conn, &from, &project, &format!("{}-{i}", if i % 2 == 0 { "b" } else { "a" }))).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
        ack(&conn, "owner", KIND_THREAD, &from, Some(3)).expect("ack");
        let take = |d: &OverlayDoc| d.body.as_deref().is_some_and(|b| b.starts_with("b-"));
        overlay::split_conversation(&conn, &from, &to, &project, &take).expect("split").expect("moved");
        let now = now_secs();
        // `to` holds b-2 (read) and b-4 (unread) as seqs 1, 2.
        let t = item(&conn, &owner(), KIND_THREAD, &to, now).expect("to");
        assert_eq!((t.last_read_seq, t.last_seq, t.unread), (1, 2, 1), "{t:?}");
        // `from` keeps its seqs and cursor: a-1, a-3 read, nothing past 3 left.
        let f = item(&conn, &owner(), KIND_THREAD, &from, now).expect("from");
        assert_eq!((f.last_read_seq, f.unread), (3, 0), "{f:?}");
    }

    #[test]
    fn waiting_cards_follow_a_move() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let from = conv();
        let to = conv();
        let (card, _) = ask(&conn, &from, &project);
        overlay::move_conversation(&conn, &from, &to).expect("move").expect("moved");
        let now = now_secs();
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &to, now).expect("to").waiting, 1);
        assert!(overlay::waiting_cards(Some(&from), now).expect("from").is_empty());
        overlay::answer_card(&to, &project, &card, Some("Go"), None, None).expect("answer on to");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &to, now).expect("to").waiting, 0);
    }

    /// A pinned Chat that moves to another conversation leaves its old
    /// Thread unreachable by address: it turns read for every seen person
    /// (no badge anyone can't clear), whatever code path swapped the pin.
    #[test]
    fn a_replaced_pin_retires_its_old_threads_unread() {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        ensure_person(&conn, "alice").expect("alice");
        let old = conv();
        let new = conv();
        crate::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{old}"), &project, None, Some(&old), "claude", "system", "running")
            .expect("pin old");
        agent_post(&conn, &old, &project, "left behind 1");
        agent_post(&conn, &old, &project, "left behind 2");
        let now = now_secs();
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &old, now).expect("owner").unread, 2);
        assert_eq!(item(&conn, &alice(), KIND_THREAD, &old, now).expect("alice").unread, 2);
        // The owner already has a cursor row on the old Thread (an upsert's
        // conflict policy must not break the trigger on an existing row).
        ack(&conn, "owner", KIND_THREAD, &old, Some(1)).expect("owner read one");
        // A fresh session takes the pin (an upsert, like the spawn path).
        crate::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{old}"), &project, None, Some(&new), "claude", "system", "running")
            .expect("pin new");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &old, now).expect("owner").unread, 0, "retired for the owner");
        assert_eq!(item(&conn, &alice(), KIND_THREAD, &old, now).expect("alice").unread, 0, "and for alice");
        // The new pin's Thread counts as usual; a same-id upsert retires nothing.
        agent_post(&conn, &new, &project, "fresh");
        crate::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{old}"), &project, None, Some(&new), "claude", "system", "running")
            .expect("same pin");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &new, now).expect("owner new").unread, 1);
        // Clearing the pin (a reset) retires it too.
        conn.execute("UPDATE workspace_sessions SET session_id = NULL WHERE project_id = ?1", params![project]).expect("clear");
        assert_eq!(item(&conn, &owner(), KIND_THREAD, &new, now).expect("owner new").unread, 0);
    }

    /// badgeKey: one per person per server, stable across reads (and so
    /// across restarts: the salt is stored), 32 lowercase hex, never the
    /// person's name; the owner token and a linked login share one.
    #[test]
    fn badge_keys_are_stable_per_person_and_never_the_name() {
        let (g, _) = db();
        let h = g.handle();
        let conn = h.lock();
        let owner = badge_key_for(&conn, "owner").expect("owner key");
        assert_eq!(owner, badge_key_for(&conn, "owner").expect("again"), "stable");
        assert_eq!(owner.len(), 32);
        assert!(owner.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{owner}");
        for name in ["gina", "rosson.long@lztek.io"] {
            let k = badge_key_for(&conn, name).expect("key");
            assert_ne!(k, owner, "a different person, a different key");
            assert!(!k.contains(name) && !k.contains("owner"), "{k}");
        }
        let linked = People { users: Vec::new(), linked: vec!["z3thon".into()] };
        assert_eq!(
            badge_key_for(&conn, &linked.person_key("z3thon")).expect("linked"),
            owner,
            "a linked login is the owner person: same key"
        );
        // The salt is minted once and kept: re-running the migration keeps it.
        let salt = badge_salt(&conn).expect("salt");
        conn.execute(
            "INSERT OR IGNORE INTO read_server (id, badge_salt) VALUES (1, lower(hex(randomblob(16))))",
            [],
        )
        .expect("re-run insert");
        assert_eq!(badge_salt(&conn).expect("salt again"), salt);
        assert!(matches!(badge_key_for(&conn, "  "), Err(ReadError::Usage(_))));
    }

    #[test]
    fn delete_person_forgets_a_user_but_never_the_owner() {
        let (g, _) = db();
        let h = g.handle();
        let conn = h.lock();
        ensure_person(&conn, "carol").expect("carol");
        delete_person(&conn, "Carol").expect("delete");
        delete_person(&conn, "owner").expect("owner is a no-op");
        let people = known_people(&conn).expect("people");
        assert!(people.contains(&"owner".to_string()) && !people.contains(&"carol".to_string()), "{people:?}");
    }

    fn thread_item(id: &str, project: Option<&str>, unread: i64, waiting: i64, muted: bool) -> ReadItem {
        ReadItem {
            kind: KIND_THREAD.into(),
            id: id.into(),
            project_id: project.map(str::to_string),
            unread,
            waiting,
            muted,
            muted_until: None,
            last_read_seq: 0,
            last_seq: unread,
        }
    }

    /// agent-unread-tickets-v1: Threads fold onto their workspace like the
    /// desktop's `threadSumForProject`: muted unread still counts but greys
    /// the badge only when all of it is muted; project chats, Threads with
    /// no workspace and empty rows are skipped; unread caps at 100.
    #[test]
    fn thread_sums_by_project_fold_like_the_desktop_badge() {
        let mut project_chat = thread_item("g1", None, 9, 0, false);
        project_chat.kind = KIND_PROJECT.into();
        let items = vec![
            thread_item("a1", Some("pa"), 3, 0, false),
            thread_item("a2", Some("pa"), 2, 1, true),
            thread_item("b1", Some("pb"), 4, 0, true),
            thread_item("c1", Some("pc"), 0, 2, false),
            thread_item("d1", Some("pd"), 0, 0, true),
            thread_item("e1", Some("pe"), 80, 0, false),
            thread_item("e2", Some("pe"), 60, 0, false),
            thread_item("x1", None, 5, 0, false),
            project_chat,
        ];
        let sums = thread_sums_by_project(&items);
        assert_eq!(
            sums.into_iter().collect::<Vec<_>>(),
            vec![
                ("pa".to_string(), ProjectThreadSum { unread: 5, waiting: 1, muted: false }),
                ("pb".to_string(), ProjectThreadSum { unread: 4, waiting: 0, muted: true }),
                ("pc".to_string(), ProjectThreadSum { unread: 0, waiting: 2, muted: false }),
                ("pe".to_string(), ProjectThreadSum { unread: UNREAD_CAP, waiting: 0, muted: false }),
            ]
        );
    }
}
