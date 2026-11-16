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
//! - Home: `{kind:"home", id, name, order, updatedAt, deleted?, mergedInto?,
//!   icon?, recentFirst?}` — `icon` is a Lucide icon name (`[a-z0-9-]{1,64}`)
//!   or null; `recentFirst` sorts its unpinned rows by `lastMessagedAt`.
//! - Row:  `{kind:"row", homeId, address:"handle::<serverId>", label,
//!   workspaceId, order, updatedAt, deleted?, pinOrder?, lastMessagedAt?}` —
//!   identity `(kind, homeId, address)`. Two more row kinds carry the same
//!   fields (migration 0306, prd-home-people-channels-v1 §5): `person`
//!   (`address` = `<external id>::<serverId>`, `k2:owner` or
//!   `k2:connect:<uuid>`: the DM with that person there) and `channel`
//!   (`address` = `<Sew channel id>::<serverId>`). An older client drops
//!   kinds it doesn't know; an older server refuses them one item at a time
//!   (`invalid_kind`). `pinOrder` is a fractional-index key among
//!   the Home's pinned rows (null = not pinned; at most [`MAX_PINS`] per
//!   Home); `lastMessagedAt` is the person's last send to that agent (epoch
//!   ms; a push never moves it back).
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
//! **Where they are stored.** Homes and agent rows in `homes_items`; people
//! and channel rows in `homes_chat_items` (migration 0307), because 0.46.12
//! reads every `homes_items` row and takes any kind but `row` for a Home: a
//! server rolled back to it must not show its clients a person or a channel
//! as an extra Home. The person's cursor is shared; a pull reads both.
//!
//! **Fields an older client leaves out.** The fields added after 0.46.1
//! (`icon`, `recentFirst`, `pinOrder`, `lastMessagedAt`, migration 0305) are
//! optional on a push: left out, the stored value stays (an older device's
//! rename must not wipe the icon or pins a newer one set); JSON `null` clears
//! it (`recentFirst`: null is the same as left out).
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
/// The longest Home icon name (a Lucide icon name, kebab-case).
pub const ICON_MAX: usize = 64;
/// The most pinned rows one Home may have (⌘1–9 opens them).
pub const MAX_PINS: usize = 9;

/// The fractional-index digits, in ASCII (= string compare) order.
pub const ORDER_DIGITS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A Home icon name: 1..=[`ICON_MAX`] chars of `[a-z0-9-]`.
pub fn valid_icon(s: &str) -> bool {
    !s.is_empty() && s.len() <= ICON_MAX && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// True when migration 0193's tables (and 0307's) exist.
pub fn tables_exist(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
         AND name IN ('homes_items', 'homes_chat_items', 'homes_people', 'homes_devices')",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n == 4)
    .unwrap_or(false)
}

// ── Items ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Home,
    /// An agent row (`handle::<serverId>`).
    Row,
    /// A person on a server (`<external id>::<serverId>`): their DM.
    Person,
    /// A Sew channel on a server (`<channel id>::<serverId>`).
    Channel,
}

/// Every item kind this store takes (the pull answer's `kinds`, so a client
/// knows the server keeps people and channels).
pub const KINDS: &[&str] = &["home", "row", "person", "channel"];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Home => "home",
            Kind::Row => "row",
            Kind::Person => "person",
            Kind::Channel => "channel",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "home" => Some(Kind::Home),
            "row" => Some(Kind::Row),
            "person" => Some(Kind::Person),
            "channel" => Some(Kind::Channel),
            _ => None,
        }
    }

    /// A row of a Home (agent, person or channel), not a Home.
    pub fn is_row(self) -> bool {
        self != Kind::Home
    }

    /// The table this kind is stored in: Homes and agent rows in
    /// [`HOMES_TABLE`] (every release reads it), people and channels in
    /// [`CHAT_TABLE`] (0307), where 0.46.12 never looks.
    pub fn table(self) -> &'static str {
        match self {
            Kind::Home | Kind::Row => HOMES_TABLE,
            Kind::Person | Kind::Channel => CHAT_TABLE,
        }
    }
}

/// Homes and agent rows (0193). 0.46.12 reads every row of a person here and
/// takes any kind but `row` for a Home, so nothing else may be stored here.
pub const HOMES_TABLE: &str = "homes_items";
/// People and channel rows (0307), same columns.
pub const CHAT_TABLE: &str = "homes_chat_items";

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
    /// Home only: its Lucide icon name, None for none.
    pub icon: Option<String>,
    /// Home only: unpinned rows sort by their last send, newest first.
    pub recent_first: bool,
    /// Row only: its key among the Home's pinned rows, None when not pinned.
    pub pin_order: Option<String>,
    /// Row only: the person's last send to it (epoch ms).
    pub last_messaged_at: Option<i64>,
    /// The optional fields this push left out (an older client): each
    /// keeps the stored value. Always empty on an item read from storage.
    pub omitted: Omitted,
}

/// Which optional fields a pushed item left out (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Omitted {
    pub icon: bool,
    pub recent_first: bool,
    pub pin_order: bool,
    pub last_messaged_at: bool,
}

