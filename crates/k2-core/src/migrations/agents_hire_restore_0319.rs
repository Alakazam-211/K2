//! `agents_hire_restore_0319` — hiring back for the workspaces that had it
//! (Rosson, 0.46.16). The second half of migration 0319.
//!
//! Before 0.46.12 nothing gated an agent's own `k2 agent hire`. Migration
//! 0299 made "Let this agent hire agents" its own switch and turned it on
//! only where the old "hire and manage agents" switch was on, so an agent
//! that had hired without that switch lost hiring (`hire_off`).
//!
//! Migration 0319 records the candidates in `agents_hire_restore`: every
//! workspace created before 0299 ran on this database that has the switch
//! off, minus those a person answered an `agents.hire` / `agents.hire-off`
//! card for. A person's set through `POST /cli/agent-access/set {toggle:
//! "agents_hire"}` (Settings, the CLI, and a card's Allow) is written only to
//! the Access Audit file, which SQL can't read, so this step does the rest
//! once at daemon boot: a candidate the audit names stays off
//! (`kept_off_explicit`); the others get the switch on (`restored`, or
//! `already_on` when it was on by then). Each row keeps its outcome, and
//! the `code_migrations` id makes every later boot a no-op.
//!
//! The audit names a workspace by its address (`ok <handle>
//! agents_hire=<value>`), so a candidate counts as set explicitly when any
//! of its names matches: its id, its handle, its name or its address now. A
//! workspace renamed after such a set, under a name nothing else has, is
//! the one case this can miss; the audit also keeps only one rotated
//! generation, and the card trail (in SQL, read by 0319) covers what that
//! rotation dropped for cards.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};

/// The `code_migrations` id (and 0319's own name).
pub const MIGRATION_ID: &str = "0319_agents_hire_restore";

/// The Access Audit event for a workspace this step turned hiring on for.
pub const AUDIT_EVENT: &str = "agents-hire-restore";

/// What one run decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `(workspace id, address)` turned on.
    pub restored: Vec<(String, String)>,
    /// Workspace ids left off: a person set the switch (the audit says so).
    pub kept_off: Vec<String>,
    /// Candidates already on (or gone) by the time this ran.
    pub unchanged: usize,
}

/// The workspace names the Access Audit says a person set "Let this agent
/// hire agents" for: `agent-access-set` lines whose outcome is `ok <name>
/// agents_hire=true|false` (a refusal changed nothing, so it doesn't
/// count).
pub fn explicit_hire_sets(lines: &[serde_json::Value]) -> HashSet<String> {
    let mut out = HashSet::new();
    for l in lines {
        if l.get("event").and_then(|e| e.as_str()) != Some("agent-access-set") {
            continue;
        }
        let Some(o) = l.get("outcome").and_then(|o| o.as_str()) else { continue };
        let Some(rest) = o.strip_prefix("ok ") else { continue };
        for tail in [" agents_hire=true", " agents_hire=false"] {
            if let Some(name) = rest.strip_suffix(tail) {
                out.insert(name.trim().to_ascii_lowercase());
            }
        }
    }
    out
}

