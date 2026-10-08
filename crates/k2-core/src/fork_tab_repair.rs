//! One-time repair for fork tabs opened before 0.45.1 (TR20).
//!
//! A tab opened from Chat History with `--resume S --fork-session`
//! registered `S`, the SOURCE workspace's conversation, as its own id.
//! So workspace B's `b/1` Thread and workspace A's Thread were one list,
//! and B's handle row (and any retired names) keyed on an id A owns.
//! From 0.45.1 a fork is a self-minting harness: no id until the
//! transcript follower adopts the fork's new one. This pass moves the
//! fork rows that already exist onto that path:
//!
//! - B's handle row (and aliases) re-key `S` → the pane key.
//! - B's docs (stored `to`/`from` is a `b/…` address) split out of `S`
//!   into the pane key. A's docs are never moved; their seqs stay.
//! - `S`'s catalog row goes back to A.
//! - B's tab row forgets `S` (adopt-pending): the follower stamps the
//!   fork's own id on its next spawn, and 0.45.0's adoption moves the
//!   pane-keyed handle and Thread onto it.
//!
//! Only rows whose saved argv carries `--fork-session` are touched.
//! Gated by `code_migrations`; each re-key is logged.

use rusqlite::{params, Connection};

use crate::overlay;
use crate::workspace_session_handles as handles;

pub const MIGRATION_ID: &str = "0.45.1-fork-tab-own-conversation";

/// One repaired fork tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkRepair {
    pub project_id: String,
    pub pane_group_id: String,
    /// The source conversation the tab had claimed.
    pub source_conversation: String,
    /// The workspace that owns `source_conversation`.
    pub owner_project_id: String,
    pub thread_moved: usize,
    pub chatter_moved: usize,
}

/// Fork tab rows that claim another workspace's conversation id.
fn forked_rows(conn: &Connection) -> Result<Vec<(String, String, String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.project_id, t.pane_group_id, t.session_id, \
                    COALESCE( \
                      (SELECT w.project_id FROM workspace_sessions w \
                        WHERE w.session_id = t.session_id AND w.project_id != t.project_id LIMIT 1), \
                      (SELECT o.project_id FROM workspace_tab_sessions o \
                        WHERE o.session_id = t.session_id AND o.project_id != t.project_id \
                          AND (o.args_json IS NULL OR o.args_json NOT LIKE '%--fork-session%') LIMIT 1)) \
             FROM workspace_tab_sessions t \
             WHERE t.session_id IS NOT NULL AND TRIM(t.session_id) != '' \
               AND t.args_json LIKE '%--fork-session%'",
        )
        .map_err(|e| format!("fork repair select: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| format!("fork repair query: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (project_id, pane, sid, owner) = row.map_err(|e| e.to_string())?;
        if let Some(owner) = owner {
            out.push((project_id, pane, sid, owner));
        }
    }
    Ok(out)
}

