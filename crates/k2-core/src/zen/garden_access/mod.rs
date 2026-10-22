//! Garden access: one list per Garden of the agents who may edit it, and
//! each Garden's History (prd-garden-access-v1 rev 4, slice A1; migration
//! 0219).
//!
//! This module is the **store and the switch** the other slices build on
//! (A2 routes and resolver, A3 data doors, A4 cards, A5 Settings, A6 copy).
//! It owns no HTTP and no wording.
//!
//! - **The switch (G0)**, [`enabled`]: the server setting `gardenAccess`
//!   (`AppSettings::garden_access`), owner only, **default off in every
//!   build**. The daemon syncs it with [`set_switch`] at boot and on every
//!   settings write, and runs the G4 pass ([`pass::run`]) on each off→on
//!   flip. **While it is off nothing may write these tables** (G0a: they
//!   exist and stay empty); every caller outside the pass checks
//!   [`enabled`] first. The store functions don't, so tests and the pass can
//!   use them.
//! - **The list (G1)**, `zen_garden_access`: `(garden, workspace)` rows.
//!   [`add`] and [`remove`] are the only writers; each change also writes an
//!   `access_added` / `access_removed` event (who, how, which card or
//!   route) and one Access Audit line.
//! - **History (G1, rev 5)**, `zen_garden_events`: [`record_event`] (about
//!   a Garden, a widget, a placement or the computer-wide look: [`On`]),
//!   [`events_for_garden`] and [`events_for_widget`]. Every Garden change
//!   the other slices make goes here ([`EventKind`]); it survives the
//!   Garden's and the workspace's deletion and is kept two years ([`EVENTS_RETAIN_SECS`]; the daemon's daily sweep
//!   calls [`prune_events`]), with no count cap.
//! - **Made (G1)**, `zen_garden_made`: which agent made a Garden (the tag,
//!   the first card's words, the 5-a-day budget [`made_since`]). Grants
//!   nothing.
//! - **Asks (G1)**, `zen_garden_access_asks`, per (Garden, agent,
//!   [`AskKind`]): the last ask, the last "Not now" and the waiting card,
//!   for K2's own `garden.access` and `garden.delete` cards ([`may_ask`]).
//!   It is a store only: how a card renders (the standard permission view)
//!   is A4's.
//! - **Workspace delete (G7)**: [`delete_for_workspace`], called by
//!   `projects_ops::projects_delete`. Events stay.
//! - **Garden delete and restore (G8)**: [`archive_garden`] mirrors the
//!   list and made rows to `.history/gardens/<id>.toml/access.json` and
//!   removes them; [`restore_garden`] puts them back. Events stay.
//!
//! **No sight here.** Rev 4 (Rosson Q1): a Garden's widgets see what the
//! person sees. Access is about *editing*; any agent can still be shown in,
//! or talked to through, a Garden's widgets without being on its list.
//!
//! **Locking.** Every function takes a `&Connection`: callers hold the
//! shared DB lock (`crate::db::shared()`), as `garden_data` does. Times are
//! Unix seconds.
//!
//! Honest bar (as `garden_data`): an agent runs as the same OS user and can
//! open `k2so.db` itself. The list is a guardrail against mistakes and
//! injected prompts, not a wall.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::store::{valid_garden_id, ZenFile, ZenFiles};

pub mod pass;

#[cfg(test)]
mod tests;

/// The server setting's wire name (`AppSettings::garden_access`).
pub const SETTING_KEY: &str = "gardenAccess";

/// The Access Audit event for a list change (`~/.k2/auth-audit.jsonl`).
pub const AUDIT_EVENT: &str = "zen-garden-access";

/// The Access Audit event for a switch flip.
pub const AUDIT_SWITCH_EVENT: &str = "zen-garden-access-switch";

/// The Access Audit event the G4 pass writes once per run.
pub const AUDIT_PASS_EVENT: &str = "zen-garden-access-pass";

/// The file a deleted Garden's rows are kept in (G8), next to the
/// `sync.json` mirror in `.history/gardens/<id>.toml/`.
pub const MIRROR_FILE: &str = "access.json";

/// No new card for a (Garden, agent, kind) within this long of a "Not now".
pub const NOT_NOW_QUIET_SECS: i64 = 24 * 60 * 60;

/// New Gardens an agent may make per day (§1.3; A2 enforces it).
pub const MADE_PER_DAY: i64 = 5;

