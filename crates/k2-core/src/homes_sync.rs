//! Homes sync between a person's devices (homes-sync-v1, 0.46.1).
//!
//! Research: `.k2/prds/research-homes-sync-v1.md`. Rosson's rulings
//! (ticket 4f537560): one home server per person, local-first; the phone
//! picks its server and that server lists the devices syncing from it;
//! the first sync combines both lists; no "personal device" labels.
//!
//! **Model.** Every device keeps its own working copy of its Homes and
//! renders from it, so an offline home server never blocks the Home screen.
//! This store is only the sync peer: one set of **items** per person (one
//! per Home, one per Home row), each last-writer-wins on
//! (`updatedAt`, `deviceId`), deletes kept as **tombstones**.
//!
//! **Items** (the wire shape the routes speak, camelCase):
//! - Home: `{kind:"home", id, name, order, updatedAt, deleted?, mergedInto?}`
//! - Row:  `{kind:"row", homeId, address:"handle::<serverId>", label,
//!   workspaceId, order, updatedAt, deleted?}` — identity `(homeId, address)`.
//!
//! `order` is a fractional-index string (digits `0-9A-Za-z`, compared as
//! plain strings, never ending in `0`), so two devices' moves merge without
//! float trouble. `updatedAt` is the device's epoch milliseconds; a tie goes
//! to the larger `deviceId` (string compare), and on a full tie a tombstone
//! wins. A tombstone beats any older edit; an edit newer than the delete
//! brings the item back. `mergedInto` (a Home tombstone only) keeps a
//! merged-away Home id as a redirect, so selections and Garden `seedHome`
//! still resolve.
//!
//! **Cursor.** Each person has a monotonic `seq`; every push that changes
//! something bumps it once and stamps the changed items with it. A pull
//! `since` a cursor returns the items with a larger `seq` (tombstones
//! included). No `since`, a cursor below the tombstone floor, or a cursor
//! from the future (another server, a reset DB) returns the full live set.
//!
//! **Invariants.** A person always keeps at least one live Home once they
//! have one: a tombstone that would delete the last live Home is rejected
//! (`last_home`). Tombstones are kept [`TOMBSTONE_KEEP_MS`] (90 days, the
//! contract floor is 30); collection runs on write and raises the floor.
//! Redirect tombstones (`mergedInto`) are never collected.
//!
//! **Devices.** A device registers (`deviceId`, name, platform) before it
//! pushes. Removing it marks it removed: its pulls and pushes answer 410
//! `device_removed` until it registers again.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

/// `/boot-status` feature key.
pub const FEATURE: &str = "homes-sync-v1";

/// Tombstones older than this (by their `updatedAt`) are collected on write.
pub const TOMBSTONE_KEEP_MS: i64 = 90 * 24 * 60 * 60 * 1000;
/// An item stamped further than this in the future is refused.
pub const MAX_FUTURE_SKEW_MS: i64 = 24 * 60 * 60 * 1000;
/// The most items one push may carry.
pub const MAX_CHANGES: usize = 2000;
/// Home names are trimmed, inner whitespace collapsed, and capped (the
/// desktop's `HOME_NAME_MAX`).
pub const HOME_NAME_MAX: usize = 60;
pub const LABEL_MAX: usize = 200;
pub const ID_MAX: usize = 128;
pub const ADDRESS_MAX: usize = 300;
pub const ORDER_MAX: usize = 128;
pub const DEVICE_NAME_MAX: usize = 80;
pub const PLATFORM_MAX: usize = 32;

/// The fractional-index digits, in ASCII (= string compare) order.
pub const ORDER_DIGITS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// True when migration 0193's tables exist.
pub fn tables_exist(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
         AND name IN ('homes_items', 'homes_people', 'homes_devices')",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n == 3)
    .unwrap_or(false)
}

// ── Items ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Home,
    Row,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Home => "home",
            Kind::Row => "row",
        }
    }
}

/// One Home or Home row as stored (and as accepted from a push).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    /// Home: its id. Row: its Home's id.
    pub home_id: String,
    /// Row: `handle::<serverId>` (lowercase). Home: empty.
    pub address: String,
    /// Home name (None on a tombstone that didn't send one).
    pub name: Option<String>,
    /// Row label (a display hint, not identity).
    pub label: Option<String>,
    /// Row `projects.id` on its server (a rename-repair hint).
    pub workspace_id: Option<String>,
    pub order: String,
    pub updated_at: i64,
    pub device_id: String,
    pub deleted: bool,
    /// Home tombstone only: the Home it was merged into.
    pub merged_into: Option<String>,
}

impl Item {
    /// Is `self` the winner over `other` under last-writer-wins?
    /// (`updatedAt`, then `deviceId` as a string, then a tombstone wins a
    /// full tie.) A full tie with the same deleted flag is "not newer".
    pub fn beats(&self, other: &Item) -> bool {
        match self.updated_at.cmp(&other.updated_at) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => match self.device_id.as_str().cmp(other.device_id.as_str()) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => self.deleted && !other.deleted,
            },
        }
    }

    /// The wire form (camelCase). `deleted` / `mergedInto` only when set.
    pub fn to_json(&self) -> Value {
        let mut v = match self.kind {
            Kind::Home => json!({
                "kind": "home",
                "id": self.home_id,
                "name": self.name,
                "order": self.order,
                "updatedAt": self.updated_at,
                "deviceId": self.device_id,
            }),
            Kind::Row => json!({
                "kind": "row",
                "homeId": self.home_id,
                "address": self.address,
                "label": self.label.clone().unwrap_or_default(),
                "workspaceId": self.workspace_id,
                "order": self.order,
                "updatedAt": self.updated_at,
                "deviceId": self.device_id,
            }),
        };
        if self.deleted {
            v["deleted"] = json!(true);
        }
        if let Some(m) = &self.merged_into {
            v["mergedInto"] = json!(m);
        }
        v
    }
}

