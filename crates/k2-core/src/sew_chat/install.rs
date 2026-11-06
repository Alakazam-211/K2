//! Which Sew install the chat ids in the main DB belong to.
//!
//! Sew assigns every Space, channel and member id, and a new install (a
//! purge, then an enable) starts with none of them. K2's main DB keeps some
//! of those ids: agents' access to Spaces (`sew_chat_access`) and each
//! agent's delivery cursor (`hub.cursor/<project>` in `sew_chat_settings`,
//! keyed to the agent's member id). On voicetest 11-04 a purge and a fresh
//! install left them all pointing at nothing, and the hub logged
//! `sew deliveries: 404 not_found` every minute.
//!
//! **The stamp.** `sew_chat_settings` key [`INSTALL_KEY`] holds the install
//! id these rows came from. [`note_install`] runs on every grant-hook sweep
//! with the install id Sew reports:
//! - the same install: nothing;
//! - another install: every agent Space other than the K5 Space is stale,
//!   and every hub cursor is dropped;
//! - no stamp yet (rows from a K2 before this one): only the rows last
//!   written before the current install was made (`sew_server.installed_at`)
//!   are stale. A row written after it came from this install.
//!
//! **Stale Spaces.** Sew can't give a Space back its id, and K2 can't make an
//! agent's Space again the way it was first made (its name and owner were
//! the agent's call, and its history is gone). So a stale Space's access rows
//! are moved to a record ([`STALE_PREFIX`]`<space id>`: slug, who had what
//! level) and taken off the agents, audited. `k2 voice repair` and `k2
//! hostchat doctor` list them with the fix: the agent makes the Space again
//! (`k2 chat space create <slug> --name "<Name>"`), then the owner gives
//! the other agents access again.
//!
//! The K5 Space (`k5-admin`) is never stale here: the voice-rooms sweep
//! checks it with Sew and re-links its access rows to the id Sew holds.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::store;
use crate::tool_hook::sew_wire;

/// The install the main DB's chat ids came from.
pub const INSTALL_KEY: &str = "sew.install";
/// A stale Space's record: `sew.stale_space/<space id>`.
pub const STALE_PREFIX: &str = "sew.stale_space/";
/// Who the moves are attributed to.
pub const ACTOR: &str = "system:sew-install";

/// One agent's access to a stale Space, as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleAccess {
    pub project_id: String,
    pub level: String,
    pub approval: String,
}

/// A Space from an earlier Sew install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleSpace {
    pub space_id: String,
    pub slug: String,
    pub access: Vec<StaleAccess>,
    /// The install it came from, when K2 knew it.
    pub install: Option<String>,
    pub since: i64,
    /// Why K2 called it stale.
    pub why: String,
}

/// What [`note_install`] (or [`forget`]) did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    /// The stamp before (`None`: never stamped).
    pub from: Option<String>,
    pub to: Option<String>,
    /// Spaces moved to the stale record now.
    pub stale: Vec<StaleSpace>,
    /// Hub cursors dropped now.
    pub cursors_reset: usize,
}

impl InstallReport {
    pub fn changed_anything(&self) -> bool {
        !self.stale.is_empty() || self.cursors_reset > 0
    }
}

fn db(e: rusqlite::Error) -> String {
    format!("sew install ids: {e}")
}

/// The stamp.
pub fn stamp(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row("SELECT value FROM sew_chat_settings WHERE key = ?1", params![INSTALL_KEY], |r| r.get(0))
        .optional()
        .map_err(db)
}

fn set_stamp(conn: &Connection, install: Option<&str>, now: i64) -> Result<(), String> {
    match install {
        Some(i) => conn.execute(
            "INSERT INTO sew_chat_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
            params![INSTALL_KEY, i, ACTOR, now],
        ),
        None => conn.execute("DELETE FROM sew_chat_settings WHERE key = ?1", params![INSTALL_KEY]),
    }
    .map(|_| ())
    .map_err(db)
}