/// History is kept this long (Rosson 2026-10-22: two years). The daemon's
/// daily sweep calls [`prune_events_older_than`] with `now - EVENTS_RETAIN_SECS`.
pub const EVENTS_RETAIN_SECS: i64 = 2 * 365 * 24 * 60 * 60;

/// The largest `detail_json` (the column's CHECK).
pub const MAX_DETAIL_BYTES: usize = 2048;

// ── the switch (G0) ──────────────────────────────────────────────────

static SWITCH: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `gardenAccess` setting into this process. Returns the
/// previous value. The daemon's `settings_routes::sync_garden_access` is the
/// one caller outside tests (boot, settings update and reset); it runs the
/// G4 pass on an off→on flip.
pub fn set_switch(on: bool) -> bool {
    SWITCH.swap(on, Ordering::SeqCst)
}

/// Garden access lists on? **Default off.** There is no env force-on: the
/// switch is the person's, per server.
pub fn enabled() -> bool {
    SWITCH.load(Ordering::SeqCst)
}

// ── errors ───────────────────────────────────────────────────────────

/// What a store call can fail with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessError {
    /// A bad Garden id, an unknown workspace, an empty actor, a detail over
    /// [`MAX_DETAIL_BYTES`] (HTTP 400).
    BadRequest(String),
    /// The database or a history file (HTTP 500).
    Store(String),
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccessError::BadRequest(m) | AccessError::Store(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for AccessError {}

impl From<rusqlite::Error> for AccessError {
    fn from(e: rusqlite::Error) -> Self {
        AccessError::Store(format!("garden access store: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, AccessError>;

// ── who ──────────────────────────────────────────────────────────────

/// Who did something, as History's `actor` column spells it (rev 4 G1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The owner token.
    Owner,
    /// A Connect login (`login:<user>`).
    Login(String),
    /// An agent passport (`agent:<project_id>`).
    Agent(String),
    /// A plain shell tab in a room: the person typing (`shell:<project_id>`).
    Shell(String),
    /// K2 itself (cleanup on a workspace delete, a lock put-back, …).
    K2,
    /// A hand edit on disk the watcher saw.
    Disk,
    /// The G4 pass.
    Migration,
}

impl Actor {
    pub fn label(&self) -> String {
        match self {
            Actor::Owner => "owner".into(),
            Actor::Login(u) => format!("login:{u}"),
            Actor::Agent(p) => format!("agent:{p}"),
            Actor::Shell(p) => format!("shell:{p}"),
            Actor::K2 => "k2".into(),
            Actor::Disk => "disk".into(),
            Actor::Migration => "migration".into(),
        }
    }

    /// Parse a stored label back (`None` for anything else).
    pub fn parse(label: &str) -> Option<Self> {
        match label {
            "owner" => return Some(Actor::Owner),
            "k2" => return Some(Actor::K2),
            "disk" => return Some(Actor::Disk),
            "migration" => return Some(Actor::Migration),
            _ => {}
        }
        let (kind, rest) = label.split_once(':')?;
        if rest.is_empty() {
            return None;
        }
        match kind {
            "login" => Some(Actor::Login(rest.into())),
            "agent" => Some(Actor::Agent(rest.into())),
            "shell" => Some(Actor::Shell(rest.into())),
            _ => None,
        }
    }
}

// ── History ──────────────────────────────────────────────────────────

/// What a History line records (the `kind` column's CHECK list, rev 4 G1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Made,
    Changed,
    WidgetChanged,
    Renamed,
    Reordered,
    Theme,
    Reset,
    Template,
    Locked,
    Unlocked,
    Sync,
    AccessAdded,
    AccessRemoved,
    AccessAsked,
    AccessNotNow,
    DeleteAsked,
    DeleteKept,
    Deleted,
    Restored,
    DiskEdit,
    LockPutBack,
    Signed,
    Imported,
}

impl EventKind {
    pub const ALL: [EventKind; 23] = [
        EventKind::Made,
        EventKind::Changed,
        EventKind::WidgetChanged,
        EventKind::Renamed,
        EventKind::Reordered,
        EventKind::Theme,
        EventKind::Reset,
        EventKind::Template,
        EventKind::Locked,
        EventKind::Unlocked,
        EventKind::Sync,
        EventKind::AccessAdded,
        EventKind::AccessRemoved,
        EventKind::AccessAsked,
        EventKind::AccessNotNow,
        EventKind::DeleteAsked,
        EventKind::DeleteKept,
        EventKind::Deleted,
        EventKind::Restored,
        EventKind::DiskEdit,
        EventKind::LockPutBack,
        EventKind::Signed,
        EventKind::Imported,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Made => "made",
            EventKind::Changed => "changed",
            EventKind::WidgetChanged => "widget_changed",
            EventKind::Renamed => "renamed",
            EventKind::Reordered => "reordered",
            EventKind::Theme => "theme",
            EventKind::Reset => "reset",
            EventKind::Template => "template",
            EventKind::Locked => "locked",
            EventKind::Unlocked => "unlocked",
            EventKind::Sync => "sync",
            EventKind::AccessAdded => "access_added",
            EventKind::AccessRemoved => "access_removed",
            EventKind::AccessAsked => "access_asked",
            EventKind::AccessNotNow => "access_not_now",
            EventKind::DeleteAsked => "delete_asked",
            EventKind::DeleteKept => "delete_kept",
            EventKind::Deleted => "deleted",
            EventKind::Restored => "restored",
            EventKind::DiskEdit => "disk_edit",
            EventKind::LockPutBack => "lock_put_back",
            EventKind::Signed => "signed",
            EventKind::Imported => "imported",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        EventKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What a History line is about (rev 5 G1): a Garden, a widget folder, or
/// the computer-wide look (`garden_id = '*'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum On<'a> {
    /// A Garden (`garden_id` set, `widget` NULL).
    Garden(&'a str),
    /// A widget folder's code (`widget` set, `garden_id` NULL): a widget
    /// write logs one row; a Garden's History shows it for the widgets
    /// placed in it at read time ([`events_for_garden`]).
    Widget(&'a str),
    /// Something that happened to one placement of a widget in a Garden
    /// (both set).
    GardenWidget(&'a str, &'a str),
    /// The computer-wide look (`zen.toml`, a theme with no Garden):
    /// `garden_id = '*'`.
    Computer,
}

/// `garden_id` for [`On::Computer`].
pub const COMPUTER_WIDE: &str = "*";

impl<'a> On<'a> {
    fn columns(self) -> Result<(Option<&'a str>, Option<&'a str>)> {
        let check_widget = |w: &str| -> Result<()> {
            if crate::zen::valid_theme_name(w) || crate::garden_data::authors::is_builtin(w) {
                Ok(())
            } else {
                Err(AccessError::BadRequest(format!("'{w}' isn't a widget folder name")))
            }
        };
        Ok(match self {
            On::Garden(g) => {
                check_garden(g)?;
                (Some(g), None)
            }
            On::Widget(w) => {
                check_widget(w)?;
                (None, Some(w))
            }
            On::GardenWidget(g, w) => {
                check_garden(g)?;
                check_widget(w)?;
                (Some(g), Some(w))
            }
            On::Computer => (Some(COMPUTER_WIDE), None),
        })
    }
}

/// One History line.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: i64,
    /// The Garden, `"*"` for the computer-wide look, or `None` for a
    /// widget-only line.
    pub garden_id: Option<String>,
    pub widget: Option<String>,
    pub at: i64,
    pub actor: String,
    pub kind: EventKind,
    /// Small JSON object: file, hash (12), snapshot, cardId, route,
    /// from/to names, workspace, via, note.
    pub detail: J,
}

/// Append one History line. `detail` must be a JSON object of at most
/// [`MAX_DETAIL_BYTES`] serialized (else 400: keep details small, hashes at
/// 12 characters). Returns the new row id. **The caller checks [`enabled`]**:
/// nothing writes History while the switch is off.
pub fn record_event(conn: &Connection, on: On<'_>, kind: EventKind, actor: &Actor, detail: &J, at: i64) -> Result<i64> {
    let (garden, widget) = on.columns()?;
    if !detail.is_object() {
        return Err(AccessError::BadRequest("an event's detail is a JSON object".into()));
    }
    let text = detail.to_string();
    if text.len() > MAX_DETAIL_BYTES {
        return Err(AccessError::BadRequest(format!(
            "an event's detail is {} bytes; the most is {MAX_DETAIL_BYTES}",
            text.len()
        )));
    }
    let label = actor.label();
    if label.len() > 160 {
        return Err(AccessError::BadRequest("an event's actor label is too long".into()));
    }
    conn.execute(
        "INSERT INTO zen_garden_events (garden_id, widget, at, actor, kind, detail_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![garden, widget, at, label, kind.as_str(), text],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Delete History lines older than `cutoff` (every Garden and widget).
/// Returns how many went. The daemon's daily sweep
/// (`garden_access_sweep`) runs it with `now - EVENTS_RETAIN_SECS`,
/// headless, switch on or off (it only ever deletes).
pub fn prune_events_older_than(conn: &Connection, cutoff: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM zen_garden_events WHERE at < ?1", params![cutoff])?)
}

/// [`prune_events_older_than`] at `now` with the 2-year rule.
pub fn prune_events(conn: &Connection, now: i64) -> Result<usize> {
    prune_events_older_than(conn, now.saturating_sub(EVENTS_RETAIN_SECS))
}

const EVENT_COLS: &str = "id, garden_id, widget, at, actor, kind, detail_json";

fn event_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let kind: String = r.get(5)?;
    let detail: String = r.get(6)?;
    let bad = |col: usize, what: String| {
        rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, what.into())
    };
    Ok(Event {
        id: r.get(0)?,
        garden_id: r.get(1)?,
        widget: r.get(2)?,
        at: r.get(3)?,
        actor: r.get(4)?,
        kind: EventKind::parse(&kind).ok_or_else(|| bad(5, format!("event kind '{kind}'")))?,
        detail: serde_json::from_str(&detail).map_err(|e| bad(6, format!("event detail: {e}")))?,
    })
}

/// A Garden's History (G29), newest first: its own lines plus the
/// widget-only lines of `placed` (the widget folders placed in it now; the
/// caller reads them from the page). At most `limit` lines, older than row
/// `before` when given (paging: pass the last line's `id`).
pub fn events_for_garden(
    conn: &Connection,
    garden: &str,
    placed: &[&str],
    limit: usize,
    before: Option<i64>,
) -> Result<Vec<Event>> {
    let widgets = serde_json::to_string(placed).map_err(|e| AccessError::Store(e.to_string()))?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {EVENT_COLS} FROM zen_garden_events
         WHERE (garden_id = ?1 OR (garden_id IS NULL AND widget IN (SELECT value FROM json_each(?2))))
           AND (?3 IS NULL OR id < ?3)
         ORDER BY id DESC LIMIT ?4"
    ))?;
    let rows = stmt
        .query_map(params![garden, widgets, before, limit as i64], event_from)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// A widget folder's History, newest first: every line naming it (its own
/// code changes and its placements' lines in any Garden).
pub fn events_for_widget(conn: &Connection, widget: &str, limit: usize, before: Option<i64>) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {EVENT_COLS} FROM zen_garden_events WHERE widget = ?1 AND (?2 IS NULL OR id < ?2) ORDER BY id DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![widget, before, limit as i64], event_from)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ── list changes ─────────────────────────────────────────────────────

/// `zen_garden_access.added_by`: why a row is on the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddedBy {
    /// The person (Settings, the CLI with the owner token, an Owner login,
    /// or their approve of a pending proposal).
    Owner,
    /// The person's Allow on a `garden.access` card.
    Card,
    /// The G4 pass on an off→on flip.
    Migration,
}

impl AddedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            AddedBy::Owner => "owner",
            AddedBy::Card => "card",
            AddedBy::Migration => "migration",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(AddedBy::Owner),
            "card" => Some(AddedBy::Card),
            "migration" => Some(AddedBy::Migration),
            _ => None,
        }
    }
}