/// Why a pushed item was refused (`rejected[].reason`).
pub mod reason {
    pub const NOT_AN_OBJECT: &str = "not_an_object";
    pub const INVALID_KIND: &str = "invalid_kind";
    pub const INVALID_ID: &str = "invalid_id";
    pub const INVALID_HOME_ID: &str = "invalid_home_id";
    pub const INVALID_NAME: &str = "invalid_name";
    pub const INVALID_ADDRESS: &str = "invalid_address";
    pub const INVALID_LABEL: &str = "invalid_label";
    pub const INVALID_WORKSPACE_ID: &str = "invalid_workspace_id";
    pub const INVALID_ORDER: &str = "invalid_order";
    pub const INVALID_UPDATED_AT: &str = "invalid_updated_at";
    pub const FUTURE_UPDATED_AT: &str = "future_updated_at";
    pub const INVALID_DELETED: &str = "invalid_deleted";
    pub const INVALID_MERGED_INTO: &str = "invalid_merged_into";
    pub const LAST_HOME: &str = "last_home";
}

/// A valid fractional-index key: 1..=[`ORDER_MAX`] chars from
/// [`ORDER_DIGITS`], not ending in `0` (so a key always has room before it).
pub fn valid_order(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= ORDER_MAX
        && s.bytes().all(|b| b.is_ascii_alphanumeric())
        && !s.ends_with('0')
}

fn clean_name(s: &str) -> Option<String> {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(HOME_NAME_MAX).collect())
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= ID_MAX && s.trim() == s && !s.chars().any(char::is_control)
}

/// A row address `handle::host`, lowercased. `host` is a `serverId` (16
/// hex) or, for a server whose id a client could not learn yet, its host key.
pub fn normalize_address(s: &str) -> Option<String> {
    let a = s.trim().to_ascii_lowercase();
    if a.is_empty() || a.len() > ADDRESS_MAX || a.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (handle, host) = a.split_once("::")?;
    if handle.is_empty() || host.is_empty() || host.contains("::") {
        return None;
    }
    Some(a)
}

/// Parse one pushed change into an [`Item`], or the reason it is refused.
pub fn parse_change(v: &Value, device_id: &str, now_ms: i64) -> Result<Item, &'static str> {
    let o = v.as_object().ok_or(reason::NOT_AN_OBJECT)?;
    let kind = match o.get("kind").and_then(Value::as_str) {
        Some("home") => Kind::Home,
        Some("row") => Kind::Row,
        _ => return Err(reason::INVALID_KIND),
    };
    let updated_at = match o.get("updatedAt") {
        Some(Value::Number(n)) => n.as_i64().filter(|t| *t > 0).ok_or(reason::INVALID_UPDATED_AT)?,
        _ => return Err(reason::INVALID_UPDATED_AT),
    };
    if updated_at > now_ms + MAX_FUTURE_SKEW_MS {
        return Err(reason::FUTURE_UPDATED_AT);
    }
    let deleted = match o.get("deleted") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(reason::INVALID_DELETED),
    };
    let order = match o.get("order") {
        None | Some(Value::Null) if deleted => String::new(),
        Some(Value::String(s)) if valid_order(s) => s.clone(),
        // A tombstone may carry any order (it is never shown).
        Some(Value::String(_)) if deleted => String::new(),
        _ => return Err(reason::INVALID_ORDER),
    };
    match kind {
        Kind::Home => {
            let id = o.get("id").and_then(Value::as_str).filter(|s| valid_id(s)).ok_or(reason::INVALID_ID)?;
            let name = match o.get("name") {
                Some(Value::String(s)) => match clean_name(s) {
                    Some(n) => Some(n),
                    None if deleted => None,
                    None => return Err(reason::INVALID_NAME),
                },
                None | Some(Value::Null) if deleted => None,
                _ => return Err(reason::INVALID_NAME),
            };
            let merged_into = match o.get("mergedInto") {
                None | Some(Value::Null) => None,
                Some(Value::String(m)) if deleted && valid_id(m) && m != id => Some(m.clone()),
                Some(_) => return Err(reason::INVALID_MERGED_INTO),
            };
            Ok(Item {
                kind,
                home_id: id.to_string(),
                address: String::new(),
                name,
                label: None,
                workspace_id: None,
                order,
                updated_at,
                device_id: device_id.to_string(),
                deleted,
                merged_into,
            })
        }
        Kind::Row => {
            let home_id =
                o.get("homeId").and_then(Value::as_str).filter(|s| valid_id(s)).ok_or(reason::INVALID_HOME_ID)?;
            let address = o
                .get("address")
                .and_then(Value::as_str)
                .and_then(normalize_address)
                .ok_or(reason::INVALID_ADDRESS)?;
            let label = match o.get("label") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.trim().chars().take(LABEL_MAX).collect(),
                Some(_) => return Err(reason::INVALID_LABEL),
            };
            let workspace_id = match o.get("workspaceId") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if s.len() <= ID_MAX && !s.trim().is_empty() => Some(s.trim().to_string()),
                Some(Value::String(s)) if s.trim().is_empty() => None,
                Some(_) => return Err(reason::INVALID_WORKSPACE_ID),
            };
            if o.get("mergedInto").is_some_and(|m| !m.is_null()) {
                return Err(reason::INVALID_MERGED_INTO);
            }
            Ok(Item {
                kind,
                home_id: home_id.to_string(),
                address,
                name: None,
                label: Some(label),
                workspace_id,
                order,
                updated_at,
                device_id: device_id.to_string(),
                deleted,
                merged_into: None,
            })
        }
    }
}

// ── Storage ────────────────────────────────────────────────────────────

const ITEM_COLS: &str =
    "kind, home_id, address, name, label, workspace_id, ord, updated_at, device_id, deleted, merged_into";

