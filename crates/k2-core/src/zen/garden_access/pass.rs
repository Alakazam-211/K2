//! The G4 pass: what an off→on flip of `gardenAccess` adds to the tables
//! (prd-garden-access-v1 rev 4 G4, Rosson's Q2 answer 2026-10-22).
//!
//! It is a daemon pass, not SQL, because Gardens are files. It is
//! **add-only** and runs on **every** off→on flip, so Gardens changed while
//! the switch was off are covered; a second run adds nothing new. It never
//! removes a row.
//!
//! For each Garden in `gardens.json`:
//! - **The list** (while [`GRANDFATHER`] is true): every live workspace that
//!   built or edited in the Garden under 0.46.3 is listed
//!   (`added_by = 'migration'`), with one `access_added` History line
//!   (actor `migration`, note [`AUTO_NOTE`]). That is:
//!   - the agent maker of a placed widget's live version
//!     (`zen_widget_authors`; the frozen copy for a locked Garden);
//!   - the proposer of an approved proposal for the Garden or a widget
//!     placed in it (`zen_proposals`);
//!   - the actor of an Access Audit `zen-garden-put`, `zen-widget-put` or
//!     `zen-widget-folder-put` for the Garden or a widget placed in it.
//!
//!   G6: Manage-Gardens switch holders that edited nothing aren't listed.
//!   An agent the person took off a Garden's list (Settings, the card, the
//!   twin) is never re-added to that Garden by a later run.
//! - **Made:** `zen_garden_made` from an agent's `zen-garden-new` in the
//!   Access Audit, when the Garden still exists, plus a `made` line.
//! - **Imported History:** the `zen-*` / `zen.*` Access Audit lines still in
//!   the file that name the Garden (or a widget placed in it) become
//!   `imported` lines, once each (a re-run finds them already there).
//!
//! There is no sight and no legacy ceiling (rev 4: no sight limits).

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value as J};

use super::{
    add, record_event, record_made, workspace_exists, Actor, Change, EventKind, On, Result, Via, AUDIT_PASS_EVENT,
};
use crate::garden_data::{authors, MadeBy};
use crate::zen::{schema, ZenFile, ZenFiles};

/// Rosson's Q2 (2026-10-22): grandfather every agent that built or edited
/// in an existing Garden. One constant, so the answer can change in one
/// place: `false` lists nobody (every agent asks once) and keeps the rest.
pub const GRANDFATHER: bool = true;

/// The History note on each automatic add.
pub const AUTO_NOTE: &str = "added automatically at upgrade";

/// The Access Audit events that mark an agent's direct edit (0.46.3, the
/// Manage Gardens switch: `zen_agent_routes.rs` `garden_put`, `widget_put`,
/// `folder_put`).
pub const EDIT_EVENTS: &[&str] = &["zen-garden-put", "zen-widget-put", "zen-widget-folder-put"];

/// The Access Audit event for an agent's `garden/new` (`zen_routes.rs`).
pub const NEW_EVENT: &str = "zen-garden-new";

/// The longest Access Audit outcome an `imported` line keeps (the detail
/// must fit [`super::MAX_DETAIL_BYTES`]).
const IMPORT_OUTCOME_CHARS: usize = 1_200;

/// Why a workspace is a candidate for a Garden.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reason {
    /// It made the live version of `widget`, placed in the Garden.
    LiveMaker { widget: String },
    /// Its proposal `id` (for the Garden or a widget in it) was approved.
    ApprovedProposal { id: String },
    /// An Access Audit edit (`event`) by it, on `target`.
    AuditEdit { event: String, target: String },
}

/// One placed custom widget folder (not a `k2:` built-in) and its live hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedWidget {
    pub widget: String,
    /// The live (or, for a locked Garden, frozen) bundle hash; empty when
    /// the folder is broken.
    pub hash: String,
}

/// What the pass reads from the files for one Garden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GardenFacts {
    pub id: String,
    pub placed: Vec<PlacedWidget>,
}

/// One candidate and what the pass did with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub garden_id: String,
    pub project_id: String,
    pub reasons: Vec<Reason>,
    /// Added by this run.
    pub listed: bool,
    /// Already on the list before this run.
    pub already: bool,
    /// The person took it off this Garden's list before (Settings, the
    /// card, the twin): the pass never re-adds it.
    pub removed_by_person: bool,
}

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassReport {
    pub gardens: usize,
    pub candidates: Vec<Candidate>,
    /// List rows this run added.
    pub listed: usize,
    /// Made rows this run added.
    pub made: usize,
    /// `imported` History lines this run added.
    pub imported: usize,
}