/// How a list change happened (the event's `via`). An add maps to the row's
/// `added_by` through [`Via::added_by`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// The person through a route (Settings → Gardens, `k2 zen garden
    /// access <g> add|remove`): set `route`.
    Settings,
    /// The person's Allow on K2's `garden.access` card: set `card_id`.
    Card,
    /// The tighten twin `garden.access to=off`: set `card_id`.
    Twin,
    /// The person approved a pending proposal (G21): the proposer is listed.
    ProposalApprove,
    /// The G4 pass on an off→on flip.
    Migration,
    /// The workspace was deleted (G7).
    WorkspaceDelete,
    /// The Garden was deleted; its rows went to `access.json` (G8).
    GardenDelete,
    /// The Garden came back from history (G8).
    GardenRestore,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Settings => "settings",
            Via::Card => "card",
            Via::Twin => "twin",
            Via::ProposalApprove => "proposal_approve",
            Via::Migration => "migration",
            Via::WorkspaceDelete => "workspace_delete",
            Via::GardenDelete => "garden_delete",
            Via::GardenRestore => "garden_restore",
        }
    }

    /// The `added_by` an add through this door writes; `None` for doors
    /// that only remove (a restore keeps the archived value).
    pub fn added_by(self) -> Option<AddedBy> {
        match self {
            Via::Settings | Via::ProposalApprove => Some(AddedBy::Owner),
            Via::Card => Some(AddedBy::Card),
            Via::Migration => Some(AddedBy::Migration),
            Via::Twin | Via::WorkspaceDelete | Via::GardenDelete | Via::GardenRestore => None,
        }
    }
}

