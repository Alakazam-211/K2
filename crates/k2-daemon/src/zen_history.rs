//! Garden History, the parts slice A6 owns (prd-garden-access-v1 rev 5,
//! G29). **Switch on only**: with `gardenAccess` off nothing here writes,
//! and the History routes don't answer at all (`zen_access_routes`).
//!
//! - **Edits outside K2 (`disk_edit`)**: [`after_refresh`] keeps the live
//!   text hash of every Garden page and the live version hash of every
//!   widget folder, as K2 last saw them. The routes' own refreshes move
//!   that baseline silently (a route writes, then refreshes before it
//!   answers); only a refresh from the folder watcher (250 ms after the
//!   file settles) that finds a Garden or a widget changed writes one
//!   `disk_edit` line, actor `disk`. A locked Garden's edit is put back
//!   before the hashes are taken, so it writes `lock_put_back` instead
//!   ([`lock_put_back`]), never `disk_edit`.
//! - **The merge**: [`merge`] adds the three older sources to a page of
//!   `zen_garden_events` lines, newest first:
//!   1. `.history` snapshots of the Garden page and of each widget folder,
//!      as "version saved" lines (source `snapshot`, actor K2) where no
//!      change line covers that time; a change line that a snapshot does
//!      cover gets `snapshot: {file, name}` (View this version, Restore);
//!   2. `zen_widget_authors` stamps older than the widget's first line
//!      (source `stamp`, kind `signed`);
//!   3. decided `zen_proposals` older than their target's first line
//!      (source `proposal`, kind `changed` / `widget_changed`).
//!
//!   The page keeps the events' cursor (`event:<id>`): a page holds the
//!   merged items whose time falls between the page's oldest line and the
//!   previous page's oldest line, and the last page holds everything older.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value as J};

use k2_core::zen::garden_access::{self as ga, Actor, EventKind, On};
use k2_core::zen::{ZenFile, ZenFiles};

// ── edits outside K2 ─────────────────────────────────────────────────

/// What K2 last saw: `g:<id>` → page text hash, `w:<name>` → live version
/// hash. `None` until the first refresh with the switch on (a baseline).
static SEEN: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

fn live_state(f: &ZenFiles) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for g in f.gardens() {
        let text = f.garden_text(&g.id).ok().and_then(|(_, t)| t);
        out.insert(format!("g:{}", g.id), k2_core::zen::agent_edit::text_hash(text.as_deref()));
    }
    for w in f.widget_names() {
        let st = f.widget_status(&w);
        let hash = st.hash();
        if !hash.is_empty() {
            out.insert(format!("w:{w}"), hash.to_string());
        }
    }
    out
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// After a refresh of the Zen folder (the caller holds the refresh lock).
/// `from_disk`: the folder watcher's refresh (a change no route made).
/// Answers the `disk_edit` lines written, as `(key, hash)`.
pub(crate) fn after_refresh(f: &ZenFiles, from_disk: bool, at: i64) -> Vec<(String, String)> {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !ga::enabled() {
        *seen = None;
        return Vec::new();
    }
    let now_state = live_state(f);
    let mut wrote = Vec::new();
    if from_disk {
        if let Some(prev) = seen.as_ref() {
            for (key, hash) in &now_state {
                if prev.get(key) == Some(hash) {
                    continue;
                }
                let new = !prev.contains_key(key);
                let (on, file) = match key.split_once(':') {
                    Some(("g", id)) => (On::Garden(id), ZenFile::Garden(id.to_string()).label()),
                    Some(("w", w)) => (On::Widget(w), k2_core::zen::widget_store::widget_label(w)),
                    _ => continue,
                };
                let detail = json!({ "file": file, "hash": short(hash), "new": new });
                let db = k2_core::db::shared();
                let conn = db.lock();
                match ga::record_event(&conn, on, EventKind::DiskEdit, &Actor::Disk, &detail, at) {
                    Ok(_) => wrote.push((key.clone(), hash.clone())),
                    Err(e) => k2_core::log_debug!("[zen/history] disk_edit {key}: {e:?}"),
                }
            }
        }
    }
    *seen = Some(now_state);
    wrote
}