/// Read the facts the pass needs from the files (no DB lock held): every
/// Garden on the list and its placed custom widget folders.
pub fn gather(files: &ZenFiles) -> Vec<GardenFacts> {
    let mut out = Vec::new();
    for g in files.gardens() {
        let f = ZenFile::Garden(g.id.clone());
        let eff = files.effective(&f, &files.base_for(&f));
        let page = crate::zen::garden_page(&eff.layer, &g.template);
        let mut placed: Vec<PlacedWidget> = Vec::new();
        for w in page["widgets"].as_array().into_iter().flatten() {
            if w["kind"] != schema::CUSTOM_KIND {
                continue;
            }
            let widget = w["widget"].as_str().unwrap_or_default().to_string();
            if widget.is_empty() || authors::is_builtin(&widget) || placed.iter().any(|p| p.widget == widget) {
                continue;
            }
            let hash = files.garden_widget_status(&g.id, &widget).hash().to_string();
            placed.push(PlacedWidget { widget, hash });
        }
        out.push(GardenFacts { id: g.id, placed });
    }
    out
}

/// `agent:<handle>` / `shell:<handle>` → the workspace id. The Access Audit
/// label is the handle (`workspace_address_name`), or the id itself when
/// that lookup failed at write time.
fn handle_map(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id FROM projects")?;
    let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut map = BTreeMap::new();
    for id in &ids {
        if let Ok(h) = crate::workspace_session_handles::workspace_address_name(conn, id) {
            map.entry(h).or_insert_with(|| id.clone());
        }
    }
    for id in ids {
        map.insert(id.clone(), id);
    }
    Ok(map)
}

fn audit_actor(map: &BTreeMap<String, String>, user: &str) -> Option<String> {
    let label = user.strip_prefix("agent:").or_else(|| user.strip_prefix("shell:"))?;
    map.get(label).cloned()
}

/// `ok target=<t> …` → `<t>`. Anything but an `ok` outcome → `None`.
pub fn audit_target(outcome: &str) -> Option<&str> {
    let rest = outcome.strip_prefix("ok target=")?;
    let t = rest.split(' ').next().unwrap_or_default();
    (!t.is_empty()).then_some(t)
}

/// What an Access Audit target names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Garden(String),
    Widget(String),
}

/// `gardens/<id>.toml` → Garden; `widgets/<w>[/…]` → Widget; a bare valid
/// Garden id (`zen-garden-new`) → Garden.
fn parse_target(t: &str) -> Option<Target> {
    if let Some(rest) = t.strip_prefix("gardens/") {
        return rest.strip_suffix(".toml").map(|id| Target::Garden(id.to_string()));
    }
    if let Some(rest) = t.strip_prefix(&format!("{}/", crate::zen::widgets::WIDGETS_DIR)) {
        let w = rest.split('/').next().unwrap_or_default();
        return (!w.is_empty()).then(|| Target::Widget(w.to_string()));
    }
    crate::zen::store::valid_garden_id(t).then(|| Target::Garden(t.to_string()))
}

fn audit_ts(rec: &J) -> Option<i64> {
    let ts = rec["ts"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|t| t.timestamp())
}

fn store_err(e: crate::garden_data::DataError) -> super::AccessError {
    super::AccessError::Store(e.to_string())
}

/// Is this Access Audit line a Zen one History may import?
fn is_zen_event(event: &str) -> bool {
    (event.starts_with("zen-") || event.starts_with("zen.")) && !event.starts_with(super::AUDIT_EVENT)
}