/// One list change as the caller describes it: who, how, the card or
/// route, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change<'a> {
    pub via: Via,
    pub actor: &'a Actor,
    pub card_id: Option<&'a str>,
    pub route: Option<&'a str>,
    /// A short line for History (the pass: "added automatically at upgrade").
    pub note: Option<&'a str>,
    pub at: i64,
}

impl<'a> Change<'a> {
    /// A change with no card and no route.
    pub fn new(via: Via, actor: &'a Actor, at: i64) -> Self {
        Change { via, actor, card_id: None, route: None, note: None, at }
    }

    pub fn card(mut self, card_id: &'a str) -> Self {
        self.card_id = Some(card_id);
        self
    }

    pub fn route(mut self, route: &'a str) -> Self {
        self.route = Some(route);
        self
    }

    pub fn note(mut self, note: &'a str) -> Self {
        self.note = Some(note);
        self
    }

    fn detail(&self, project_id: &str) -> J {
        let mut d = json!({ "workspace": project_id, "via": self.via.as_str() });
        if let Some(c) = self.card_id {
            d["cardId"] = json!(c);
        }
        if let Some(r) = self.route {
            d["route"] = json!(r);
        }
        if let Some(n) = self.note {
            d["note"] = json!(n);
        }
        d
    }
}