impl Item {
    /// Fill every field this push left out from what the server holds (or
    /// the default when it holds nothing).
    fn keep_omitted(&mut self, existing: Option<&Item>) {
        if self.omitted.icon {
            self.icon = existing.and_then(|e| e.icon.clone());
        }
        if self.omitted.recent_first {
            self.recent_first = existing.is_some_and(|e| e.recent_first);
        }
        if self.omitted.pin_order {
            self.pin_order = existing.and_then(|e| e.pin_order.clone());
        }
        let stored_last = existing.and_then(|e| e.last_messaged_at);
        if self.omitted.last_messaged_at {
            self.last_messaged_at = stored_last;
        } else if let (Some(new), Some(old)) = (self.last_messaged_at, stored_last) {
            // A device that hadn't seen the newest send never moves it back.
            self.last_messaged_at = Some(new.max(old));
        }
        self.omitted = Omitted::default();
    }
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
                "icon": self.icon,
                "recentFirst": self.recent_first,
                "order": self.order,
                "updatedAt": self.updated_at,
                "deviceId": self.device_id,
            }),
            Kind::Row | Kind::Person | Kind::Channel => json!({
                "kind": self.kind.as_str(),
                "homeId": self.home_id,
                "address": self.address,
                "label": self.label.clone().unwrap_or_default(),
                "workspaceId": self.workspace_id,
                "pinOrder": self.pin_order,
                "lastMessagedAt": self.last_messaged_at,
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
    pub const INVALID_ICON: &str = "invalid_icon";
    pub const INVALID_RECENT_FIRST: &str = "invalid_recent_first";
    pub const INVALID_PIN_ORDER: &str = "invalid_pin_order";
    pub const INVALID_LAST_MESSAGED_AT: &str = "invalid_last_messaged_at";
    /// A pin past [`super::MAX_PINS`] on one Home.
    pub const TOO_MANY_PINS: &str = "too_many_pins";
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

/// Does a normalized address name what its kind says? A person is a box
/// person's external id (`k2:owner`, `k2:connect:<id>`), never an agent's or
/// an App user's; a channel is a Sew id (digits). An agent row is any handle.
pub fn address_fits(kind: Kind, address: &str) -> bool {
    let Some((handle, _)) = address.split_once("::") else { return false };
    match kind {
        Kind::Home => false,
        Kind::Row => true,
        Kind::Person => {
            handle == "k2:owner"
                || handle.strip_prefix("k2:connect:").is_some_and(|id| {
                    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
        }
        Kind::Channel => !handle.is_empty() && handle.len() <= 20 && handle.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// Parse one pushed change into an [`Item`], or the reason it is refused.
pub fn parse_change(v: &Value, device_id: &str, now_ms: i64) -> Result<Item, &'static str> {
    let o = v.as_object().ok_or(reason::NOT_AN_OBJECT)?;
    let kind = o.get("kind").and_then(Value::as_str).and_then(Kind::parse).ok_or(reason::INVALID_KIND)?;
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
            let (icon, icon_omitted) = match o.get("icon") {
                None => (None, true),
                Some(Value::Null) => (None, false),
                Some(Value::String(s)) if valid_icon(s) => (Some(s.clone()), false),
                Some(_) => return Err(reason::INVALID_ICON),
            };
            let (recent_first, recent_first_omitted) = match o.get("recentFirst") {
                None | Some(Value::Null) => (false, true),
                Some(Value::Bool(b)) => (*b, false),
                Some(_) => return Err(reason::INVALID_RECENT_FIRST),
            };
            if o.get("pinOrder").is_some_and(|m| !m.is_null()) {
                return Err(reason::INVALID_PIN_ORDER);
            }
            if o.get("lastMessagedAt").is_some_and(|m| !m.is_null()) {
                return Err(reason::INVALID_LAST_MESSAGED_AT);
            }
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
                icon,
                recent_first,
                pin_order: None,
                last_messaged_at: None,
                omitted: Omitted {
                    icon: icon_omitted,
                    recent_first: recent_first_omitted,
                    pin_order: false,
                    last_messaged_at: false,
                },
            })
        }
        Kind::Row | Kind::Person | Kind::Channel => {
            let home_id =
                o.get("homeId").and_then(Value::as_str).filter(|s| valid_id(s)).ok_or(reason::INVALID_HOME_ID)?;
            let address = o
                .get("address")
                .and_then(Value::as_str)
                .and_then(normalize_address)
                .filter(|a| address_fits(kind, a))
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
            if o.get("icon").is_some_and(|m| !m.is_null()) {
                return Err(reason::INVALID_ICON);
            }
            if o.get("recentFirst").is_some_and(|m| !m.is_null()) {
                return Err(reason::INVALID_RECENT_FIRST);
            }
            let (pin_order, pin_omitted) = match o.get("pinOrder") {
                None => (None, true),
                Some(Value::Null) => (None, false),
                Some(Value::String(s)) if valid_order(s) => (Some(s.clone()), false),
                Some(_) => return Err(reason::INVALID_PIN_ORDER),
            };
            let (last_messaged_at, last_omitted) = match o.get("lastMessagedAt") {
                None => (None, true),
                Some(Value::Null) => (None, false),
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(t) if t > 0 && t <= now_ms + MAX_FUTURE_SKEW_MS => (Some(t), false),
                    _ => return Err(reason::INVALID_LAST_MESSAGED_AT),
                },
                Some(_) => return Err(reason::INVALID_LAST_MESSAGED_AT),
            };
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
                icon: None,
                recent_first: false,
                pin_order,
                last_messaged_at,
                omitted: Omitted {
                    icon: false,
                    recent_first: false,
                    pin_order: pin_omitted,
                    last_messaged_at: last_omitted,
                },
            })
        }
    }
}

// ── Storage ────────────────────────────────────────────────────────────

const ITEM_COLS: &str =
    "kind, home_id, address, name, label, workspace_id, ord, updated_at, device_id, deleted, merged_into, icon, \
     recent_first, pin_order, last_messaged_at";

fn item_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let kind: String = r.get(0)?;
    Ok(Item {
        kind: Kind::parse(&kind).unwrap_or(Kind::Home),
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
        icon: r.get(11)?,
        recent_first: r.get::<_, i64>(12)? != 0,
        pin_order: r.get(13)?,
        last_messaged_at: r.get(14)?,
        omitted: Omitted::default(),
    })
}