/// Sew reports install `install` (made at `installed_at`, unix seconds, when
/// K2 knows it). Moves what belongs to an earlier install (module doc) and
/// stamps `install`.
pub fn note_install(conn: &Connection, install: &str, installed_at: Option<i64>, now: i64) -> Result<InstallReport, String> {
    if !sew_wire::is_sew_id(install) {
        return Err(format!("unexpected Sew install id '{install}'"));
    }
    let from = stamp(conn)?;
    let mut rep = InstallReport { from: from.clone(), to: Some(install.to_string()), ..Default::default() };
    if from.as_deref() == Some(install) {
        return Ok(rep);
    }
    let before = match from.as_deref() {
        // Another install: everything K2 held is from it.
        Some(_) => Some(i64::MAX),
        // Never stamped: only what was written before this install began.
        None => installed_at,
    };
    if let Some(cut) = before {
        let why = match from.as_deref() {
            Some(old) => format!("Sew install {old} was replaced by {install}"),
            None => format!("written before Sew install {install} was made"),
        };
        rep.stale = move_stale(conn, cut, from.as_deref(), &why, now)?;
        rep.cursors_reset = drop_cursors(conn, cut)?;
    }
    set_stamp(conn, Some(install), now)?;
    Ok(rep)
}

/// A purge: every agent Space but the K5 Space is gone with the install,
/// every hub cursor too, and the stamp is dropped (the next install stamps
/// itself).
pub fn forget(conn: &Connection, why: &str, now: i64) -> Result<InstallReport, String> {
    let from = stamp(conn)?;
    let mut rep = InstallReport { from: from.clone(), to: None, ..Default::default() };
    rep.stale = move_stale(conn, i64::MAX, from.as_deref(), why, now)?;
    rep.cursors_reset = drop_cursors(conn, i64::MAX)?;
    set_stamp(conn, None, now)?;
    Ok(rep)
}

/// Access rows (not the K5 Space) last written before `cut` → the stale
/// record, audited; the rows are removed (queued posts there cancelled).
fn move_stale(conn: &Connection, cut: i64, install: Option<&str>, why: &str, now: i64) -> Result<Vec<StaleSpace>, String> {
    let rows: Vec<store::Access> = store::list_all_access(conn)?
        .into_iter()
        .filter(|a| !a.space_slug.eq_ignore_ascii_case(sew_wire::ADMIN_SPACE_SLUG) && a.updated_at < cut)
        .collect();
    let mut by_space: std::collections::BTreeMap<String, StaleSpace> = std::collections::BTreeMap::new();
    for a in &rows {
        let e = by_space.entry(a.space_id.clone()).or_insert_with(|| StaleSpace {
            space_id: a.space_id.clone(),
            slug: a.space_slug.clone(),
            access: Vec::new(),
            install: install.map(str::to_string),
            since: now,
            why: why.to_string(),
        });
        e.access.push(StaleAccess {
            project_id: a.project_id.clone(),
            level: a.level.as_str().to_string(),
            approval: a.approval.as_str().to_string(),
        });
    }
    let mut out = Vec::new();
    for (space_id, mut sp) in by_space {
        // A Space already recorded keeps what it had (a second move adds).
        if let Some(prev) = stale_space(conn, &space_id)? {
            for a in prev.access {
                if !sp.access.iter().any(|x| x.project_id == a.project_id) {
                    sp.access.push(a);
                }
            }
            sp.since = prev.since;
        }
        let text = serde_json::to_string(&sp).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sew_chat_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
            params![format!("{STALE_PREFIX}{space_id}"), text, ACTOR, now],
        )
        .map_err(db)?;
        for a in &sp.access {
            store::revoke_access(conn, &a.project_id, &space_id, ACTOR, now)?;
        }
        store::insert_audit(
            conn,
            &store::AuditRow {
                id: super::new_id("aud_"),
                project_id: None,
                actor: ACTOR.into(),
                actor_kind: "system".into(),
                op: "space_stale".into(),
                space_id: Some(space_id.clone()),
                target: Some(sp.slug.clone()),
                detail: Some(serde_json::json!({"why": why, "agents": sp.access.iter().map(|a| &a.project_id).collect::<Vec<_>>()}).to_string()),
                outcome: "ok".into(),
                reason: None,
                created_at: now,
            },
        )?;
        out.push(sp);
    }
    Ok(out)
}