// ── rows ─────────────────────────────────────────────────────────────

/// One workspace on a Garden's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRow {
    pub garden_id: String,
    pub project_id: String,
    pub added_by: AddedBy,
    pub added_at: i64,
    pub card_id: Option<String>,
}

/// Who made a Garden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub garden_id: String,
    pub made_by_project: String,
    pub made_at: i64,
}

/// Which of K2's own cards an ask is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AskKind {
    /// `garden.access`: to edit the Garden.
    Access,
    /// `garden.delete`: to have the person delete it.
    Delete,
}

impl AskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AskKind::Access => "access",
            AskKind::Delete => "delete",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "access" => Some(AskKind::Access),
            "delete" => Some(AskKind::Delete),
            _ => None,
        }
    }
}

/// The asks store for one (Garden, agent, kind).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskRow {
    pub garden_id: String,
    pub project_id: String,
    pub kind: AskKind,
    pub last_ask_at: Option<i64>,
    pub last_not_now_at: Option<i64>,
    /// The waiting card, if one is out.
    pub card_id: Option<String>,
}

// ── checks ───────────────────────────────────────────────────────────

fn check_garden(garden: &str) -> Result<()> {
    if valid_garden_id(garden) {
        Ok(())
    } else {
        Err(AccessError::BadRequest(format!("'{garden}' isn't a Garden id")))
    }
}

/// Does workspace `project_id` exist?
pub fn workspace_exists(conn: &Connection, project_id: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM projects WHERE id = ?1", params![project_id], |_| Ok(())).optional()?.is_some())
}

fn audit(action: &str, garden: &str, project: &str, c: &Change<'_>) {
    let mut outcome = format!("ok {action} garden={garden} workspace={project} via={}", c.via.as_str());
    if let Some(card) = c.card_id {
        outcome.push_str(&format!(" card={card}"));
    }
    if let Some(route) = c.route {
        outcome.push_str(&format!(" route={route}"));
    }
    crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
        AUDIT_EVENT,
        &c.actor.label(),
        outcome,
        "-",
        "-",
        "cli",
    ));
}

// ── the list ─────────────────────────────────────────────────────────

/// Put workspace `project_id` on Garden `garden`'s list. `Ok(true)` when it
/// was added; `Ok(false)` when it was already there (nothing is written).
/// The workspace must exist, and `change.via` must be a door that adds
/// ([`Via::added_by`]). Writes an `access_added` event and an Access Audit
/// line.
pub fn add(conn: &Connection, garden: &str, project_id: &str, change: &Change<'_>) -> Result<bool> {
    let added_by = change
        .via
        .added_by()
        .ok_or_else(|| AccessError::BadRequest(format!("'{}' doesn't add to a list", change.via.as_str())))?;
    insert_row(conn, garden, project_id, added_by, change.at, change.card_id, change)
}