fn get_item(conn: &Connection, person: &str, item: &Item) -> rusqlite::Result<Option<Item>> {
    conn.query_row(
        &format!(
            "SELECT {ITEM_COLS} FROM {} WHERE person = ?1 AND kind = ?2 AND home_id = ?3 AND address = ?4",
            item.kind.table()
        ),
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

/// Live pinned rows of every row kind on a Home, but `(except_kind,
/// except_address)` itself (pins are shared by agents, people and channels).
fn pinned_count_except(conn: &Connection, person: &str, home_id: &str, except_kind: Kind, except_address: &str) -> rusqlite::Result<i64> {
    let mut n = 0;
    for table in [HOMES_TABLE, CHAT_TABLE] {
        n += conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {table} WHERE person = ?1 AND kind != 'home' AND home_id = ?2 \
                 AND deleted = 0 AND pin_order IS NOT NULL AND NOT (kind = ?3 AND address = ?4)"
            ),
            params![person, home_id, except_kind.as_str(), except_address],
            |r| r.get::<_, i64>(0),
        )?;
    }
    Ok(n)
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

/// Stamp an active device's `last_sync_at` (Homes and stash sync share the registry).
pub fn touch_device(conn: &Connection, person: &str, device_id: &str, now_ms: i64) -> rusqlite::Result<()> {
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
            "SELECT {ITEM_COLS} FROM {HOMES_TABLE} WHERE person = ?1 AND deleted = 0 \
             UNION ALL SELECT {ITEM_COLS} FROM {CHAT_TABLE} WHERE person = ?1 AND deleted = 0 \
             ORDER BY kind, home_id, ord, address"
        ))?;
        let rows = st.query_map(params![person], item_from_row)?;
        rows.collect::<rusqlite::Result<_>>()?
    } else {
        // `seq` rides as a last column (item_from_row never reads it): a
        // compound select orders by its result columns only.
        let mut st = conn.prepare(&format!(
            "SELECT {ITEM_COLS}, seq FROM {HOMES_TABLE} WHERE person = ?1 AND seq > ?2 \
             UNION ALL SELECT {ITEM_COLS}, seq FROM {CHAT_TABLE} WHERE person = ?1 AND seq > ?2 \
             ORDER BY seq, kind, home_id, address"
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
    // Live first, tombstones after; within the live ones, pins last (so a
    // batch that unpins one row and pins another never trips MAX_PINS);
    // stable within each group.
    parsed.sort_by_key(|i| (i.deleted, i.pin_order.is_some()));

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
        item.keep_omitted(existing.as_ref());
        let newly_pinned = item.kind.is_row()
            && !item.deleted
            && item.pin_order.is_some()
            && !existing.as_ref().is_some_and(|e| !e.deleted && e.pin_order.is_some());
        if newly_pinned && pinned_count_except(&tx, person, &item.home_id, item.kind, &item.address)? >= MAX_PINS as i64 {
            rejected.push(Rejected { item: item.to_json(), reason: reason::TOO_MANY_PINS });
            continue;
        }
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
                if item.kind.is_row() && item.label.as_deref().unwrap_or("").is_empty() {
                    item.label = ex.label.clone();
                }
                if item.order.is_empty() {
                    item.order = ex.order.clone();
                }
            }
        }
        tx.execute(
            &format!(
                "INSERT INTO {} (person, kind, home_id, address, name, label, workspace_id, ord, \
                   updated_at, device_id, deleted, merged_into, seq, icon, recent_first, pin_order, last_messaged_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) \
                 ON CONFLICT (person, kind, home_id, address) DO UPDATE SET \
                   name = excluded.name, label = excluded.label, workspace_id = excluded.workspace_id, \
                   ord = excluded.ord, updated_at = excluded.updated_at, device_id = excluded.device_id, \
                   deleted = excluded.deleted, merged_into = excluded.merged_into, seq = excluded.seq, \
                   icon = excluded.icon, recent_first = excluded.recent_first, pin_order = excluded.pin_order, \
                   last_messaged_at = excluded.last_messaged_at",
                item.kind.table()
            ),
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
                next,
                item.icon,
                item.recent_first as i64,
                item.pin_order,
                item.last_messaged_at
            ],
        )?;
        applied += 1;
    }
    let cursor = if applied > 0 {
        // Collect old tombstones (never redirects) and raise the floor past
        // them, so a device that last pulled before them gets a full set.
        let cutoff = now_ms - TOMBSTONE_KEEP_MS;
        for table in [HOMES_TABLE, CHAT_TABLE] {
            let purged_max: Option<i64> = tx.query_row(
                &format!(
                    "SELECT MAX(seq) FROM {table} WHERE person = ?1 AND deleted = 1 AND merged_into IS NULL AND updated_at < ?2"
                ),
                params![person, cutoff],
                |r| r.get(0),
            )?;
            if let Some(m) = purged_max {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE person = ?1 AND deleted = 1 AND merged_into IS NULL AND updated_at < ?2"),
                    params![person, cutoff],
                )?;
                floor = floor.max(m);
            }
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
        conn.execute_batch(include_str!("../drizzle_sql/0305_home_groups.sql").replace("--> statement-breakpoint", "").as_str())
            .expect("0305 applies");
        conn.execute_batch(include_str!("../drizzle_sql/0306_home_chat_rows.sql").replace("--> statement-breakpoint", "").as_str())
            .expect("0306 applies");
        conn.execute_batch(sql_0307().as_str()).expect("0307 applies");
        assert!(tables_exist(&conn));
        conn
    }

    fn sql_0307() -> String {
        include_str!("../drizzle_sql/0307_home_chat_rows_apart.sql").replace("--> statement-breakpoint", "")
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

    fn icon_of(conn: &Connection, person: &str, id: &str) -> Value {
        live(conn, person)
            .into_iter()
            .find(|v| v["kind"] == "home" && v["id"] == id)
            .unwrap_or_else(|| panic!("no live Home {id}"))["icon"]
            .clone()
    }

    #[test]
    fn a_home_icon_pushes_and_pulls() {
        let c = db();
        let mut h = home("h1", "Work", "V", NOW);
        h["icon"] = json!("briefcase");
        let r = push(&c, "owner", "laptop", &[h], NOW).expect("push");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
        assert_eq!(icon_of(&c, "owner", "h1"), json!("briefcase"));
        // The incremental pull carries it too.
        let p = pull(&c, "owner", Some(0), None, NOW).expect("delta");
        assert_eq!(p.changes[0].to_json()["icon"], json!("briefcase"));
        // A newer edit with another icon wins; JSON null clears it.
        let mut h = home("h1", "Work", "V", NOW + 1);
        h["icon"] = json!("rocket");
        push(&c, "owner", "phone", &[h], NOW).expect("change");
        assert_eq!(icon_of(&c, "owner", "h1"), json!("rocket"));
        let mut h = home("h1", "Work", "V", NOW + 2);
        h["icon"] = Value::Null;
        push(&c, "owner", "phone", &[h], NOW).expect("clear");
        assert_eq!(icon_of(&c, "owner", "h1"), Value::Null, "null clears the icon");
        // A Home that never had one reads as null.
        push(&c, "owner", "phone", &[home("h2", "Other", "k", NOW)], NOW).expect("h2");
        assert_eq!(icon_of(&c, "owner", "h2"), Value::Null);
    }

    #[test]
    fn an_older_client_that_leaves_icon_out_keeps_the_stored_icon() {
        let c = db();
        let mut h = home("h1", "Work", "V", NOW);
        h["icon"] = json!("briefcase");
        push(&c, "owner", "new-client", &[h], NOW).expect("set");
        // An older client renames the Home later: no `icon` key at all.
        let old = home("h1", "Work stuff", "k", NOW + 10);
        assert!(old.get("icon").is_none());
        let r = push(&c, "owner", "old-client", &[old], NOW).expect("old rename");
        assert_eq!(r.applied, 1);
        let l = live(&c, "owner");
        let got = l.iter().find(|v| v["id"] == "h1").expect("h1");
        assert_eq!((got["name"].clone(), got["order"].clone(), got["icon"].clone()), (json!("Work stuff"), json!("k"), json!("briefcase")));
        // Its tombstone keeps the icon for context as well (a later edit brings it back with it).
        push(&c, "owner", "old-client", &[home("h2", "Spare", "s", NOW)], NOW).expect("spare");
        push(&c, "owner", "old-client", &[tomb(home("h1", "Work stuff", "k", NOW + 20))], NOW).expect("tomb");
        let p = pull(&c, "owner", Some(2), None, NOW).expect("delta");
        let t = p.changes.iter().map(Item::to_json).find(|v| v["id"] == "h1").expect("tombstone in delta");
        assert_eq!((t["deleted"].clone(), t["icon"].clone()), (json!(true), json!("briefcase")));
    }

    #[test]
    fn a_bad_icon_is_refused() {
        let c = db();
        let mut cases = Vec::new();
        for bad in [json!("Briefcase"), json!("brief case"), json!(""), json!("a".repeat(ICON_MAX + 1)), json!("emoji-\u{1F600}"), json!(5), json!(true)] {
            let mut h = home("h1", "Work", "V", NOW);
            h["icon"] = bad;
            cases.push(h);
        }
        // An icon on a row is refused too (rows have none).
        let mut r = row("h1", "a::b", "V", NOW);
        r["icon"] = json!("rocket");
        cases.push(r);
        let res = push(&c, "owner", "d", &cases, NOW).expect("push");
        assert_eq!(res.applied, 0);
        assert_eq!(res.rejected.iter().map(|x| x.reason).collect::<Vec<_>>(), vec![reason::INVALID_ICON; 8]);
        // The longest valid name passes.
        let mut ok = home("h1", "Work", "V", NOW);
        ok["icon"] = json!("a".repeat(ICON_MAX));
        assert!(push(&c, "owner", "d", &[ok], NOW).expect("ok").rejected.is_empty());
        assert!(valid_icon("gamepad-2") && valid_icon("house") && !valid_icon("House"));
    }

    fn row_of(conn: &Connection, person: &str, home: &str, addr: &str) -> Value {
        live(conn, person)
            .into_iter()
            .find(|v| v["kind"] == "row" && v["homeId"] == home && v["address"] == addr)
            .unwrap_or_else(|| panic!("no live row {home} {addr}"))
    }

    fn home_of(conn: &Connection, person: &str, id: &str) -> Value {
        live(conn, person)
            .into_iter()
            .find(|v| v["kind"] == "home" && v["id"] == id)
            .unwrap_or_else(|| panic!("no live Home {id}"))
    }

    #[test]
    fn pins_recent_first_and_last_send_push_and_pull() {
        let c = db();
        let mut h = home("h1", "Work", "V", NOW);
        h["recentFirst"] = json!(true);
        let mut r = row("h1", "a::s", "V", NOW);
        r["pinOrder"] = json!("V");
        r["lastMessagedAt"] = json!(NOW - 5);
        let res = push(&c, "owner", "laptop", &[h, r, row("h1", "b::s", "k", NOW)], NOW).expect("push");
        assert!(res.rejected.is_empty(), "{:?}", res.rejected);
        assert_eq!(home_of(&c, "owner", "h1")["recentFirst"], json!(true));
        let a = row_of(&c, "owner", "h1", "a::s");
        assert_eq!((a["pinOrder"].clone(), a["lastMessagedAt"].clone()), (json!("V"), json!(NOW - 5)));
        let b = row_of(&c, "owner", "h1", "b::s");
        assert_eq!((b["pinOrder"].clone(), b["lastMessagedAt"].clone()), (Value::Null, Value::Null));
        // The incremental pull carries them.
        let p = pull(&c, "owner", Some(0), None, NOW).expect("delta");
        let j: Vec<Value> = p.changes.iter().map(Item::to_json).collect();
        assert!(j.iter().any(|v| v["id"] == "h1" && v["recentFirst"] == true), "{j:?}");
        assert!(j.iter().any(|v| v["address"] == "a::s" && v["pinOrder"] == "V" && v["lastMessagedAt"] == json!(NOW - 5)), "{j:?}");
        // Unpin with null; switch recent-first off with false.
        let mut r = row("h1", "a::s", "V", NOW + 1);
        r["pinOrder"] = Value::Null;
        let mut h = home("h1", "Work", "V", NOW + 1);
        h["recentFirst"] = json!(false);
        push(&c, "owner", "phone", &[r, h], NOW).expect("unpin");
        assert_eq!(row_of(&c, "owner", "h1", "a::s")["pinOrder"], Value::Null);
        assert_eq!(row_of(&c, "owner", "h1", "a::s")["lastMessagedAt"], json!(NOW - 5), "left out: kept");
        assert_eq!(home_of(&c, "owner", "h1")["recentFirst"], json!(false));
    }

    #[test]
    fn an_older_client_keeps_pins_recent_first_and_last_send() {
        let c = db();
        let mut h = home("h1", "Work", "V", NOW);
        h["recentFirst"] = json!(true);
        h["icon"] = json!("briefcase");
        let mut r = row("h1", "a::s", "V", NOW);
        r["pinOrder"] = json!("V");
        r["lastMessagedAt"] = json!(NOW - 5);
        push(&c, "owner", "new-client", &[h, r], NOW).expect("set");
        // An older client renames the Home and moves the row: none of the new keys.
        let old_home = home("h1", "Work stuff", "V", NOW + 10);
        let old_row = row("h1", "a::s", "k", NOW + 10);
        for k in ["icon", "recentFirst", "pinOrder", "lastMessagedAt"] {
            assert!(old_home.get(k).is_none() && old_row.get(k).is_none());
        }
        let res = push(&c, "owner", "old-client", &[old_home, old_row], NOW).expect("old");
        assert_eq!(res.applied, 2);
        let h = home_of(&c, "owner", "h1");
        assert_eq!((h["name"].clone(), h["recentFirst"].clone(), h["icon"].clone()), (json!("Work stuff"), json!(true), json!("briefcase")));
        let a = row_of(&c, "owner", "h1", "a::s");
        assert_eq!((a["order"].clone(), a["pinOrder"].clone(), a["lastMessagedAt"].clone()), (json!("k"), json!("V"), json!(NOW - 5)));
        // A device that hadn't seen the newest send never moves it back.
        let mut stale = row("h1", "a::s", "k", NOW + 20);
        stale["lastMessagedAt"] = json!(NOW - 100);
        push(&c, "owner", "stale", &[stale], NOW).expect("stale");
        assert_eq!(row_of(&c, "owner", "h1", "a::s")["lastMessagedAt"], json!(NOW - 5));
        let mut newer = row("h1", "a::s", "k", NOW + 30);
        newer["lastMessagedAt"] = json!(NOW + 25);
        push(&c, "owner", "phone", &[newer], NOW).expect("newer");
        assert_eq!(row_of(&c, "owner", "h1", "a::s")["lastMessagedAt"], json!(NOW + 25));
    }

    #[test]
    fn a_home_holds_at_most_nine_pins() {
        let c = db();
        push(&c, "owner", "d", &[home("h1", "Work", "V", NOW)], NOW).expect("home");
        let pinned: Vec<Value> = (1..=MAX_PINS)
            .map(|i| {
                let mut r = row("h1", &format!("a{i}::s"), "V", NOW);
                r["pinOrder"] = json!(format!("{i}"));
                r
            })
            .collect();
        assert!(push(&c, "owner", "d", &pinned, NOW).expect("nine").rejected.is_empty());
        let mut tenth = row("h1", "a10::s", "V", NOW);
        tenth["pinOrder"] = json!("a");
        let res = push(&c, "owner", "d", &[tenth.clone()], NOW).expect("tenth");
        assert_eq!(res.rejected.iter().map(|x| x.reason).collect::<Vec<_>>(), vec![reason::TOO_MANY_PINS]);
        // Re-pinning (moving) a row that is already pinned is fine.
        let mut moved = row("h1", "a3::s", "V", NOW + 1);
        moved["pinOrder"] = json!("z");
        assert!(push(&c, "owner", "d", &[moved], NOW).expect("move").rejected.is_empty());
        // Unpin one and pin the tenth in one batch, in either order: it fits.
        let mut un = row("h1", "a1::s", "V", NOW + 2);
        un["pinOrder"] = Value::Null;
        tenth["updatedAt"] = json!(NOW + 2);
        let res = push(&c, "owner", "d", &[tenth, un], NOW).expect("swap");
        assert!(res.rejected.is_empty(), "{:?}", res.rejected);
        // Another Home has its own nine.
        let mut other = row("h2", "a1::s", "V", NOW);
        other["pinOrder"] = json!("V");
        assert!(push(&c, "owner", "d", &[home("h2", "Play", "k", NOW), other], NOW).expect("h2").rejected.is_empty());
    }

    #[test]
    fn bad_pins_recent_first_and_last_send_are_refused() {
        let c = db();
        let set = |mut v: Value, k: &str, x: Value| {
            v[k] = x;
            v
        };
        let cases = vec![
            (set(home("h", "x", "V", NOW), "recentFirst", json!("yes")), reason::INVALID_RECENT_FIRST),
            (set(row("h", "a::b", "V", NOW), "recentFirst", json!(true)), reason::INVALID_RECENT_FIRST),
            (set(row("h", "a::b", "V", NOW), "pinOrder", json!("V0")), reason::INVALID_PIN_ORDER),
            (set(row("h", "a::b", "V", NOW), "pinOrder", json!(3)), reason::INVALID_PIN_ORDER),
            (set(home("h", "x", "V", NOW), "pinOrder", json!("V")), reason::INVALID_PIN_ORDER),
            (set(row("h", "a::b", "V", NOW), "lastMessagedAt", json!(-1)), reason::INVALID_LAST_MESSAGED_AT),
            (set(row("h", "a::b", "V", NOW), "lastMessagedAt", json!("today")), reason::INVALID_LAST_MESSAGED_AT),
            (set(row("h", "a::b", "V", NOW), "lastMessagedAt", json!(NOW + MAX_FUTURE_SKEW_MS + 1)), reason::INVALID_LAST_MESSAGED_AT),
            (set(home("h", "x", "V", NOW), "lastMessagedAt", json!(NOW)), reason::INVALID_LAST_MESSAGED_AT),
        ];
        let input: Vec<Value> = cases.iter().map(|(v, _)| v.clone()).collect();
        let r = push(&c, "owner", "d", &input, NOW).expect("push");
        assert_eq!(r.applied, 0);
        let got: Vec<&str> = r.rejected.iter().map(|x| x.reason).collect();
        let want: Vec<&str> = cases.iter().map(|(_, w)| *w).collect();
        assert_eq!(got, want);
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

    fn chat_row(kind: &str, home: &str, addr: &str, order: &str, at: i64) -> Value {
        json!({"kind": kind, "homeId": home, "address": addr, "label": "Alex", "workspaceId": null, "order": order, "updatedAt": at})
    }

    #[test]
    fn person_and_channel_rows_round_trip_beside_agent_rows() {
        let c = db();
        push(&c, "owner", "laptop", &[home("h1", "Work", "V", NOW)], NOW).expect("home");
        let mut p = chat_row("person", "h1", "K2:Connect:3f2a-9b::abcdef0123456789", "V", NOW);
        p["pinOrder"] = json!("V");
        p["lastMessagedAt"] = json!(NOW - 10);
        let ch = chat_row("channel", "h1", "184467440737::abcdef0123456789", "l", NOW);
        // An agent row whose handle happens to be the channel's id is its own item.
        let agent = row("h1", "184467440737::abcdef0123456789", "t", NOW);
        let r = push(&c, "owner", "laptop", &[p, ch, agent], NOW).expect("push");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
        assert_eq!(r.applied, 3);
        let l = live(&c, "owner");
        let person = l.iter().find(|v| v["kind"] == "person").expect("person row");
        assert_eq!(person["address"], json!("k2:connect:3f2a-9b::abcdef0123456789"), "lowercased like agent rows");
        assert_eq!((person["pinOrder"].clone(), person["lastMessagedAt"].clone()), (json!("V"), json!(NOW - 10)));
        assert_eq!(l.iter().filter(|v| v["kind"] == "channel").count(), 1);
        assert_eq!(l.iter().filter(|v| v["kind"] == "row").count(), 1, "the agent row with the same address stays apart");
        // A tombstone for the channel leaves the agent row.
        push(&c, "owner", "phone", &[tomb(chat_row("channel", "h1", "184467440737::abcdef0123456789", "l", NOW + 5))], NOW)
            .expect("tomb");
        let l = live(&c, "owner");
        assert!(l.iter().all(|v| v["kind"] != "channel"));
        assert_eq!(l.iter().filter(|v| v["kind"] == "row").count(), 1);
        // An incremental pull carries the tombstone with its kind.
        let d = pull(&c, "owner", Some(1), None, NOW).expect("delta");
        let t = d.changes.iter().map(Item::to_json).find(|v| v["deleted"] == json!(true)).expect("tombstone");
        assert_eq!(t["kind"], json!("channel"));
        assert_eq!(KINDS, &["home", "row", "person", "channel"]);
    }

    #[test]
    fn a_person_is_a_box_person_and_a_channel_a_sew_id() {
        let c = db();
        let bad = [
            chat_row("person", "h1", "k2:workspace:5d1c::s", "V", NOW),
            chat_row("person", "h1", "k2:principal:app-1::s", "V", NOW),
            chat_row("person", "h1", "alex::s", "V", NOW),
            chat_row("person", "h1", "k2:connect:::s", "V", NOW),
            chat_row("channel", "h1", "general::s", "V", NOW),
            chat_row("channel", "h1", "123456789012345678901::s", "V", NOW),
        ];
        let r = push(&c, "owner", "d", &bad, NOW).expect("push");
        assert_eq!(r.applied, 0);
        assert_eq!(r.rejected.iter().map(|x| x.reason).collect::<Vec<_>>(), vec![reason::INVALID_ADDRESS; 6]);
        let good = [
            chat_row("person", "h1", "k2:owner::s", "V", NOW),
            chat_row("person", "h1", "k2:connect:0b6f6b8e-1d2c-4e1a-9f00-1234567890ab::s", "l", NOW),
            chat_row("channel", "h1", "42::s", "t", NOW),
        ];
        let r = push(&c, "owner", "d", &good, NOW).expect("push");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
        let r = push(&c, "owner", "d", &[json!({"kind": "group", "id": "h1", "name": "x", "order": "V", "updatedAt": NOW})], NOW)
            .expect("push");
        assert_eq!(r.rejected[0].reason, reason::INVALID_KIND);
    }

    #[test]
    fn pins_are_shared_by_agents_people_and_channels() {
        let c = db();
        push(&c, "owner", "d", &[home("h1", "Work", "V", NOW)], NOW).expect("home");
        let mut items = Vec::new();
        for i in 1..=MAX_PINS {
            let kind = ["row", "person", "channel"][i % 3];
            let addr = match kind {
                "row" => format!("a{i}::s"),
                "person" => format!("k2:connect:p{i}::s"),
                _ => format!("{i}::s"),
            };
            let mut r = chat_row(kind, "h1", &addr, "V", NOW);
            r["pinOrder"] = json!(format!("{i}"));
            items.push(r);
        }
        assert!(push(&c, "owner", "d", &items, NOW).expect("nine").rejected.is_empty());
        let mut tenth = chat_row("person", "h1", "k2:owner::s", "V", NOW);
        tenth["pinOrder"] = json!("a");
        let r = push(&c, "owner", "d", &[tenth], NOW).expect("tenth");
        assert_eq!(r.rejected.iter().map(|x| x.reason).collect::<Vec<_>>(), vec![reason::TOO_MANY_PINS]);
        // Moving a pinned channel among the pins is not a new pin.
        let mut moved = chat_row("channel", "h1", "2::s", "V", NOW + 1);
        moved["pinOrder"] = json!("z");
        assert!(push(&c, "owner", "d", &[moved], NOW).expect("move").rejected.is_empty());
        // An agent row on pinned channel 5's address is another item: a tenth pin.
        let mut twin = chat_row("row", "h1", "5::s", "V", NOW);
        twin["pinOrder"] = json!("b");
        let r = push(&c, "owner", "d", &[twin], NOW).expect("twin");
        assert_eq!(r.rejected.iter().map(|x| x.reason).collect::<Vec<_>>(), vec![reason::TOO_MANY_PINS]);
    }

    #[test]
    fn rows_written_before_0306_keep_every_field_through_the_rebuild() {
        let conn = Connection::open_in_memory().expect("mem db");
        conn.execute_batch(include_str!("../drizzle_sql/0193_homes_sync.sql").replace("--> statement-breakpoint", "").as_str())
            .expect("0193");
        conn.execute_batch(include_str!("../drizzle_sql/0305_home_groups.sql").replace("--> statement-breakpoint", "").as_str())
            .expect("0305");
        // This build's push and pull also read 0307's table: it is made
        // first (on this older shape it moves nothing).
        conn.execute_batch(sql_0307().as_str()).expect("0307's table");
        let mut r = row("h1", "atlas::s", "V", NOW);
        r["pinOrder"] = json!("V");
        r["lastMessagedAt"] = json!(NOW - 1);
        let mut h = home("h1", "Work", "V", NOW);
        h["icon"] = json!("rocket");
        h["recentFirst"] = json!(true);
        push(&conn, "owner", "d", &[h, r], NOW).expect("old push");
        // The old CHECK refuses the new kinds outright.
        assert!(conn
            .execute(
                "INSERT INTO homes_items (person, kind, home_id, address, ord, updated_at, device_id, seq) \
                 VALUES ('owner', 'person', 'h1', 'k2:owner::s', 'V', 1, 'd', 9)",
                [],
            )
            .is_err());
        let before = pull(&conn, "owner", None, None, NOW).expect("pull").changes;
        conn.execute_batch(include_str!("../drizzle_sql/0306_home_chat_rows.sql").replace("--> statement-breakpoint", "").as_str())
            .expect("0306");
        let after = pull(&conn, "owner", None, None, NOW).expect("pull").changes;
        assert_eq!(before.len(), 2);
        assert_eq!(before, after, "every item and field survives the rebuild");
        let idx: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'homes_items_person_seq'", [], |r| {
                r.get(0)
            })
            .expect("index");
        assert_eq!(idx, 1, "the cursor index is made again");
        let r = push(&conn, "owner", "d", &[chat_row("person", "h1", "k2:owner::s", "l", NOW)], NOW).expect("new");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
    }

    /// v0.46.12's `pull`, as that release runs it against this database
    /// (`git show v0.46.12:crates/k2-core/src/homes_sync.rs`): its columns,
    /// its two queries word for word, and its kind mapping, where any kind
    /// but `row` is a Home (`kind: if kind == "row" { Row } else { Home }`)
    /// and a Home's wire form is `{kind:"home", id, name, order, …}`.
    fn v0_46_12_pull(conn: &Connection, person: &str, since: Option<i64>) -> Vec<Value> {
        const COLS: &str = "kind, home_id, address, name, label, workspace_id, ord, updated_at, device_id, deleted, merged_into";
        let sql = match since {
            None => format!("SELECT {COLS} FROM homes_items WHERE person = ?1 AND deleted = 0 ORDER BY kind, home_id, ord, address"),
            Some(_) => format!("SELECT {COLS} FROM homes_items WHERE person = ?1 AND seq > ?2 ORDER BY seq, kind, home_id, address"),
        };
        let mut st = conn.prepare(&sql).expect("0.46.12's query runs on this schema");
        let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Value> {
            let kind: String = r.get(0)?;
            let (home_id, address): (String, String) = (r.get(1)?, r.get(2)?);
            let (name, label): (Option<String>, Option<String>) = (r.get(3)?, r.get(4)?);
            let (order, updated_at, device_id): (String, i64, String) = (r.get(6)?, r.get(7)?, r.get(8)?);
            let mut v = if kind == "row" {
                json!({"kind": "row", "homeId": home_id, "address": address, "label": label.unwrap_or_default(),
                       "workspaceId": r.get::<_, Option<String>>(5)?, "order": order, "updatedAt": updated_at, "deviceId": device_id})
            } else {
                json!({"kind": "home", "id": home_id, "name": name, "order": order, "updatedAt": updated_at, "deviceId": device_id})
            };
            if r.get::<_, i64>(9)? != 0 {
                v["deleted"] = json!(true);
            }
            if let Some(m) = r.get::<_, Option<String>>(10)? {
                v["mergedInto"] = json!(m);
            }
            Ok(v)
        };
        let rows = match since {
            None => st.query_map(params![person], map),
            Some(s) => st.query_map(params![person, s], map),
        }
        .expect("0.46.12's query");
        rows.collect::<rusqlite::Result<Vec<_>>>().expect("0.46.12's rows")
    }

    /// A 0.46.13 server rolled back to 0.46.12 (the database stays): 0.46.12's
    /// own read gets the groups exactly as before people and channels were
    /// added to them: no extra Home, no lost name, no moved group.
    #[test]
    fn a_server_rolled_back_to_0_46_12_reads_its_groups_unchanged() {
        let c = db();
        let groups = [home("h1", "Work", "V", NOW), home("h2", "Family", "l", NOW), row("h1", "atlas::s", "V", NOW)];
        assert!(push(&c, "owner", "laptop", &groups, NOW).expect("groups").rejected.is_empty());
        let before_full = v0_46_12_pull(&c, "owner", None);
        let before_delta = v0_46_12_pull(&c, "owner", Some(0));
        assert_eq!(before_full.iter().filter(|v| v["kind"] == "home").count(), 2);

        // 0.46.13 puts people and channels in both groups (one pinned, one
        // ordered first, one deleted again).
        let mut p = chat_row("person", "h1", "k2:owner::s", "0", NOW + 1);
        p["pinOrder"] = json!("V");
        let rows = [p, chat_row("channel", "h1", "42::s", "1", NOW + 1), chat_row("person", "h2", "k2:connect:ab12::s", "z", NOW + 1)];
        assert!(push(&c, "owner", "phone", &rows, NOW).expect("chat rows").rejected.is_empty());
        let gone = tomb(chat_row("channel", "h2", "43::s", "2", NOW + 2));
        assert!(push(&c, "owner", "phone", &[chat_row("channel", "h2", "43::s", "2", NOW + 1)], NOW).expect("ch").rejected.is_empty());
        assert!(push(&c, "owner", "phone", &[gone], NOW).expect("tomb").rejected.is_empty());

        assert_eq!(v0_46_12_pull(&c, "owner", None), before_full, "0.46.12's full read: the groups unchanged");
        assert_eq!(v0_46_12_pull(&c, "owner", Some(0)), before_delta, "0.46.12's delta read: the groups unchanged");
        let cursor = pull(&c, "owner", None, None, NOW).expect("pull").cursor;
        assert!(v0_46_12_pull(&c, "owner", Some(1)).is_empty(), "nothing after the groups' own write reaches 0.46.12");
        assert!(cursor > 1);

        // 0.46.13 itself still reads everything, both ways.
        let l = live(&c, "owner");
        assert_eq!(l.iter().filter(|v| v["kind"] == "home").count(), 2);
        assert_eq!(l.iter().filter(|v| v["kind"] == "person").count(), 2);
        assert_eq!(l.iter().filter(|v| v["kind"] == "channel").count(), 1);
        assert_eq!(l.iter().filter(|v| v["kind"] == "row").count(), 1);
        let d: Vec<Value> = pull(&c, "owner", Some(1), None, NOW).expect("delta").changes.iter().map(Item::to_json).collect();
        assert_eq!(
            d.iter().map(|v| (v["kind"].as_str().unwrap_or_default().to_string(), v["deleted"] == json!(true))).collect::<Vec<_>>(),
            vec![
                ("channel".to_string(), false),
                ("person".to_string(), false),
                ("person".to_string(), false),
                ("channel".to_string(), true)
            ],
            "the delta in seq order across both tables: {d:?}"
        );
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM homes_items WHERE kind NOT IN ('home', 'row')", [], |r| r.get::<_, i64>(0)).expect("count"),
            0,
            "no person or channel row in the table 0.46.12 reads"
        );
    }

    /// 0307 moves the person and channel rows a queue build wrote into
    /// `homes_items` (0306) out of 0.46.12's sight, every field kept: before
    /// it, 0.46.12's read shows them as nameless Homes; after it, the groups
    /// read as they were, and this build pulls the very same items.
    #[test]
    fn rows_a_queue_build_wrote_move_out_of_0_46_12s_sight_at_0307() {
        let conn = Connection::open_in_memory().expect("mem db");
        for (n, sql) in [
            ("0193", include_str!("../drizzle_sql/0193_homes_sync.sql")),
            ("0305", include_str!("../drizzle_sql/0305_home_groups.sql")),
            ("0306", include_str!("../drizzle_sql/0306_home_chat_rows.sql")),
        ] {
            conn.execute_batch(sql.replace("--> statement-breakpoint", "").as_str()).unwrap_or_else(|e| panic!("{n}: {e}"));
        }
        // What a queue build (0306 only) stored: Homes, an agent row, and
        // people and channels in the same table.
        let ins = |kind: &str, home: &str, addr: &str, name: Option<&str>, label: Option<&str>, ord: &str, deleted: i64, seq: i64, pin: Option<&str>| {
            conn.execute(
                "INSERT INTO homes_items (person, kind, home_id, address, name, label, ord, updated_at, device_id, deleted, seq, \
                 pin_order, last_messaged_at, icon, recent_first) VALUES ('owner', ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'phone', ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    kind,
                    home,
                    addr,
                    name,
                    label,
                    ord,
                    NOW + seq,
                    deleted,
                    seq,
                    pin,
                    (kind != "home").then_some(NOW - seq),
                    (kind == "home").then_some("rocket"),
                    (kind == "home") as i64
                ],
            )
            .expect("queue build row");
        };
        ins("home", "h1", "", Some("Work"), None, "V", 0, 1, None);
        ins("home", "h2", "", Some("Family"), None, "l", 0, 1, None);
        ins("row", "h1", "atlas::s", None, Some("Atlas"), "V", 0, 1, Some("V"));
        ins("person", "h1", "k2:owner::s", None, Some("Owner"), "0", 0, 2, Some("a"));
        ins("channel", "h1", "42::s", None, Some("general"), "1", 0, 2, None);
        ins("channel", "h2", "43::s", None, Some("old"), "2", 1, 3, None);
        conn.execute("INSERT INTO homes_people (person, seq, floor) VALUES ('owner', 3, 0)", []).expect("cursor");
        let damaged = v0_46_12_pull(&conn, "owner", None);
        assert_eq!(damaged.iter().filter(|v| v["kind"] == "home").count(), 4, "the defect: 0.46.12 reads people and channels as Homes");
        let raw = |conn: &Connection, table: &str| -> Vec<Item> {
            let mut st = conn.prepare(&format!("SELECT {ITEM_COLS} FROM {table} ORDER BY kind, home_id, address")).expect("raw");
            let rows = st.query_map([], item_from_row).expect("raw rows");
            rows.collect::<rusqlite::Result<_>>().expect("raw items")
        };
        let mut before = raw(&conn, "homes_items");
        let seqs_before: Vec<(String, i64)> = conn
            .prepare("SELECT home_id || '/' || address, seq FROM homes_items ORDER BY kind, home_id, address")
            .expect("seqs")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("seqs")
            .collect::<rusqlite::Result<_>>()
            .expect("seqs");

        conn.execute_batch(sql_0307().as_str()).expect("0307");
        assert!(tables_exist(&conn));
        let groups = v0_46_12_pull(&conn, "owner", None);
        assert_eq!(groups.len(), 3, "{groups:?}");
        assert_eq!(
            groups.iter().filter(|v| v["kind"] == "home").map(|v| (v["id"].clone(), v["name"].clone(), v["order"].clone())).collect::<Vec<_>>(),
            vec![(json!("h1"), json!("Work"), json!("V")), (json!("h2"), json!("Family"), json!("l"))],
            "0.46.12 reads its groups with their names and order"
        );
        let mut after = raw(&conn, "homes_items");
        after.extend(raw(&conn, "homes_chat_items"));
        let key = |i: &Item| (i.kind.as_str(), i.home_id.clone(), i.address.clone());
        before.sort_by_key(key);
        after.sort_by_key(key);
        assert_eq!(before, after, "every item and field kept through the move");
        let mut seqs_after: Vec<(String, i64)> = Vec::new();
        for t in ["homes_items", "homes_chat_items"] {
            let mut st = conn.prepare(&format!("SELECT home_id || '/' || address, seq FROM {t}")).expect("seqs");
            seqs_after.extend(st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).expect("seqs").collect::<rusqlite::Result<Vec<_>>>().expect("seqs"));
        }
        let (mut a, mut b) = (seqs_before, seqs_after);
        a.sort();
        b.sort();
        assert_eq!(a, b, "seqs kept, so cursors still work");
        // This build's pull serves the moved rows (the tombstone only in a delta).
        let l = live(&conn, "owner");
        assert_eq!(l.len(), 5, "{l:?}");
        let person = l.iter().find(|v| v["kind"] == "person").expect("the person row");
        assert_eq!((person["pinOrder"].clone(), person["label"].clone()), (json!("a"), json!("Owner")));
        let d = pull(&conn, "owner", Some(2), None, NOW).expect("delta").changes;
        assert_eq!(d.iter().map(|i| (i.kind, i.deleted)).collect::<Vec<_>>(), vec![(Kind::Channel, true)]);
        // A second run moves nothing and loses nothing.
        conn.execute_batch(sql_0307().as_str()).expect("0307 again");
        assert_eq!(live(&conn, "owner"), l);
    }
}