/// Repair every fork tab row (not gated; [`run_once`] gates it).
pub fn repair(conn: &Connection) -> Result<Vec<ForkRepair>, String> {
    let mut out = Vec::new();
    for (project_id, pane, source, owner) in forked_rows(conn)? {
        let pane_key = handles::normalize_pane_key(&pane).to_string();
        if pane_key.is_empty() || pane_key == source {
            continue;
        }
        // Handle row + retired names follow the tab onto its pane key.
        if handles::get(conn, &project_id, &source)?.is_some() {
            if handles::get(conn, &project_id, &pane_key)?.is_some() {
                conn.execute(
                    "DELETE FROM workspace_session_handles WHERE project_id = ?1 AND conversation_key = ?2",
                    params![project_id, source],
                )
                .map_err(|e| format!("fork repair drop dup handle: {e}"))?;
            } else {
                handles::rekey_conversation(conn, &project_id, &source, &pane_key)?;
            }
        } else {
            conn.execute(
                "UPDATE workspace_session_handle_aliases SET conversation_key = ?3 \
                 WHERE project_id = ?1 AND conversation_key = ?2",
                params![project_id, source, pane_key],
            )
            .map_err(|e| format!("fork repair aliases: {e}"))?;
        }
        // Only B's docs leave the shared Thread.
        let ws = handles::workspace_address_name(conn, &project_id)?;
        let prefix = format!("{ws}/");
        let take = |doc: &overlay::OverlayDoc| {
            doc.to.as_deref().is_some_and(|t| t.starts_with(&prefix)) || doc.from.starts_with(&prefix)
        };
        let moved = overlay::split_conversation(conn, &source, &pane_key, &project_id, &take)?;
        conn.execute(
            "UPDATE overlay_conversations SET project_id = ?2 WHERE conversation_id = ?1",
            params![source, owner],
        )
        .map_err(|e| format!("fork repair catalog owner: {e}"))?;
        // Adopt-pending: the follower stamps the fork's own id next spawn.
        conn.execute(
            "UPDATE workspace_tab_sessions SET session_id = NULL \
             WHERE project_id = ?1 AND pane_group_id = ?2",
            params![project_id, pane],
        )
        .map_err(|e| format!("fork repair tab row: {e}"))?;
        let repair = ForkRepair {
            project_id: project_id.clone(),
            pane_group_id: pane.clone(),
            source_conversation: source.clone(),
            owner_project_id: owner.clone(),
            thread_moved: moved.as_ref().map(|m| m.thread_moved).unwrap_or(0),
            chatter_moved: moved.as_ref().map(|m| m.chatter_moved).unwrap_or(0),
        };
        crate::log_debug!(
            "[core/fork-repair] tab {pane} in {project_id}: forgot {source} (owned by {owner}); \
             handle now pane-keyed; moved {} Thread / {} Chatter rows",
            repair.thread_moved,
            repair.chatter_moved
        );
        out.push(repair);
    }
    Ok(out)
}