/// Decide every undecided candidate 0319 recorded. `explicit` is
/// [`explicit_hire_sets`] of the whole Access Audit. One transaction: a
/// failure leaves every row undecided for the next boot.
pub fn run(conn: &Connection, explicit: &HashSet<String>) -> rusqlite::Result<Outcome> {
    let mut out = Outcome::default();
    let has_table: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'agents_hire_restore'", [], |r| r.get(0))?;
    if !has_table {
        return Ok(out);
    }
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT workspace_id FROM agents_hire_restore WHERE outcome IS NULL ORDER BY workspace_id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    conn.execute_batch("BEGIN;")?;
    let result = (|| {
        for id in ids {
            let row: Option<(Option<String>, String)> = conn
                .query_row("SELECT handle, name FROM projects WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let decided = match row {
                None => {
                    out.unchanged += 1;
                    "gone"
                }
                Some((handle, name)) => {
                    let address = crate::workspace_session_handles::workspace_address_name(conn, &id).unwrap_or_else(|_| id.clone());
                    let names = [Some(id.clone()), handle, Some(name), Some(address.clone())];
                    let set_by_person =
                        names.iter().flatten().any(|n| !n.trim().is_empty() && explicit.contains(&n.trim().to_ascii_lowercase()));
                    if set_by_person {
                        out.kept_off.push(id.clone());
                        "kept_off_explicit"
                    } else if conn.execute(
                        "UPDATE projects SET agents_can_hire_agents = 1 WHERE id = ?1 AND agents_can_hire_agents = 0",
                        params![id],
                    )? == 1
                    {
                        out.restored.push((id.clone(), address));
                        "restored"
                    } else {
                        out.unchanged += 1;
                        "already_on"
                    }
                }
            };
            conn.execute(
                "UPDATE agents_hire_restore SET outcome = ?2, decided_at = unixepoch() WHERE workspace_id = ?1",
                params![id, decided],
            )?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(out)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A database as 0.46.12–0.46.15 left it: 0299 ran when `old-*` already
    /// existed; `new-*` came after. Then 0.46.16's migrations, 0319 too.
    fn upgraded_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::run_migrations_through(&conn, Some("0311_postal_friends")).unwrap();
        // The files 0299 found, and the ones made after it (created_at is
        // set by hand: everything here runs in the same second).
        conn.execute("UPDATE _migrations SET applied_at = 2000000 WHERE name = '0299_agents_hire'", []).unwrap();
        for (id, handle, created) in [
            ("old-quiet", "old-quiet", 1_000_000),
            ("old-person-off", "old-person-off", 1_000_000),
            ("old-card-denied", "old-card-denied", 1_000_000),
            ("old-on", "old-on", 1_000_000),
            ("new-after", "new-after", 3_000_000),
        ] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle, created_at) VALUES (?1, ?2, ?3, ?2, ?4)",
                params![id, handle, format!("/t/{id}"), created],
            )
            .unwrap();
        }
        conn.execute("UPDATE projects SET agents_can_hire_agents = 1 WHERE id = 'old-on'", []).unwrap();
        // A person answered a card about hiring for one of them.
        conn.execute(
            "INSERT INTO permission_requests (id, workspace_id, action, params_hash, text_hash, why, state, floor, created_at, expires_at) \
             VALUES ('w_1', 'old-card-denied', 'agents.hire', 'h', 't', 'hire a helper', 'denied', 'admin', 1, 2)",
            [],
        )
        .unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn
    }

    fn hire_on(conn: &Connection, id: &str) -> bool {
        conn.query_row("SELECT agents_can_hire_agents FROM projects WHERE id = ?1", params![id], |r| r.get::<_, i64>(0)).unwrap() == 1
    }

    fn outcome_of(conn: &Connection, id: &str) -> Option<Option<String>> {
        conn.query_row("SELECT outcome FROM agents_hire_restore WHERE workspace_id = ?1", params![id], |r| r.get(0)).optional().unwrap()
    }

    #[test]
    fn the_audit_names_only_a_persons_set_of_the_hire_switch() {
        let lines = vec![
            json!({"event": "agent-access-set", "outcome": "ok Ops agents_hire=false"}),
            json!({"event": "agent-access-set", "outcome": "ok billing agents_hire=true"}),
            json!({"event": "agent-access-set", "outcome": "refused:forbidden"}),
            json!({"event": "agent-access-set", "outcome": "ok sales agents_manage=false"}),
            json!({"event": "login", "outcome": "ok other agents_hire=false"}),
        ];
        let got = explicit_hire_sets(&lines);
        let want: HashSet<String> = ["ops", "billing"].into_iter().map(String::from).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn old_workspaces_get_hiring_back_unless_a_person_set_it_and_it_runs_once() {
        let conn = upgraded_db();
        // 0319's candidates: off and older than 0299, no answered card.
        assert_eq!(outcome_of(&conn, "old-quiet"), Some(None), "an old workspace with the switch off is a candidate");
        assert_eq!(outcome_of(&conn, "old-person-off"), Some(None));
        assert_eq!(outcome_of(&conn, "old-card-denied"), None, "a person answered a card: not a candidate");
        assert_eq!(outcome_of(&conn, "old-on"), None, "on already: not a candidate");
        assert_eq!(outcome_of(&conn, "new-after"), None, "made after 0299: not a candidate");

        // A person turned it off for old-person-off in Settings (the audit).
        let explicit = explicit_hire_sets(&[json!({"event": "agent-access-set", "outcome": "ok old-person-off agents_hire=false"})]);
        let out = run(&conn, &explicit).unwrap();
        assert_eq!(out.restored, vec![("old-quiet".to_string(), "old-quiet".to_string())], "{out:?}");
        assert_eq!(out.kept_off, vec!["old-person-off".to_string()], "{out:?}");
        assert!(hire_on(&conn, "old-quiet"), "the old workspace hires again");
        assert!(!hire_on(&conn, "old-person-off"), "a person's off stays off");
        assert!(!hire_on(&conn, "old-card-denied"), "a person's card answer stands");
        assert!(!hire_on(&conn, "new-after"), "a workspace made after 0299 stays off");
        assert_eq!(outcome_of(&conn, "old-quiet"), Some(Some("restored".to_string())));
        assert_eq!(outcome_of(&conn, "old-person-off"), Some(Some("kept_off_explicit".to_string())));

        // Once: a person turns old-quiet off again; nothing turns it back on,
        // and the migration itself doesn't run again either.
        conn.execute("UPDATE projects SET agents_can_hire_agents = 0 WHERE id = 'old-quiet'", []).unwrap();
        let again = run(&conn, &HashSet::new()).unwrap();
        assert_eq!(again, Outcome::default(), "every candidate is decided");
        assert!(!hire_on(&conn, "old-quiet"));
        crate::db::run_migrations(&conn).unwrap();
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM agents_hire_restore", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2, "0319 applies once");
        assert_eq!(run(&conn, &HashSet::new()).unwrap(), Outcome::default());
        assert!(!hire_on(&conn, "old-quiet"));
    }

    #[test]
    fn a_fresh_install_has_no_candidates() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('fresh', 'Fresh', '/t/fresh')", []).unwrap();
        assert_eq!(run(&conn, &HashSet::new()).unwrap(), Outcome::default());
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM agents_hire_restore", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        let on: i64 = conn.query_row("SELECT agents_can_hire_agents FROM projects WHERE id = 'fresh'", [], |r| r.get(0)).unwrap();
        assert_eq!(on, 0, "new workspaces stay off by default");
    }
}