fn item_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let kind: String = r.get(0)?;
    Ok(Item {
        kind: if kind == "row" { Kind::Row } else { Kind::Home },
        home_id: r.get(1)?,
        address: r.get(2)?,
        name: r.get(3)?,
        label: r.get(4)?,
        workspace_id: r.get(5)?,
        order: r.get(6)?,
        updated_at: r.get(7)?,
        device_id: r.get(8)?,
        deleted: r.get::<_, i64>(9)? != 0,
        merged_into: r.get(10)?,
    })
}

fn get_item(conn: &Connection, person: &str, item: &Item) -> rusqlite::Result<Option<Item>> {
    conn.query_row(
        &format!("SELECT {ITEM_COLS} FROM homes_items WHERE person = ?1 AND kind = ?2 AND home_id = ?3 AND address = ?4"),
        params![person, item.kind.as_str(), item.home_id, item.address],
        item_from_row,
    )
    .optional()
}

fn person_seq(conn: &Connection, person: &str) -> rusqlite::Result<(i64, i64)> {
    Ok(conn
        .query_row("SELECT seq, floor FROM homes_people WHERE person = ?1", params![person], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .unwrap_or((0, 0)))
}

fn live_home_count_except(conn: &Connection, person: &str, except_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM homes_items WHERE person = ?1 AND kind = 'home' AND deleted = 0 AND home_id != ?2",
        params![person, except_id],
        |r| r.get(0),
    )
}

/// Where a device stands for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    /// Registered and not removed: it syncs.
    Active,
    /// Removed from Server settings: pulls and pushes answer 410.
    Removed,
    /// Never registered with this person.
    Unknown,
}

pub fn device_state(conn: &Connection, person: &str, device_id: &str) -> rusqlite::Result<DeviceState> {
    let row: Option<Option<i64>> = conn
        .query_row(
            "SELECT removed_at FROM homes_devices WHERE person = ?1 AND device_id = ?2",
            params![person, device_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match row {
        None => DeviceState::Unknown,
        Some(None) => DeviceState::Active,
        Some(Some(_)) => DeviceState::Removed,
    })
}

fn touch_device(conn: &Connection, person: &str, device_id: &str, now_ms: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE homes_devices SET last_sync_at = ?3 WHERE person = ?1 AND device_id = ?2 AND removed_at IS NULL",
        params![person, device_id, now_ms],
    )?;
    Ok(())
}

/// A valid device id: 1..=128 chars of `[A-Za-z0-9._:-]`.
pub fn valid_device_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= ID_MAX && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

// ── Pull ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    /// The person's cursor now (pass it back as `since`).
    pub cursor: i64,
    /// True when `changes` is the whole live set (no `since`, or the cursor
    /// can't be served incrementally): the client replaces, not patches.
    pub full: bool,
    pub changes: Vec<Item>,
    /// Every merged-away Home id → the Home it went into.
    pub redirects: Vec<(String, String)>,
}