/// Boot pass, once per DB.
pub fn run_once() {
    let db = crate::db::shared();
    let conn = db.lock();
    if crate::db::has_code_migration_applied(&conn, MIGRATION_ID) {
        return;
    }
    match repair(&conn) {
        Ok(done) => {
            let notes = format!("fork_tabs_repaired={}", done.len());
            crate::db::mark_code_migration_applied(&conn, MIGRATION_ID, Some(&notes));
        }
        Err(e) => {
            // Not marked: the next boot tries again.
            crate::log_debug!("[core/fork-repair] failed, will retry next boot: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> std::sync::Arc<parking_lot::ReentrantMutex<Connection>> {
        crate::db::init_for_tests()
    }

    fn seed_project(handle: &str) -> String {
        let dbh = conn();
        let c = dbh.lock();
        let id = uuid::Uuid::new_v4().to_string();
        c.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            params![id, handle, format!("/tmp/fork-repair-{handle}-{id}")],
        )
        .expect("seed project");
        id
    }

    fn bodies(conv: &str) -> Vec<String> {
        overlay::read_thread(conv, 0)
            .expect("read thread")
            .into_iter()
            .map(|i| i.doc.body.expect("text body"))
            .collect()
    }

    fn pin(project_id: &str, session_id: &str) {
        let dbh = conn();
        let c = dbh.lock();
        crate::db::schema::WorkspaceSession::upsert(
            &c,
            &format!("ws-{project_id}"),
            project_id,
            None,
            Some(session_id),
            "claude",
            "system",
            "running",
        )
        .expect("pin");
    }

    fn tab(project_id: &str, pane: &str, session_id: &str, args: serde_json::Value) {
        let dbh = conn();
        let c = dbh.lock();
        c.execute(
            "INSERT INTO workspace_tab_sessions \
             (project_id, pane_group_id, agent_name, session_id, command, args_json, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', ?5, unixepoch())",
            params![project_id, pane, format!("tab-{pane}"), session_id, args.to_string()],
        )
        .expect("tab row");
    }

    /// TR20/TR21: a fork tab in B that claimed A's pinned conversation S.
    /// The repair gives B's tab its pane key: B's `b/1` docs move there,
    /// A's docs stay under S with their seqs, B's handle row follows, and
    /// the tab row forgets S. A's Chats name is never touched.
    #[test]
    fn repair_splits_a_fork_tab_off_the_source_conversation() {
        let tag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let a_ws = format!("fra{tag}");
        let b_ws = format!("frb{tag}");
        let a = seed_project(&a_ws);
        let b = seed_project(&b_ws);
        let source = uuid::Uuid::new_v4().to_string();
        let pane = format!("pane-{tag}");
        let b_addr = format!("{b_ws}/1");
        pin(&a, &source);
        tab(&b, &pane, &source, serde_json::json!(["--resume", source, "--fork-session"]));
        {
            let dbh = conn();
            let c = dbh.lock();
            handles::allocate_ordinal(&c, &b, &source).expect("B ordinal on S");
            c.execute(
                "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
                 VALUES ('claude', ?1, 'A main', 0, unixepoch())",
                params![source],
            )
            .expect("A's Chats name");
            overlay::post_thread(&c, &source, &a, "rosson", &a_ws, "a-1", "compose").expect("a-1");
            overlay::post_thread(&c, &source, &b, "rosson", &b_addr, "b-1", "compose").expect("b-1");
            overlay::post_thread(&c, &source, &a, &a_ws, &a_ws, "a-2", "thread").expect("a-2");
            overlay::post_thread(&c, &source, &b, &b_addr, &b_addr, "b-2", "thread").expect("b-2");
        }
        let a_seqs_before: Vec<(i64, String)> = overlay::read_thread(&source, 0)
            .expect("read")
            .into_iter()
            .filter(|i| i.doc.body.as_deref().is_some_and(|b| b.starts_with("a-")))
            .map(|i| (i.seq, i.id))
            .collect();

        let done = {
            let dbh = conn();
            let c = dbh.lock();
            repair(&c).expect("repair")
        };
        let mine: Vec<&ForkRepair> = done.iter().filter(|r| r.project_id == b).collect();
        assert_eq!(mine.len(), 1, "{done:?}");
        assert_eq!(mine[0].source_conversation, source);
        assert_eq!(mine[0].owner_project_id, a);
        assert_eq!(mine[0].thread_moved, 2);

        assert_eq!(bodies(&pane), ["b-1", "b-2"], "B's docs on its pane key");
        assert_eq!(bodies(&source), ["a-1", "a-2"], "A's docs stay");
        let a_seqs_after: Vec<(i64, String)> = overlay::read_thread(&source, 0)
            .expect("read")
            .into_iter()
            .map(|i| (i.seq, i.id))
            .collect();
        assert_eq!(a_seqs_after, a_seqs_before, "A's seqs untouched");

        let dbh = conn();
        let c = dbh.lock();
        assert_eq!(
            handles::get(&c, &b, &pane).expect("get").map(|r| r.ordinal),
            Some(1),
            "B's ordinal follows the tab"
        );
        assert!(handles::get(&c, &b, &source).expect("get").is_none());
        let tab_sid: Option<String> = c
            .query_row(
                "SELECT session_id FROM workspace_tab_sessions WHERE project_id = ?1 AND pane_group_id = ?2",
                params![b, pane],
                |r| r.get(0),
            )
            .expect("tab row");
        assert_eq!(tab_sid, None, "adopt-pending");
        let owner: String = c
            .query_row(
                "SELECT project_id FROM overlay_conversations WHERE conversation_id = ?1",
                params![source],
                |r| r.get(0),
            )
            .expect("catalog");
        assert_eq!(owner, a, "S's catalog row back with A");
        assert_eq!(
            handles::custom_name_for_session_id(&c, &source).expect("name").as_deref(),
            Some("A main"),
            "A's Chats name untouched"
        );
        assert_eq!(
            handles::resolve_for_delivery(&c, &b, "1").expect("b/1").conversation_key,
            pane,
            "b/1 now reaches the fork tab's own key"
        );
        drop(c);
        let again = {
            let dbh = conn();
            let c = dbh.lock();
            repair(&c).expect("repair again")
        };
        assert!(again.iter().all(|r| r.project_id != b), "a second pass finds nothing: {again:?}");
    }

    /// A same-conversation `--resume` (no fork) is never touched.
    #[test]
    fn repair_leaves_plain_resume_tabs_alone() {
        let tag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let a = seed_project(&format!("frc{tag}"));
        let b = seed_project(&format!("frd{tag}"));
        let source = uuid::Uuid::new_v4().to_string();
        let pane = format!("pane-plain-{tag}");
        pin(&a, &source);
        tab(&b, &pane, &source, serde_json::json!(["--resume", source]));
        let done = {
            let dbh = conn();
            let c = dbh.lock();
            repair(&c).expect("repair")
        };
        assert!(done.iter().all(|r| r.project_id != b), "{done:?}");
    }
}