/// Forget the baseline (tests; a switch flip rebuilds it on the next refresh).
#[cfg(test)]
pub(crate) fn forget_baseline() {
    *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// One `lock_put_back` line per locked-Garden edit the refresh put back
/// (switch on). The daemon can't tell who saved the file: actor `disk`.
pub(crate) fn lock_put_back(refused: &[k2_core::zen::lock::LockRefusal], at: i64) {
    if !ga::enabled() || refused.is_empty() {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    for r in refused {
        let detail = json!({ "name": r.name, "kept": r.kept });
        if let Err(e) = ga::record_event(&conn, On::Garden(&r.garden), EventKind::LockPutBack, &Actor::Disk, &detail, at) {
            k2_core::log_debug!("[zen/history] lock_put_back {}: {e:?}", r.garden);
        }
    }
}

// ── the merge ────────────────────────────────────────────────────────

/// What a History route shows.
pub(crate) enum Scope<'a> {
    /// A Garden (live or deleted) and the widgets placed in it now. The
    /// computer-wide look (`*`) merges nothing.
    Garden { id: &'a str, placed: &'a [String] },
    /// One widget folder.
    Widget(&'a str),
}

/// A target of change lines: the Garden page, or a widget folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    Page(String),
    Widget(String),
}

impl Target {
    fn file(&self) -> String {
        match self {
            Target::Page(g) => ZenFile::Garden(g.clone()).label(),
            Target::Widget(w) => k2_core::zen::widget_store::widget_label(w),
        }
    }
}

/// A kept version: when, its name, and (a widget's) code hash.
#[derive(Debug, Clone)]
struct Snap {
    target: Target,
    at: i64,
    name: String,
    hash: Option<String>,
}

/// Change lines that a snapshot taken at about the same time belongs to.
const CHANGE_KINDS: &[&str] = &["made", "changed", "widget_changed", "reset", "template", "disk_edit", "restored"];

/// A snapshot belongs to the nearest change line on its file written
/// within this many seconds of it (a route writes its line just after its
/// own refresh kept the copy; a version a route didn't refresh is kept by
/// the watcher's refresh 250 ms later).
const COVER_SECS: i64 = 5;

fn rfc3339_secs(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

fn snaps(f: &ZenFiles, targets: &[Target]) -> Vec<Snap> {
    let mut out = Vec::new();
    for t in targets {
        match t {
            Target::Page(g) => {
                for s in f.snapshots(&ZenFile::Garden(g.clone())) {
                    if let Some(at) = rfc3339_secs(&s.at) {
                        out.push(Snap { target: t.clone(), at, name: s.name, hash: None });
                    }
                }
            }
            Target::Widget(w) => {
                for s in f.widget_snapshots(w) {
                    if let Some(at) = rfc3339_secs(&s.at) {
                        out.push(Snap { target: t.clone(), at, name: s.name, hash: s.hash });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then(a.name.cmp(&b.name)));
    out
}

/// `(id, at)` of the change lines on `t` between `from` and `to`, oldest first.
fn change_lines(conn: &rusqlite::Connection, t: &Target, from: i64, to: i64) -> rusqlite::Result<Vec<(i64, i64)>> {
    let kinds = CHANGE_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    let (sql, key) = match t {
        Target::Page(g) => (
            format!("SELECT id, at FROM zen_garden_events WHERE garden_id = ?1 AND widget IS NULL AND kind IN ({kinds}) AND at BETWEEN ?2 AND ?3 ORDER BY at, id"),
            g,
        ),
        Target::Widget(w) => (
            format!("SELECT id, at FROM zen_garden_events WHERE widget = ?1 AND kind IN ({kinds}) AND at BETWEEN ?2 AND ?3 ORDER BY at, id"),
            w,
        ),
    };
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params![key, from, to], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    rows.collect()
}

/// When the first History line about `t` was written (`None`: never).
fn first_line_at(conn: &rusqlite::Connection, t: &Target) -> rusqlite::Result<Option<i64>> {
    let (sql, key) = match t {
        Target::Page(g) => ("SELECT MIN(at) FROM zen_garden_events WHERE garden_id = ?1 AND widget IS NULL", g),
        Target::Widget(w) => ("SELECT MIN(at) FROM zen_garden_events WHERE widget = ?1", w),
    };
    conn.query_row(sql, params![key], |r| r.get::<_, Option<i64>>(0))
}

/// When History line `id` was written (the `before` cursor's time).
fn line_at(conn: &rusqlite::Connection, id: i64) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT at FROM zen_garden_events WHERE id = ?1", params![id], |r| r.get(0)).optional()
}

/// Which snapshot each change line gets (event id → snapshot), and the
/// snapshots no change line covers.
fn cover(conn: &rusqlite::Connection, all: &[Snap]) -> rusqlite::Result<(HashMap<i64, Snap>, Vec<Snap>)> {
    let mut linked: HashMap<i64, Snap> = HashMap::new();
    let mut loose = Vec::new();
    let mut by_target: BTreeMap<Target, Vec<&Snap>> = BTreeMap::new();
    for s in all {
        by_target.entry(s.target.clone()).or_default().push(s);
    }
    for (t, list) in by_target {
        let from = list.iter().map(|s| s.at).min().unwrap_or(0) - COVER_SECS;
        let to = list.iter().map(|s| s.at).max().unwrap_or(0) + COVER_SECS;
        let lines = change_lines(conn, &t, from, to)?;
        for s in list {
            // The nearest change line within ±5 s (the later one on a tie);
            // a later snapshot for the same line replaces an earlier one
            // (the version the change left, not the copy kept before it).
            let line = lines
                .iter()
                .filter(|(_, at)| (*at - s.at).abs() <= COVER_SECS)
                .min_by_key(|(id, at)| ((*at - s.at).abs(), -*id));
            match line {
                Some((id, _)) => {
                    linked.insert(*id, s.clone());
                }
                None => loose.push(s.clone()),
            }
        }
    }
    Ok((linked, loose))
}

fn snapshot_json(s: &Snap) -> J {
    json!({ "file": s.target.file(), "name": s.name })
}

fn snap_item(s: &Snap, garden: Option<&str>) -> J {
    let (kind, widget) = match &s.target {
        Target::Page(_) => ("changed", J::Null),
        Target::Widget(w) => ("widget_changed", json!(w)),
    };
    let mut detail = json!({ "file": s.target.file(), "name": s.name });
    if let Some(h) = &s.hash {
        detail["hash"] = json!(short(h));
    }
    json!({
        "id": format!("snapshot:{}:{}", s.target.file(), s.name),
        "at": s.at,
        "kind": kind,
        "source": "snapshot",
        "actor": crate::zen_access_routes::actor_json(Actor::K2.label().as_str()),
        "garden": if matches!(s.target, Target::Page(_)) { json!(garden) } else { J::Null },
        "widget": widget,
        "detail": detail,
        "snapshot": snapshot_json(s),
    })
}

/// `zen_widget_authors` rows of `widget` signed before `before`.
fn stamp_items(conn: &rusqlite::Connection, widget: &str, before: i64, snaps: &[Snap]) -> rusqlite::Result<Vec<J>> {
    let mut st = conn.prepare(
        "SELECT hash, author_kind, author_project_id, source, caller_kind, signed_at FROM zen_widget_authors
         WHERE widget = ?1 AND signed_at < ?2 ORDER BY signed_at DESC, hash",
    )?;
    let rows = st.query_map(params![widget, before], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (hash, kind, project, source, caller_kind, at) = row?;
        let actor = match (kind.as_str(), project) {
            ("owner", _) => Actor::Owner,
            (_, Some(p)) => Actor::Agent(p),
            _ => Actor::K2,
        };
        let snap = snaps
            .iter()
            .filter(|s| s.target == Target::Widget(widget.to_string()))
            .find(|s| s.hash.as_deref() == Some(hash.as_str()))
            .map(snapshot_json)
            .unwrap_or(J::Null);
        out.push(json!({
            "id": format!("stamp:{widget}:{}", short(&hash)),
            "at": at,
            "kind": "signed",
            "source": "stamp",
            "actor": crate::zen_access_routes::actor_json(&actor.label()),
            "garden": J::Null,
            "widget": widget,
            "detail": { "hash": short(&hash), "source": source, "callerKind": caller_kind },
            "snapshot": snap,
        }));
    }
    Ok(out)
}

/// Decided proposals for `t` decided before `before`.
fn proposal_items(conn: &rusqlite::Connection, t: &Target, before: i64) -> rusqlite::Result<Vec<J>> {
    let (sql, key) = match t {
        Target::Page(g) => (
            "SELECT id, target, file, status, made_by_project, decided_by, decided_at, note FROM zen_proposals
             WHERE kind = 'garden' AND target = ?1 AND status IN ('approved', 'rejected') AND decided_at < ?2
             ORDER BY decided_at DESC, id",
            g,
        ),
        Target::Widget(w) => (
            "SELECT id, target, file, status, made_by_project, decided_by, decided_at, note FROM zen_proposals
             WHERE kind <> 'garden' AND target = ?1 AND status IN ('approved', 'rejected') AND decided_at < ?2
             ORDER BY decided_at DESC, id",
            w,
        ),
    };
    let mut st = conn.prepare(sql)?;
    let rows = st.query_map(params![key, before], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, target, file, status, proposer, decided_by, at, note) = row?;
        let (kind, garden, widget) = match t {
            Target::Page(g) => ("changed", json!(g), J::Null),
            Target::Widget(_) => ("widget_changed", J::Null, json!(target)),
        };
        out.push(json!({
            "id": format!("proposal:{id}"),
            "at": at,
            "kind": kind,
            "source": "proposal",
            "actor": crate::zen_access_routes::actor_json(&Actor::Agent(proposer.clone()).label()),
            "garden": garden,
            "widget": widget,
            "detail": { "proposal": id, "decision": status, "decidedBy": decided_by, "file": file, "note": note, "proposer": proposer },
            "snapshot": J::Null,
        }));
    }
    Ok(out)
}

fn item_kind_ok(item: &J, kinds: &Option<BTreeSet<&'static str>>) -> bool {
    kinds.as_ref().is_none_or(|k| item["kind"].as_str().is_some_and(|s| k.contains(s)))
}

/// Merge the older sources into one page of event items (G29).
///
/// `events` is the page `zen_access_routes` read (newest first) and
/// `items` their JSON, in the same order; `before` is the request's
/// cursor (an event id) and `next` the answer's.
pub(crate) fn merge(
    f: &ZenFiles,
    scope: Scope<'_>,
    events: &[ga::Event],
    mut items: Vec<J>,
    before: Option<i64>,
    next: Option<i64>,
    kinds: &Option<BTreeSet<&'static str>>,
) -> rusqlite::Result<Vec<J>> {
    let (garden, targets): (Option<&str>, Vec<Target>) = match scope {
        Scope::Garden { id, .. } if id == ga::COMPUTER_WIDE => return Ok(items),
        Scope::Garden { id, placed } => {
            let mut t = vec![Target::Page(id.to_string())];
            t.extend(placed.iter().map(|w| Target::Widget(w.clone())));
            (Some(id), t)
        }
        Scope::Widget(w) => (None, vec![Target::Widget(w.to_string())]),
    };
    let all_snaps = snaps(f, &targets);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let upper = match before {
        Some(id) => line_at(&conn, id)?.unwrap_or(i64::MAX),
        None => i64::MAX,
    };
    let lower = match (next, events.last()) {
        (Some(_), Some(e)) => e.at,
        _ => i64::MIN,
    };
    let in_page = |at: i64| at > lower && at <= upper;

    let (linked, loose) = cover(&conn, &all_snaps)?;
    for (e, item) in events.iter().zip(items.iter_mut()) {
        if let Some(s) = linked.get(&e.id) {
            item["snapshot"] = snapshot_json(s);
        }
    }
    let mut extra: Vec<J> = loose.iter().filter(|s| in_page(s.at)).map(|s| snap_item(s, garden)).collect();
    for t in &targets {
        let first = first_line_at(&conn, t)?.unwrap_or(i64::MAX);
        if let Target::Widget(w) = t {
            extra.extend(stamp_items(&conn, w, first, &all_snaps)?.into_iter().filter(|i| i["at"].as_i64().is_some_and(in_page)));
        }
        extra.extend(proposal_items(&conn, t, first)?.into_iter().filter(|i| i["at"].as_i64().is_some_and(in_page)));
    }
    drop(conn);
    items.extend(extra.into_iter().filter(|i| item_kind_ok(i, kinds)));
    // Newest first; at the same second an event line comes before the
    // older sources. The sort is stable: events keep the store's order
    // (newest id first), the rest the order built above.
    let rank = |i: &J| if i["source"] == "event" { 0 } else { 1 };
    items.sort_by(|a, b| b["at"].as_i64().cmp(&a["at"].as_i64()).then(rank(a).cmp(&rank(b))));
    Ok(items)
}

#[cfg(test)]
#[path = "zen_history_tests.rs"]
mod tests;