/// Read a person's changes since `since` (None = everything live). Updates
/// the device's `lastSyncAt` when it is an active device.
pub fn pull(
    conn: &Connection,
    person: &str,
    since: Option<i64>,
    device_id: Option<&str>,
    now_ms: i64,
) -> rusqlite::Result<Pull> {
    let (seq, floor) = person_seq(conn, person)?;
    let full = match since {
        None => true,
        Some(s) => s < floor || s > seq || s < 0,
    };
    let changes: Vec<Item> = if full {
        let mut st = conn.prepare(&format!(
            "SELECT {ITEM_COLS} FROM homes_items WHERE person = ?1 AND deleted = 0 ORDER BY kind, home_id, ord, address"
        ))?;
        let rows = st.query_map(params![person], item_from_row)?;
        rows.collect::<rusqlite::Result<_>>()?
    } else {
        let mut st = conn.prepare(&format!(
            "SELECT {ITEM_COLS} FROM homes_items WHERE person = ?1 AND seq > ?2 ORDER BY seq, kind, home_id, address"
        ))?;
        let rows = st.query_map(params![person, since.unwrap_or(0)], item_from_row)?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut st = conn.prepare(
        "SELECT home_id, merged_into FROM homes_items \
         WHERE person = ?1 AND kind = 'home' AND deleted = 1 AND merged_into IS NOT NULL ORDER BY home_id",
    )?;
    let redirects = st
        .query_map(params![person], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if let Some(d) = device_id {
        touch_device(conn, person, d, now_ms)?;
    }
    Ok(Pull { cursor: seq, full, changes, redirects })
}

// ── Push ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Rejected {
    pub item: Value,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PushResult {
    /// The person's cursor after the push.
    pub cursor: i64,
    /// Items written (newer than what the server held).
    pub applied: usize,
    /// Items the server already had a newer (or equal) version of. Not an
    /// error: last writer wins, and the next pull brings the winner.
    pub superseded: usize,
    pub rejected: Vec<Rejected>,
}

impl PushResult {
    pub fn changed(&self) -> bool {
        self.applied > 0
    }
}

/// Merge `changes` from `device_id` into `person`'s items (one transaction).
/// Live items are applied before tombstones, so a batch that creates a Home
/// and deletes the old one never trips the last-Home rule.
pub fn push(
    conn: &Connection,
    person: &str,
    device_id: &str,
    changes: &[Value],
    now_ms: i64,
) -> rusqlite::Result<PushResult> {
    let mut rejected = Vec::new();
    let mut parsed: Vec<Item> = Vec::new();
    for c in changes {
        match parse_change(c, device_id, now_ms) {
            Ok(item) => parsed.push(item),
            Err(reason) => rejected.push(Rejected { item: c.clone(), reason }),
        }
    }
    // Live first, tombstones after; stable within each group.
    parsed.sort_by_key(|i| i.deleted);

    let tx = conn.unchecked_transaction()?;
    let (seq, mut floor) = person_seq(&tx, person)?;
    let next = seq + 1;
    let mut applied = 0usize;
    let mut superseded = 0usize;
    for item in parsed {
        let existing = get_item(&tx, person, &item)?;
        if let Some(ex) = &existing {
            if !item.beats(ex) {
                superseded += 1;
                continue;
            }
        }
        let mut item = item;
        if item.kind == Kind::Home && item.deleted && existing.as_ref().is_some_and(|e| !e.deleted) {
            if live_home_count_except(&tx, person, &item.home_id)? == 0 {
                rejected.push(Rejected { item: item.to_json(), reason: reason::LAST_HOME });
                continue;
            }
        }
        // A tombstone keeps the last known name/label/order for context.
        if item.deleted {
            if let Some(ex) = &existing {
                if item.name.is_none() {
                    item.name = ex.name.clone();
                }
                if item.kind == Kind::Row && item.label.as_deref().unwrap_or("").is_empty() {
                    item.label = ex.label.clone();
                }
                if item.order.is_empty() {
                    item.order = ex.order.clone();
                }
            }
        }
        tx.execute(
            "INSERT INTO homes_items (person, kind, home_id, address, name, label, workspace_id, ord, \
               updated_at, device_id, deleted, merged_into, seq) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
             ON CONFLICT (person, kind, home_id, address) DO UPDATE SET \
               name = excluded.name, label = excluded.label, workspace_id = excluded.workspace_id, \
               ord = excluded.ord, updated_at = excluded.updated_at, device_id = excluded.device_id, \
               deleted = excluded.deleted, merged_into = excluded.merged_into, seq = excluded.seq",
            params![
                person,
                item.kind.as_str(),
                item.home_id,
                item.address,
                item.name,
                item.label,
                item.workspace_id,
                item.order,
                item.updated_at,
                item.device_id,
                item.deleted as i64,
                item.merged_into,
                next
            ],
        )?;
        applied += 1;
    }
    let cursor = if applied > 0 {
        // Collect old tombstones (never redirects) and raise the floor past
        // them, so a device that last pulled before them gets a full set.
        let cutoff = now_ms - TOMBSTONE_KEEP_MS;
        let purged_max: Option<i64> = tx.query_row(
            "SELECT MAX(seq) FROM homes_items WHERE person = ?1 AND deleted = 1 AND merged_into IS NULL AND updated_at < ?2",
            params![person, cutoff],
            |r| r.get(0),
        )?;
        if let Some(m) = purged_max {
            tx.execute(
                "DELETE FROM homes_items WHERE person = ?1 AND deleted = 1 AND merged_into IS NULL AND updated_at < ?2",
                params![person, cutoff],
            )?;
            floor = floor.max(m);
        }
        tx.execute(
            "INSERT INTO homes_people (person, seq, floor) VALUES (?1, ?2, ?3) \
             ON CONFLICT (person) DO UPDATE SET seq = excluded.seq, floor = excluded.floor",
            params![person, next, floor],
        )?;
        next
    } else {
        seq
    };
    touch_device(&tx, person, device_id, now_ms)?;
    tx.commit()?;
    Ok(PushResult { cursor, applied, superseded, rejected })
}

// ── Devices ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub registered_at: i64,
    pub last_sync_at: Option<i64>,
}

impl Device {
    pub fn to_json(&self, current: Option<&str>) -> Value {
        json!({
            "deviceId": self.device_id,
            "name": self.name,
            "platform": self.platform,
            "registeredAt": self.registered_at,
            "lastSyncAt": self.last_sync_at,
            "current": current == Some(self.device_id.as_str()),
        })
    }
}

/// Clean a device name (trim, collapse whitespace, cap). None when empty.
pub fn clean_device_name(s: &str) -> Option<String> {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then(|| collapsed.chars().take(DEVICE_NAME_MAX).collect())
}

/// Clean a platform tag (`ios`, `android`, `macos`, `web`, …): trimmed,
/// lowercased, `[a-z0-9._-]`, capped. None when empty or malformed.
pub fn clean_platform(s: &str) -> Option<String> {
    let p = s.trim().to_ascii_lowercase();
    (!p.is_empty() && p.len() <= PLATFORM_MAX && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
        .then_some(p)
}

/// Register (or re-register) a device. A removed device syncs again.
pub fn register_device(
    conn: &Connection,
    person: &str,
    device_id: &str,
    name: &str,
    platform: &str,
    now_ms: i64,
) -> rusqlite::Result<Device> {
    conn.execute(
        "INSERT INTO homes_devices (person, device_id, name, platform, registered_at, last_sync_at, removed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL) \
         ON CONFLICT (person, device_id) DO UPDATE SET name = excluded.name, platform = excluded.platform, \
           registered_at = CASE WHEN homes_devices.removed_at IS NULL THEN homes_devices.registered_at ELSE excluded.registered_at END, \
           removed_at = NULL",
        params![person, device_id, name, platform, now_ms],
    )?;
    conn.query_row(
        "SELECT device_id, name, platform, registered_at, last_sync_at FROM homes_devices WHERE person = ?1 AND device_id = ?2",
        params![person, device_id],
        |r| {
            Ok(Device {
                device_id: r.get(0)?,
                name: r.get(1)?,
                platform: r.get(2)?,
                registered_at: r.get(3)?,
                last_sync_at: r.get(4)?,
            })
        },
    )
}

/// A person's active devices, most recently synced first.
pub fn list_devices(conn: &Connection, person: &str) -> rusqlite::Result<Vec<Device>> {
    let mut st = conn.prepare(
        "SELECT device_id, name, platform, registered_at, last_sync_at FROM homes_devices \
         WHERE person = ?1 AND removed_at IS NULL \
         ORDER BY last_sync_at IS NULL, last_sync_at DESC, registered_at DESC, device_id",
    )?;
    let rows = st.query_map(params![person], |r| {
        Ok(Device {
            device_id: r.get(0)?,
            name: r.get(1)?,
            platform: r.get(2)?,
            registered_at: r.get(3)?,
            last_sync_at: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// What [`remove_device`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,
    AlreadyRemoved,
    Unknown,
}

pub fn remove_device(conn: &Connection, person: &str, device_id: &str, now_ms: i64) -> rusqlite::Result<Removal> {
    Ok(match device_state(conn, person, device_id)? {
        DeviceState::Unknown => Removal::Unknown,
        DeviceState::Removed => Removal::AlreadyRemoved,
        DeviceState::Active => {
            conn.execute(
                "UPDATE homes_devices SET removed_at = ?3 WHERE person = ?1 AND device_id = ?2",
                params![person, device_id, now_ms],
            )?;
            Removal::Removed
        }
    })
}

// ── Fractional index (shared with the clients' helper) ──────────────────

/// A key strictly between `a` and `b` (`None` = open end). Both must be
/// [`valid_order`] when given, and `a < b`. Mirrors
/// `src/renderer/lib/fractional-index.ts` `orderBetween`.
pub fn order_between(a: Option<&str>, b: Option<&str>) -> Result<String, String> {
    let digits = ORDER_DIGITS.as_bytes();
    for k in [a, b].into_iter().flatten() {
        if !valid_order(k) {
            return Err(format!("invalid order key {k:?}"));
        }
    }
    if let (Some(x), Some(y)) = (a, b) {
        if x >= y {
            return Err(format!("order keys out of order: {x:?} >= {y:?}"));
        }
    }
    fn mid(a: &[u8], b: Option<&[u8]>, digits: &[u8]) -> Vec<u8> {
        let idx = |c: u8| digits.iter().position(|d| *d == c).expect("valid digit");
        if let Some(b) = b {
            let mut n = 0;
            while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
                n += 1;
            }
            if n > 0 {
                let mut out = b[..n].to_vec();
                out.extend(mid(if n <= a.len() { &a[n..] } else { &[] }, Some(&b[n..]), digits));
                return out;
            }
        }
        let da = a.first().map(|c| idx(*c)).unwrap_or(0);
        let db = b.and_then(|b| b.first()).map(|c| idx(*c)).unwrap_or(digits.len());
        if db - da > 1 {
            return vec![digits[(da + db + 1) / 2]];
        }
        if let Some(b) = b {
            if b.len() > 1 {
                return vec![b[0]];
            }
        }
        let mut out = vec![digits[da]];
        out.extend(mid(if a.is_empty() { &[] } else { &a[1..] }, None, digits));
        out
    }
    let out = mid(a.unwrap_or("").as_bytes(), b.map(str::as_bytes), digits);
    Ok(String::from_utf8(out).expect("ascii digits"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000_000;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("mem db");
        conn.execute_batch(include_str!("../drizzle_sql/0193_homes_sync.sql").replace("--> statement-breakpoint", "").as_str())
            .expect("0193 applies");
        assert!(tables_exist(&conn));
        conn
    }

    fn home(id: &str, name: &str, order: &str, at: i64) -> Value {
        json!({"kind": "home", "id": id, "name": name, "order": order, "updatedAt": at})
    }

    fn row(home: &str, addr: &str, order: &str, at: i64) -> Value {
        json!({"kind": "row", "homeId": home, "address": addr, "label": addr, "workspaceId": null, "order": order, "updatedAt": at})
    }

    fn tomb(mut v: Value) -> Value {
        v["deleted"] = json!(true);
        v
    }

    fn live(conn: &Connection, person: &str) -> Vec<Value> {
        pull(conn, person, None, None, NOW).expect("pull").changes.iter().map(Item::to_json).collect()
    }

    fn names(conn: &Connection, person: &str) -> Vec<String> {
        live(conn, person)
            .iter()
            .filter(|v| v["kind"] == "home")
            .map(|v| v["name"].as_str().expect("name").to_string())
            .collect()
    }

    #[test]
    fn concurrent_edits_last_writer_wins_ties_by_device_id() {
        let c = db();
        push(&c, "owner", "laptop", &[home("h1", "Home", "V", NOW)], NOW).expect("seed");
        // Phone renames later than the laptop: the phone wins, whatever order they arrive in.
        let r = push(&c, "owner", "phone", &[home("h1", "Phone name", "V", NOW + 20)], NOW).expect("phone");
        assert_eq!((r.applied, r.superseded), (1, 0));
        let r = push(&c, "owner", "laptop", &[home("h1", "Laptop name", "V", NOW + 10)], NOW).expect("laptop late");
        assert_eq!((r.applied, r.superseded), (0, 1), "older edit loses");
        assert_eq!(names(&c, "owner"), vec!["Phone name"]);
        // Same millisecond: the larger deviceId wins (string compare).
        push(&c, "owner", "a-dev", &[home("h1", "From a", "V", NOW + 30)], NOW).expect("a");
        let r = push(&c, "owner", "b-dev", &[home("h1", "From b", "V", NOW + 30)], NOW).expect("b");
        assert_eq!(r.applied, 1, "b > a on a tie");
        let r = push(&c, "owner", "a-dev", &[home("h1", "From a again", "V", NOW + 30)], NOW).expect("a again");
        assert_eq!(r.applied, 0, "a < b on a tie");
        assert_eq!(names(&c, "owner"), vec!["From b"]);
        // Re-sending the exact same item is a no-op (idempotent retry).
        let r = push(&c, "owner", "b-dev", &[home("h1", "From b", "V", NOW + 30)], NOW).expect("retry");
        assert_eq!((r.applied, r.superseded, r.cursor), (0, 1, 4), "no write, no cursor bump");
    }

    #[test]
    fn delete_vs_edit_and_tombstones_keep_stale_devices_from_resurrecting() {
        let c = db();
        push(&c, "owner", "laptop", &[home("h1", "Home", "V", NOW), row("h1", "atlas::aaaa", "V", NOW)], NOW).expect("seed");
        // Phone deletes the row at t+10; the laptop's older edit (t+5) arrives later and loses.
        push(&c, "owner", "phone", &[tomb(row("h1", "atlas::aaaa", "V", NOW + 10))], NOW).expect("delete");
        let r = push(&c, "owner", "laptop", &[row("h1", "atlas::aaaa", "k", NOW + 5)], NOW).expect("stale edit");
        assert_eq!(r.applied, 0, "a tombstone beats an older edit");
        assert!(live(&c, "owner").iter().all(|v| v["kind"] != "row"), "row stays deleted");
        // A stale device re-pushing its old copy (same old stamp) can't bring it back.
        let r = push(&c, "owner", "laptop", &[row("h1", "atlas::aaaa", "V", NOW)], NOW).expect("stale copy");
        assert_eq!(r.applied, 0);
        // An edit made after the delete brings it back.
        let r = push(&c, "owner", "laptop", &[row("h1", "atlas::aaaa", "k", NOW + 20)], NOW).expect("re-add");
        assert_eq!(r.applied, 1);
        assert_eq!(live(&c, "owner").iter().filter(|v| v["kind"] == "row").count(), 1);
        // The incremental pull carries the tombstone with `deleted: true`.
        let since0 = pull(&c, "owner", Some(1), None, NOW).expect("since 1");
        assert!(!since0.full);
        let p = pull(&c, "owner", Some(0), None, NOW).expect("since 0");
        assert!(!p.full, "cursor 0 with floor 0 is incremental");
        push(&c, "owner", "phone", &[tomb(row("h1", "atlas::aaaa", "V", NOW + 30))], NOW).expect("delete again");
        let p = pull(&c, "owner", Some(r.cursor), None, NOW).expect("after re-add");
        assert_eq!(p.changes.len(), 1);
        assert_eq!(p.changes[0].to_json()["deleted"], json!(true), "tombstone in the delta");
        // A tombstone and an edit at the very same stamp from the same device: the tombstone wins.
        let r = push(&c, "owner", "phone", &[row("h1", "atlas::aaaa", "V", NOW + 30)], NOW).expect("tie");
        assert_eq!(r.applied, 0);
    }

    #[test]
    fn full_pull_omits_tombstones_and_cursor_rules() {
        let c = db();
        let r = push(&c, "owner", "d", &[home("h1", "One", "V", NOW), home("h2", "Two", "k", NOW)], NOW).expect("seed");
        assert_eq!(r.cursor, 1);
        push(&c, "owner", "d", &[tomb(home("h2", "Two", "k", NOW + 1))], NOW).expect("delete h2");
        let p = pull(&c, "owner", None, None, NOW).expect("full");
        assert!(p.full);
        assert_eq!(p.cursor, 2);
        assert_eq!(p.changes.len(), 1, "no tombstones in a full pull: {:?}", p.changes);
        // A cursor from the future (another server, a reset DB) gets the full set.
        assert!(pull(&c, "owner", Some(99), None, NOW).expect("future").full);
        // Nothing new since the current cursor.
        let p = pull(&c, "owner", Some(2), None, NOW).expect("current");
        assert!(!p.full && p.changes.is_empty() && p.cursor == 2);
        // An empty person: cursor 0, empty, full.
        let p = pull(&c, "nobody", None, None, NOW).expect("empty");
        assert_eq!((p.cursor, p.full, p.changes.len()), (0, true, 0));
    }

    #[test]
    fn merged_homes_keep_their_ids_as_redirects() {
        let c = db();
        push(&c, "owner", "laptop", &[home("srv-home", "Home", "V", NOW)], NOW).expect("server copy");
        // The phone's first sync: its own "Home" (local-home) merges into srv-home.
        let mut t = tomb(home("local-home", "Home", "V", NOW + 1));
        t["mergedInto"] = json!("srv-home");
        let r = push(&c, "owner", "phone", &[home("local-home", "Home", "V", NOW), t], NOW).expect("merge");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
        let p = pull(&c, "owner", None, None, NOW).expect("full");
        assert_eq!(p.redirects, vec![("local-home".to_string(), "srv-home".to_string())]);
        assert_eq!(names(&c, "owner"), vec!["Home"], "one Home after the union");
        // An incremental pull carries the redirect on the tombstone too.
        let p = pull(&c, "owner", Some(1), None, NOW).expect("delta");
        let j: Vec<Value> = p.changes.iter().map(Item::to_json).collect();
        assert!(j.iter().any(|v| v["id"] == "local-home" && v["mergedInto"] == "srv-home" && v["deleted"] == true), "{j:?}");
        // mergedInto on a live Home, on itself, or on a row is refused.
        let mut bad = home("x", "X", "V", NOW);
        bad["mergedInto"] = json!("srv-home");
        let mut own = tomb(home("y", "Y", "V", NOW));
        own["mergedInto"] = json!("y");
        let mut rowm = row("srv-home", "a::b", "V", NOW);
        rowm["mergedInto"] = json!("srv-home");
        let r = push(&c, "owner", "phone", &[bad, own, rowm], NOW).expect("bad merges");
        assert_eq!(r.rejected.iter().map(|x| x.reason).collect::<Vec<_>>(), vec![reason::INVALID_MERGED_INTO; 3]);
        // Redirect tombstones are never collected.
        push(&c, "owner", "phone", &[home("z", "Z", "k", NOW + TOMBSTONE_KEEP_MS * 2)], NOW + TOMBSTONE_KEEP_MS * 2).expect("gc pass");
        assert_eq!(pull(&c, "owner", None, None, NOW).expect("full").redirects.len(), 1);
    }

    #[test]
    fn the_last_live_home_is_never_deleted() {
        let c = db();
        push(&c, "owner", "d", &[home("h1", "Home", "V", NOW)], NOW).expect("seed");
        let r = push(&c, "owner", "d", &[tomb(home("h1", "Home", "V", NOW + 1))], NOW).expect("delete last");
        assert_eq!(r.rejected.len(), 1);
        assert_eq!(r.rejected[0].reason, reason::LAST_HOME);
        assert_eq!(names(&c, "owner"), vec!["Home"]);
        // Creating a replacement in the same batch (in any order) lets the old one go.
        let r = push(&c, "owner", "d", &[tomb(home("h1", "Home", "V", NOW + 2)), home("h2", "Work", "k", NOW + 2)], NOW)
            .expect("swap");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
        assert_eq!(names(&c, "owner"), vec!["Work"]);
    }

    #[test]
    fn validation_reasons() {
        let c = db();
        let cases = vec![
            (json!("nope"), reason::NOT_AN_OBJECT),
            (json!({"kind": "garden"}), reason::INVALID_KIND),
            (json!({"kind": "home", "id": "h", "name": "x", "order": "V"}), reason::INVALID_UPDATED_AT),
            (home("h", "x", "V", NOW + MAX_FUTURE_SKEW_MS + 1), reason::FUTURE_UPDATED_AT),
            (home("", "x", "V", NOW), reason::INVALID_ID),
            (home("h", "   ", "V", NOW), reason::INVALID_NAME),
            (home("h", "x", "V0", NOW), reason::INVALID_ORDER),
            (home("h", "x", "", NOW), reason::INVALID_ORDER),
            (home("h", "x", "a.b", NOW), reason::INVALID_ORDER),
            (row("h", "no-separator", "V", NOW), reason::INVALID_ADDRESS),
            (row("h", "a::", "V", NOW), reason::INVALID_ADDRESS),
            (row("", "a::b", "V", NOW), reason::INVALID_HOME_ID),
            (json!({"kind": "row", "homeId": "h", "address": "a::b", "label": 5, "order": "V", "updatedAt": NOW}), reason::INVALID_LABEL),
            (json!({"kind": "home", "id": "h", "name": "x", "order": "V", "updatedAt": NOW, "deleted": "yes"}), reason::INVALID_DELETED),
        ];
        let input: Vec<Value> = cases.iter().map(|(v, _)| v.clone()).collect();
        let r = push(&c, "owner", "d", &input, NOW).expect("push");
        assert_eq!(r.applied, 0);
        let got: Vec<&str> = r.rejected.iter().map(|x| x.reason).collect();
        let want: Vec<&str> = cases.iter().map(|(_, w)| *w).collect();
        assert_eq!(got, want);
        assert_eq!(r.cursor, 0, "nothing changed, cursor unchanged");
        // Names are cleaned; addresses lowercased; a tombstone needs no name or order.
        let r = push(
            &c,
            "owner",
            "d",
            &[home("h", "  My   Home  ", "V", NOW), row("h", "Atlas::ABCD", "V", NOW), json!({"kind": "home", "id": "gone", "updatedAt": NOW, "deleted": true})],
            NOW,
        )
        .expect("ok");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
        let l = live(&c, "owner");
        assert!(l.iter().any(|v| v["name"] == "My Home"), "{l:?}");
        assert!(l.iter().any(|v| v["address"] == "atlas::abcd"), "{l:?}");
    }

    #[test]
    fn people_are_isolated() {
        let c = db();
        push(&c, "owner", "d", &[home("h1", "Owner home", "V", NOW)], NOW).expect("owner");
        push(&c, "gina", "g", &[home("h1", "Gina home", "V", NOW + 5)], NOW).expect("gina");
        assert_eq!(names(&c, "owner"), vec!["Owner home"], "same Home id, different people");
        assert_eq!(names(&c, "gina"), vec!["Gina home"]);
        assert_eq!(pull(&c, "owner", None, None, NOW).expect("o").cursor, 1);
        assert_eq!(pull(&c, "gina", None, None, NOW).expect("g").cursor, 1, "cursors are per person");
        register_device(&c, "owner", "phone", "Phone", "ios", NOW).expect("reg");
        assert!(list_devices(&c, "gina").expect("gina devices").is_empty());
        assert_eq!(device_state(&c, "gina", "phone").expect("state"), DeviceState::Unknown);
    }

    #[test]
    fn offline_queue_replay_merges_with_concurrent_server_changes() {
        // The phone is offline and queues edits; meanwhile the laptop edits.
        // On reconnect the phone replays its queue (with its original stamps).
        let c = db();
        push(&c, "owner", "laptop", &[home("h1", "Home", "V", NOW), row("h1", "a::s1", "V", NOW), row("h1", "b::s1", "k", NOW)], NOW)
            .expect("seed");
        let base = pull(&c, "owner", None, None, NOW).expect("phone pulls before going offline").cursor;
        // Laptop, online: renames the Home at t+50 and deletes row b at t+60.
        push(&c, "owner", "laptop", &[home("h1", "Laptop name", "V", NOW + 50), tomb(row("h1", "b::s1", "k", NOW + 60))], NOW)
            .expect("laptop");
        // Phone queue (offline): rename at t+40 (older), add row c at t+45, move row a at t+70.
        let queue = vec![home("h1", "Phone name", "V", NOW + 40), row("h1", "c::s2", "s", NOW + 45), row("h1", "a::s1", "z", NOW + 70)];
        let r = push(&c, "owner", "phone", &queue, NOW + 100).expect("replay");
        assert_eq!((r.applied, r.superseded), (2, 1), "the older rename loses, the rest lands");
        // Replaying the same queue again (a lost response) changes nothing.
        let again = push(&c, "owner", "phone", &queue, NOW + 101).expect("replay twice");
        assert_eq!((again.applied, again.cursor), (0, r.cursor));
        // The phone catches up from its old cursor and sees everything it missed.
        let p = pull(&c, "owner", Some(base), None, NOW).expect("catch up");
        let j: Vec<Value> = p.changes.iter().map(Item::to_json).collect();
        assert!(j.iter().any(|v| v["name"] == "Laptop name"), "{j:?}");
        assert!(j.iter().any(|v| v["address"] == "b::s1" && v["deleted"] == true), "{j:?}");
        let l = live(&c, "owner");
        let rows: Vec<(&str, &str)> = l
            .iter()
            .filter(|v| v["kind"] == "row")
            .map(|v| (v["address"].as_str().unwrap(), v["order"].as_str().unwrap()))
            .collect();
        let mut sorted = rows.clone();
        sorted.sort_by(|x, y| x.1.cmp(y.1));
        assert_eq!(sorted, vec![("c::s2", "s"), ("a::s1", "z")]);
    }

    #[test]
    fn tombstones_are_collected_and_the_floor_forces_a_full_pull() {
        let c = db();
        push(&c, "owner", "d", &[home("h1", "Home", "V", NOW), row("h1", "a::s", "V", NOW)], NOW).expect("seed");
        let old = pull(&c, "owner", None, None, NOW).expect("p").cursor;
        push(&c, "owner", "d", &[tomb(row("h1", "a::s", "V", NOW + 1))], NOW).expect("tomb");
        // 91 days later, any write collects it.
        let later = NOW + TOMBSTONE_KEEP_MS + 10;
        push(&c, "owner", "d", &[home("h1", "Home 2", "V", later)], later).expect("later write");
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM homes_items WHERE person = 'owner' AND deleted = 1", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0, "collected");
        let p = pull(&c, "owner", Some(old), None, later).expect("old cursor");
        assert!(p.full, "a cursor below the floor gets the whole set");
    }

    #[test]
    fn devices_register_list_remove_and_re_register() {
        let c = db();
        assert_eq!(device_state(&c, "owner", "phone").unwrap(), DeviceState::Unknown);
        register_device(&c, "owner", "phone", "Rosson's phone", "ios", NOW).expect("reg phone");
        register_device(&c, "owner", "desk", "Desk", "macos", NOW + 1).expect("reg desk");
        pull(&c, "owner", None, Some("phone"), NOW + 5).expect("phone pulls");
        let l = list_devices(&c, "owner").expect("list");
        assert_eq!(l.iter().map(|d| d.device_id.as_str()).collect::<Vec<_>>(), vec!["phone", "desk"], "synced first");
        assert_eq!(l[0].last_sync_at, Some(NOW + 5));
        assert_eq!(l[0].to_json(Some("phone"))["current"], true);
        assert_eq!(l[1].to_json(Some("phone"))["current"], false);
        assert_eq!(remove_device(&c, "owner", "phone", NOW + 6).unwrap(), Removal::Removed);
        assert_eq!(remove_device(&c, "owner", "phone", NOW + 7).unwrap(), Removal::AlreadyRemoved);
        assert_eq!(remove_device(&c, "owner", "ghost", NOW + 7).unwrap(), Removal::Unknown);
        assert_eq!(device_state(&c, "owner", "phone").unwrap(), DeviceState::Removed);
        assert_eq!(list_devices(&c, "owner").unwrap().len(), 1);
        // A removed device's pull doesn't touch its row.
        pull(&c, "owner", None, Some("phone"), NOW + 8).expect("pull");
        // Registering again re-syncs it.
        let d = register_device(&c, "owner", "phone", "Phone", "ios", NOW + 9).expect("re-register");
        assert_eq!(d.registered_at, NOW + 9);
        assert_eq!(device_state(&c, "owner", "phone").unwrap(), DeviceState::Active);
        assert!(valid_device_id("d-1.2:3_x") && !valid_device_id("a b") && !valid_device_id(""));
        assert_eq!(clean_platform(" iOS "), Some("ios".into()));
        assert_eq!(clean_platform("mac os"), None);
        assert_eq!(clean_device_name("  My   Phone "), Some("My Phone".into()));
    }

    #[test]
    fn order_between_matches_the_client_helper() {
        assert_eq!(order_between(None, None).unwrap(), "V");
        assert_eq!(order_between(Some("V"), None).unwrap(), "l");
        assert_eq!(order_between(None, Some("V")).unwrap(), "G");
        assert_eq!(order_between(Some("z"), None).unwrap(), "zV");
        assert_eq!(order_between(Some("a"), Some("b")).unwrap(), "aV");
        assert_eq!(order_between(Some("a1"), Some("a2")).unwrap(), "a1V");
        assert_eq!(order_between(None, Some("05")).unwrap(), "03");
        assert!(order_between(Some("b"), Some("a")).is_err());
        assert!(order_between(Some("a0"), None).is_err());
        // A long run of appends and prepends keeps strict order and valid keys.
        let mut keys = vec![order_between(None, None).unwrap()];
        for _ in 0..200 {
            let last = keys.last().unwrap().clone();
            keys.push(order_between(Some(&last), None).unwrap());
            let first = keys[0].clone();
            keys.insert(0, order_between(None, Some(&first)).unwrap());
        }
        for w in keys.windows(2) {
            assert!(w[0] < w[1] && valid_order(&w[1]), "{w:?}");
        }
        // Repeated inserts between two neighbours stay strictly between.
        let (mut lo, hi) = ("V".to_string(), "W".to_string());
        for _ in 0..100 {
            let m = order_between(Some(&lo), Some(&hi)).unwrap();
            assert!(lo < m && m < hi && valid_order(&m), "{lo} {m} {hi}");
            lo = m;
        }
    }
}