/// Did the person take `project_id` off `garden`'s list (an
/// `access_removed` line via Settings, a card or the twin)? K2's own
/// removals (workspace or Garden delete) don't count. History is kept two
/// years, so a removal older than that is forgotten.
fn removed_by_person(conn: &Connection, garden: &str, project_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM zen_garden_events WHERE garden_id = ?1 AND kind = 'access_removed'
               AND json_extract(detail_json, '$.workspace') = ?2
               AND json_extract(detail_json, '$.via') IN ('settings', 'card', 'twin') LIMIT 1",
            params![garden, project_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn already_imported(conn: &Connection, garden: &str, detail: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM zen_garden_events WHERE garden_id = ?1 AND kind = 'imported' AND detail_json = ?2",
            params![garden, detail],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Run the pass on gathered facts and Access Audit records (oldest first),
/// with `conn` held. One transaction; one Access Audit line per run.
pub fn run_with(conn: &Connection, gardens: &[GardenFacts], audit: &[J], now: i64) -> Result<PassReport> {
    let handles = handle_map(conn)?;
    let mut report = PassReport { gardens: gardens.len(), ..Default::default() };
    let mut approved: Vec<(String, String, String, String)> = Vec::new(); // (id, kind, target, project)
    {
        let mut stmt = conn.prepare(
            "SELECT id, kind, target, made_by_project FROM zen_proposals WHERE status = 'approved' ORDER BY created_at, id",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))? {
            approved.push(row?);
        }
    }

    let tx = conn.unchecked_transaction()?;
    let migration = Actor::Migration;
    for g in gardens {
        let placed: BTreeSet<&str> = g.placed.iter().map(|p| p.widget.as_str()).collect();
        let names = |t: &Target| match t {
            Target::Garden(id) => *id == g.id,
            Target::Widget(w) => placed.contains(w.as_str()),
        };
        let mut cands: BTreeMap<String, BTreeSet<Reason>> = BTreeMap::new();
        for p in &g.placed {
            if p.hash.is_empty() {
                continue;
            }
            if let MadeBy::Agent { project_id } = authors::maker_of(&tx, &p.widget, &p.hash).map_err(store_err)? {
                cands.entry(project_id).or_default().insert(Reason::LiveMaker { widget: p.widget.clone() });
            }
        }
        for (id, kind, target, project) in &approved {
            if (kind == "garden" && *target == g.id) || (kind == "widget" && placed.contains(target.as_str())) {
                cands.entry(project.clone()).or_default().insert(Reason::ApprovedProposal { id: id.clone() });
            }
        }
        for rec in audit {
            let event = rec["event"].as_str().unwrap_or_default();
            let outcome = rec["outcome"].as_str().unwrap_or_default();
            let user = rec["user"].as_str().unwrap_or_default();
            let Some(raw) = audit_target(outcome) else { continue };
            let Some(t) = parse_target(raw) else { continue };
            if !names(&t) {
                continue;
            }
            let actor = audit_actor(&handles, user);
            if EDIT_EVENTS.contains(&event) {
                if let Some(p) = &actor {
                    cands
                        .entry(p.clone())
                        .or_default()
                        .insert(Reason::AuditEdit { event: event.to_string(), target: raw.to_string() });
                }
            }
            if event == NEW_EVENT {
                if let (Some(p), Some(at), Target::Garden(_)) = (&actor, audit_ts(rec), &t) {
                    if workspace_exists(&tx, p)? && record_made(&tx, &g.id, p, at)? {
                        record_event(
                            &tx,
                            On::Garden(&g.id),
                            EventKind::Made,
                            &Actor::Agent(p.clone()),
                            &json!({ "workspace": p, "via": "migration", "source": "access-audit" }),
                            at,
                        )?;
                        report.made += 1;
                    }
                }
            }
            if is_zen_event(event) {
                let at = audit_ts(rec).unwrap_or(now);
                let short: String = outcome.chars().take(IMPORT_OUTCOME_CHARS).collect();
                let detail = json!({ "event": event, "user": user, "outcome": short, "ts": rec["ts"] });
                if !already_imported(&tx, &g.id, &detail.to_string())? {
                    record_event(&tx, On::Garden(&g.id), EventKind::Imported, &migration, &detail, at)?;
                    report.imported += 1;
                }
            }
        }
        for (project, reasons) in cands {
            if !workspace_exists(&tx, &project)? {
                continue;
            }
            let already = super::is_listed(&tx, &g.id, &project)?;
            let removed = !already && removed_by_person(&tx, &g.id, &project)?;
            let listed = GRANDFATHER
                && !already
                && !removed
                && add(&tx, &g.id, &project, &Change::new(Via::Migration, &migration, now).note(AUTO_NOTE))?;
            if listed {
                report.listed += 1;
            }
            report.candidates.push(Candidate {
                garden_id: g.id.clone(),
                project_id: project,
                reasons: reasons.into_iter().collect(),
                listed,
                already,
                removed_by_person: removed,
            });
        }
    }
    tx.commit()?;
    crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
        AUDIT_PASS_EVENT,
        &migration.label(),
        format!(
            "ok gardens={} candidates={} listed={} made={} imported={}",
            report.gardens,
            report.candidates.len(),
            report.listed,
            report.made,
            report.imported
        ),
        "-",
        "-",
        "cli",
    ));
    Ok(report)
}

/// Gather from `files`, read the whole Access Audit, and run with the shared
/// DB. The daemon calls this on every off→on flip.
pub fn run(files: &ZenFiles, now: i64) -> Result<PassReport> {
    let gardens = gather(files);
    let audit = crate::auth_audit::read_all().map_err(super::AccessError::Store)?;
    let db = crate::db::shared();
    let conn = db.lock();
    run_with(&conn, &gardens, &audit, now)
}