fn insert_row(
    conn: &Connection,
    garden: &str,
    project_id: &str,
    added_by: AddedBy,
    added_at: i64,
    card_id: Option<&str>,
    change: &Change<'_>,
) -> Result<bool> {
    check_garden(garden)?;
    if !workspace_exists(conn, project_id)? {
        return Err(AccessError::BadRequest(format!("no workspace '{project_id}' on this server")));
    }
    let n = conn.execute(
        "INSERT OR IGNORE INTO zen_garden_access (garden_id, project_id, added_by, added_at, card_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![garden, project_id, added_by.as_str(), added_at, card_id],
    )?;
    if n == 0 {
        return Ok(false);
    }
    record_event(conn, On::Garden(garden), EventKind::AccessAdded, change.actor, &change.detail(project_id), change.at)?;
    audit("add", garden, project_id, change);
    Ok(true)
}

/// Take workspace `project_id` off Garden `garden`'s list. `Ok(true)` when
/// it was there. Writes an `access_removed` event and an Access Audit line.
/// Takes effect on the next request: there is no cache.
pub fn remove(conn: &Connection, garden: &str, project_id: &str, change: &Change<'_>) -> Result<bool> {
    check_garden(garden)?;
    let n = conn.execute(
        "DELETE FROM zen_garden_access WHERE garden_id = ?1 AND project_id = ?2",
        params![garden, project_id],
    )?;
    if n == 0 {
        return Ok(false);
    }
    record_event(conn, On::Garden(garden), EventKind::AccessRemoved, change.actor, &change.detail(project_id), change.at)?;
    audit("remove", garden, project_id, change);
    Ok(true)
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccessRow> {
    let added_by: String = r.get(2)?;
    Ok(AccessRow {
        garden_id: r.get(0)?,
        project_id: r.get(1)?,
        added_by: AddedBy::parse(&added_by).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, format!("added_by '{added_by}'").into())
        })?,
        added_at: r.get(3)?,
        card_id: r.get(4)?,
    })
}

const ROW_COLS: &str = "garden_id, project_id, added_by, added_at, card_id";

/// Garden `garden`'s list, oldest first.
pub fn list(conn: &Connection, garden: &str) -> Result<Vec<AccessRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ROW_COLS} FROM zen_garden_access WHERE garden_id = ?1 ORDER BY added_at, project_id"
    ))?;
    let rows = stmt.query_map(params![garden], row_from)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every row, by Garden then age.
pub fn all_rows(conn: &Connection) -> Result<Vec<AccessRow>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {ROW_COLS} FROM zen_garden_access ORDER BY garden_id, added_at, project_id"))?;
    let rows = stmt.query_map([], row_from)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Is workspace `project_id` on Garden `garden`'s list?