/// Hub cursors last saved before `cut` (they name an old member id).
fn drop_cursors(conn: &Connection, cut: i64) -> Result<usize, String> {
    conn.execute("DELETE FROM sew_chat_settings WHERE substr(key, 1, 11) = 'hub.cursor/' AND updated_at < ?1", params![cut])
        .map_err(db)
}

/// One stale Space's record.
pub fn stale_space(conn: &Connection, space_id: &str) -> Result<Option<StaleSpace>, String> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM sew_chat_settings WHERE key = ?1", params![format!("{STALE_PREFIX}{space_id}")], |r| r.get(0))
        .optional()
        .map_err(db)?;
    raw.map(|t| serde_json::from_str(&t).map_err(|e| format!("stale Space record {space_id}: {e}"))).transpose()
}

/// Every stale Space, by slug.
pub fn stale_spaces(conn: &Connection) -> Result<Vec<StaleSpace>, String> {
    let mut st = conn
        .prepare("SELECT key, value FROM sew_chat_settings WHERE substr(key, 1, ?2) = ?1 ORDER BY key")
        .map_err(db)?;
    let rows = st
        .query_map(params![STALE_PREFIX, STALE_PREFIX.len() as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(db)?;
    let mut out = Vec::new();
    for r in rows {
        let (k, v) = r.map_err(db)?;
        out.push(serde_json::from_str::<StaleSpace>(&v).map_err(|e| format!("stale Space record {k}: {e}"))?);
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug).then_with(|| a.space_id.cmp(&b.space_id)));
    Ok(out)
}

/// Forget a stale Space's record (the owner dealt with it). Returns whether
/// there was one.
pub fn dismiss(conn: &Connection, space_id: &str) -> Result<bool, String> {
    let n = conn
        .execute("DELETE FROM sew_chat_settings WHERE key = ?1", params![format!("{STALE_PREFIX}{space_id}")])
        .map_err(db)?;
    Ok(n > 0)
}

/// The K5 Space's access rows follow the id Sew holds now (after the
/// voice-rooms sweep re-linked it). Returns how many moved.
pub fn relink_k5_access(conn: &Connection, space_id: &str, now: i64) -> Result<usize, String> {
    let rows: Vec<store::Access> = store::list_all_access(conn)?
        .into_iter()
        .filter(|a| a.space_slug.eq_ignore_ascii_case(sew_wire::ADMIN_SPACE_SLUG) && a.space_id != space_id)
        .collect();
    for a in &rows {
        if store::get_access(conn, &a.project_id, space_id)?.is_none() {
            store::set_access(conn, &store::Access { space_id: space_id.to_string(), updated_by: ACTOR.into(), updated_at: now, ..a.clone() })?;
        }
        conn.execute("DELETE FROM sew_chat_access WHERE project_id = ?1 AND space_id = ?2", params![a.project_id, a.space_id])
            .map_err(db)?;
    }
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sew_chat::{ApprovalMode, Level};

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE sew_chat_access (project_id TEXT NOT NULL, space_id TEXT NOT NULL, space_slug TEXT NOT NULL,
               level TEXT NOT NULL, approval TEXT NOT NULL DEFAULT 'none', updated_by TEXT NOT NULL, updated_at INTEGER NOT NULL,
               PRIMARY KEY (project_id, space_id));
             CREATE TABLE sew_chat_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_by TEXT, updated_at INTEGER NOT NULL);
             CREATE TABLE sew_chat_outbox (id TEXT PRIMARY KEY, project_id TEXT, space_id TEXT, status TEXT);
             CREATE TABLE sew_chat_audit (id TEXT PRIMARY KEY, project_id TEXT, actor TEXT NOT NULL, actor_kind TEXT NOT NULL,
               op TEXT NOT NULL, space_id TEXT, target TEXT, detail TEXT, outcome TEXT NOT NULL, reason TEXT, created_at INTEGER NOT NULL);",
        )
        .unwrap();
        c
    }

    fn access(c: &Connection, project: &str, space: &str, slug: &str, at: i64) {
        store::set_access(
            c,
            &store::Access {
                project_id: project.into(),
                space_id: space.into(),
                space_slug: slug.into(),
                level: Level::Manage,
                approval: ApprovalMode::None,
                updated_by: "user:owner".into(),
                updated_at: at,
            },
        )
        .unwrap();
    }

    fn cursor(c: &Connection, project: &str, at: i64) {
        c.execute(
            "INSERT INTO sew_chat_settings (key, value, updated_by, updated_at) VALUES (?1, '{}', 'hub', ?2)",
            params![format!("hub.cursor/{project}"), at],
        )
        .unwrap();
    }

    fn slugs(c: &Connection) -> Vec<(String, String)> {
        store::list_all_access(c).unwrap().into_iter().map(|a| (a.project_id, a.space_slug)).collect()
    }

    #[test]
    fn an_unstamped_box_moves_only_what_predates_the_install() {
        let c = conn();
        access(&c, "p1", "501", "u1", 100);
        access(&c, "p2", "502", "pathbs", 100);
        access(&c, "p3", "601", "fresh", 300);
        access(&c, "p1", "700", "k5-admin", 100);
        cursor(&c, "p1", 100);
        cursor(&c, "p3", 300);
        let r = note_install(&c, "2002", Some(200), 400).unwrap();
        assert_eq!(r.from, None);
        assert_eq!(r.stale.iter().map(|s| s.slug.as_str()).collect::<Vec<_>>(), ["u1", "pathbs"]);
        assert_eq!(r.cursors_reset, 1, "the cursor saved before the install");
        assert_eq!(slugs(&c), [("p1".to_string(), "k5-admin".to_string()), ("p3".to_string(), "fresh".to_string())]);
        let rec = stale_space(&c, "501").unwrap().expect("record");
        assert_eq!(rec.access, [StaleAccess { project_id: "p1".into(), level: "manage".into(), approval: "none".into() }]);
        assert_eq!(stamp(&c).unwrap().as_deref(), Some("2002"));
        // The same install again does nothing.
        assert_eq!(note_install(&c, "2002", Some(200), 500).unwrap().changed_anything(), false);
        assert_eq!(stale_spaces(&c).unwrap().len(), 2);
        let audit: i64 = c.query_row("SELECT COUNT(*) FROM sew_chat_audit WHERE op = 'space_stale'", [], |r| r.get(0)).unwrap();
        assert_eq!(audit, 2);
    }

    #[test]
    fn a_new_install_moves_every_agent_space_and_cursor_and_k5_follows_its_id() {
        let c = conn();
        note_install(&c, "1001", None, 10).unwrap();
        access(&c, "p1", "501", "u1", 900);
        access(&c, "p1", "700", "k5-admin", 900);
        access(&c, "p2", "700", "k5-admin", 900);
        cursor(&c, "p1", 900);
        let r = note_install(&c, "1002", Some(10), 1000).unwrap();
        assert_eq!((r.from.as_deref(), r.stale.len(), r.cursors_reset), (Some("1001"), 1, 1));
        assert_eq!(r.stale[0].install.as_deref(), Some("1001"));
        assert_eq!(relink_k5_access(&c, "800", 1000).unwrap(), 2);
        let k5: Vec<String> = store::list_all_access(&c).unwrap().into_iter().map(|a| a.space_id).collect();
        assert_eq!(k5, ["800", "800"]);
        assert_eq!(relink_k5_access(&c, "800", 1001).unwrap(), 0, "twice is a no-op");
        assert!(dismiss(&c, "501").unwrap());
        assert!(stale_spaces(&c).unwrap().is_empty());
    }

    #[test]
    fn a_purge_forgets_the_stamp() {
        let c = conn();
        note_install(&c, "1001", None, 10).unwrap();
        access(&c, "p1", "501", "u1", 900);
        let r = forget(&c, "hostchat purge", 1000).unwrap();
        assert_eq!(r.stale.len(), 1);
        assert_eq!(stamp(&c).unwrap(), None);
        assert!(note_install(&c, "not an id", None, 1).is_err(), "not a Sew id");
    }
}