pub fn is_listed(conn: &Connection, garden: &str, project_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM zen_garden_access WHERE garden_id = ?1 AND project_id = ?2",
            params![garden, project_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Is workspace `project_id` listed on **every** Garden in `gardens`? (A
/// widget placed in several Gardens, §1.7.) An empty slice is `false`: a
/// widget placed nowhere is a draft, which A2 rules on separately.
pub fn listed_on_all(conn: &Connection, gardens: &[&str], project_id: &str) -> Result<bool> {
    if gardens.is_empty() {
        return Ok(false);
    }
    for g in gardens {
        if !is_listed(conn, g, project_id)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Is workspace `project_id` on at least one Garden's list? (Rev 3 Q6: the
/// computer-wide look changes are open only to such agents.) Uses the
/// `project_id` index.
pub fn listed_anywhere(conn: &Connection, project_id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM zen_garden_access WHERE project_id = ?1 LIMIT 1", params![project_id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// The Gardens workspace `project_id` is listed on, sorted.
pub fn gardens_of(conn: &Connection, project_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT garden_id FROM zen_garden_access WHERE project_id = ?1 ORDER BY garden_id")?;
    let ids = stmt.query_map(params![project_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

// ── made ─────────────────────────────────────────────────────────────

/// Record that workspace `project_id` made Garden `garden`. A Garden is made
/// once: `Ok(false)` when a row exists (it is never overwritten). Writes no
/// event: the caller writes `made` with its own actor and detail.
pub fn record_made(conn: &Connection, garden: &str, project_id: &str, at: i64) -> Result<bool> {
    check_garden(garden)?;
    if project_id.trim().is_empty() {
        return Err(AccessError::BadRequest("a made row names its workspace".into()));
    }
    Ok(conn.execute(
        "INSERT OR IGNORE INTO zen_garden_made (garden_id, made_by_project, made_at) VALUES (?1, ?2, ?3)",
        params![garden, project_id, at],
    )? == 1)
}

/// Who made Garden `garden`, if an agent did.
pub fn made(conn: &Connection, garden: &str) -> Result<Option<Made>> {
    Ok(conn
        .query_row(
            "SELECT garden_id, made_by_project, made_at FROM zen_garden_made WHERE garden_id = ?1",
            params![garden],
            |r| Ok(Made { garden_id: r.get(0)?, made_by_project: r.get(1)?, made_at: r.get(2)? }),
        )
        .optional()?)
}

/// Gardens workspace `project_id` made at or after `since` (the budget:
/// A2 refuses the 6th within a day, [`MADE_PER_DAY`]).
pub fn made_since(conn: &Connection, project_id: &str, since: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM zen_garden_made WHERE made_by_project = ?1 AND made_at >= ?2",
        params![project_id, since],
        |r| r.get(0),
    )?)
}

// ── asks ─────────────────────────────────────────────────────────────

/// The asks row for (Garden, agent, kind), if any.
pub fn ask_state(conn: &Connection, garden: &str, project_id: &str, kind: AskKind) -> Result<Option<AskRow>> {
    Ok(conn
        .query_row(
            "SELECT garden_id, project_id, kind, last_ask_at, last_not_now_at, card_id
             FROM zen_garden_access_asks WHERE garden_id = ?1 AND project_id = ?2 AND kind = ?3",
            params![garden, project_id, kind.as_str()],
            |r| {
                let k: String = r.get(2)?;
                Ok(AskRow {
                    garden_id: r.get(0)?,
                    project_id: r.get(1)?,
                    kind: AskKind::parse(&k).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, format!("ask kind '{k}'").into())
                    })?,
                    last_ask_at: r.get(3)?,
                    last_not_now_at: r.get(4)?,
                    card_id: r.get(5)?,
                })
            },
        )
        .optional()?)
}

/// Record an ask: card `card_id` is now the one waiting for (Garden, agent,
/// kind).
pub fn record_ask(conn: &Connection, garden: &str, project_id: &str, kind: AskKind, card_id: &str, at: i64) -> Result<()> {
    check_garden(garden)?;
    conn.execute(
        "INSERT INTO zen_garden_access_asks (garden_id, project_id, kind, last_ask_at, card_id) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (garden_id, project_id, kind) DO UPDATE SET last_ask_at = excluded.last_ask_at, card_id = excluded.card_id",
        params![garden, project_id, kind.as_str(), at, card_id],
    )?;
    Ok(())
}

/// Record the person's "Not now" (or "Keep it" on a delete ask): the waiting
/// card is answered (cleared) and no new card of that kind appears for
/// [`NOT_NOW_QUIET_SECS`].
pub fn record_not_now(conn: &Connection, garden: &str, project_id: &str, kind: AskKind, at: i64) -> Result<()> {
    check_garden(garden)?;
    conn.execute(
        "INSERT INTO zen_garden_access_asks (garden_id, project_id, kind, last_not_now_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (garden_id, project_id, kind) DO UPDATE SET last_not_now_at = excluded.last_not_now_at, card_id = NULL",
        params![garden, project_id, kind.as_str(), at],
    )?;
    Ok(())
}

/// The waiting card was answered with Allow, voided or expired: clear it
/// (the Not now time stays).
pub fn clear_ask_card(conn: &Connection, garden: &str, project_id: &str, kind: AskKind) -> Result<()> {
    conn.execute(
        "UPDATE zen_garden_access_asks SET card_id = NULL WHERE garden_id = ?1 AND project_id = ?2 AND kind = ?3",
        params![garden, project_id, kind.as_str()],
    )?;
    Ok(())
}

/// May K2 present a new card for this asks row at `now`? Not while one is
/// waiting, and not within [`NOT_NOW_QUIET_SECS`] of a Not now. (A4 decides
/// whether a waiting card is still live; this reads only the store.)
pub fn may_ask(row: Option<&AskRow>, now: i64) -> bool {
    match row {
        None => true,
        Some(r) => {
            r.card_id.is_none() && r.last_not_now_at.map_or(true, |t| now.saturating_sub(t) >= NOT_NOW_QUIET_SECS)
        }
    }
}

// ── G7: workspace delete ─────────────────────────────────────────────

/// A workspace was deleted (G7, from `projects_ops::projects_delete`): its
/// list rows go (each with an `access_removed` event, actor `k2`, via
/// `workspace_delete`), and its asks and made rows go. Its History lines
/// stay, labels and all. Returns how many list rows went.
pub fn delete_for_workspace(conn: &Connection, project_id: &str, at: i64) -> Result<usize> {
    let gardens = gardens_of(conn, project_id)?;
    let change = Change::new(Via::WorkspaceDelete, &Actor::K2, at);
    let mut n = 0;
    for g in &gardens {
        if remove(conn, g, project_id, &change)? {
            n += 1;
        }
    }
    conn.execute("DELETE FROM zen_garden_access_asks WHERE project_id = ?1", params![project_id])?;
    conn.execute("DELETE FROM zen_garden_made WHERE made_by_project = ?1", params![project_id])?;
    Ok(n)
}

// ── G8: Garden delete and restore ────────────────────────────────────

/// What `access.json` holds for one deleted Garden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Archived {
    pub garden_id: String,
    pub archived_at: i64,
    pub access: Vec<AccessRow>,
    pub made: Option<Made>,
}

/// `.history/gardens/<id>.toml/access.json` under `files`' root.
pub fn mirror_path(files: &ZenFiles, garden: &str) -> PathBuf {
    files.history_root().join(ZenFile::Garden(garden.to_string()).label()).join(MIRROR_FILE)
}

fn write_json(path: &Path, v: &Archived) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| AccessError::Store(format!("{}: {e}", dir.display())))?;
    }
    let body = serde_json::to_string_pretty(v).map_err(|e| AccessError::Store(e.to_string()))?;
    crate::fs_atomic::atomic_write_str(path, &(body + "\n"))
        .map_err(|e| AccessError::Store(format!("{}: {e}", path.display())))
}

/// Garden `garden` was deleted (G8; call it after `ZenFiles::delete_garden`
/// succeeded): its list and made rows are written to `access.json` in its
/// history folder, then removed (each list row with an `access_removed`
/// event via `garden_delete`), and its asks go. History lines stay; the
/// caller adds the `deleted` line when the switch is on. With no list or
/// made rows nothing is written (so a server that never turned the switch
/// on keeps 0.46.3's history folder byte for byte). Returns the path when a
/// file was written.
pub fn archive_garden(conn: &Connection, files: &ZenFiles, garden: &str, actor: &Actor, at: i64) -> Result<Option<PathBuf>> {
    check_garden(garden)?;
    let access = list(conn, garden)?;
    let made_row = made(conn, garden)?;
    conn.execute("DELETE FROM zen_garden_access_asks WHERE garden_id = ?1", params![garden])?;
    if access.is_empty() && made_row.is_none() {
        return Ok(None);
    }
    let path = mirror_path(files, garden);
    write_json(&path, &Archived { garden_id: garden.to_string(), archived_at: at, access: access.clone(), made: made_row })?;
    let change = Change::new(Via::GardenDelete, actor, at);
    for row in &access {
        remove(conn, garden, &row.project_id, &change)?;
    }
    conn.execute("DELETE FROM zen_garden_made WHERE garden_id = ?1", params![garden])?;
    Ok(Some(path))
}

/// Garden `garden` came back from history (G8): put back the rows
/// [`archive_garden`] kept, with their original `added_by`, `added_at` and
/// card; each gets an `access_added` event via `garden_restore`. Workspaces
/// deleted since are skipped. Idempotent (rows already there stay). The
/// caller adds the `restored` line. Returns how many list rows came back;
/// no `access.json` → 0. (0.46.3 has no Garden restore route; this is the
/// store half for the one that comes.)
pub fn restore_garden(conn: &Connection, files: &ZenFiles, garden: &str, actor: &Actor, at: i64) -> Result<usize> {
    check_garden(garden)?;
    let path = mirror_path(files, garden);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(AccessError::Store(format!("{}: {e}", path.display()))),
    };
    let archived: Archived = serde_json::from_str(&text)
        .map_err(|e| AccessError::Store(format!("{} doesn't parse: {e}", path.display())))?;
    if archived.garden_id != garden {
        return Err(AccessError::Store(format!("{} is for Garden '{}'", path.display(), archived.garden_id)));
    }
    let change = Change::new(Via::GardenRestore, actor, at);
    let mut n = 0;
    for row in &archived.access {
        if !workspace_exists(conn, &row.project_id)? {
            continue;
        }
        if insert_row(conn, garden, &row.project_id, row.added_by, row.added_at, row.card_id.as_deref(), &change)? {
            n += 1;
        }
    }
    if let Some(m) = &archived.made {
        if workspace_exists(conn, &m.made_by_project)? {
            record_made(conn, garden, &m.made_by_project, m.made_at)?;
        }
    }
    Ok(n)
}
