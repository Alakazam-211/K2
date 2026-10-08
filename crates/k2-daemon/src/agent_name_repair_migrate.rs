//! One-shot repair (A8 H2): agents misnamed "AGENT.md" / "ROLE.md".
//!
//! From 0.40.100 until 0.45.1, `k2 agent hire` on a folder that wasn't
//! registered yet sent the persona FILE name as the agent's name (a loop
//! variable shadowed `--name`). `set-name` then wrote it into both name
//! stores: AGENT.md `display_name:` and `projects.name`.
//!
//! This pass finds workspaces where BOTH stores say exactly `AGENT.md` (or
//! both `ROLE.md`) and resets the name to the folder basename — what a
//! fresh hire without `--name` gets. A workspace where only one store has
//! the file name is logged and left alone (someone may have chosen it).
//! Gated by `code_migrations`, so it runs once per DB.

use k2_core::log_debug;

const MIGRATION_ID: &str = "agent-name-persona-filename-repair-v1";

/// The names the old hire bug wrote.
const BAD_NAMES: [&str; 2] = ["AGENT.md", "ROLE.md"];

/// The persona file's `display_name:` for `path`, if it has one.
fn persona_display_name(path: &str) -> Option<String> {
    use k2_core::workspace::agent_identity::{parse_frontmatter, persona_md_in, workspace_agent_path};
    let file = persona_md_in(workspace_agent_path(path));
    let content = std::fs::read_to_string(file).ok()?;
    parse_frontmatter(&content)
        .get("display_name")
        .map(|s| s.trim().to_string())
}

/// Run the repair once. Returns the paths it renamed (tests / logs).
pub fn run_once() -> Vec<String> {
    // Collect under the DB lock, then release it: `set_agent_display_name`
    // takes the shared lock itself.
    let candidates: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if k2_core::db::has_code_migration_applied(&conn, MIGRATION_ID) {
            return Vec::new();
        }
        let mut stmt = match conn.prepare(
            "SELECT path, name FROM projects WHERE name IN ('AGENT.md', 'ROLE.md')",
        ) {
            Ok(s) => s,
            Err(e) => {
                log_debug!("[daemon/boot] {MIGRATION_ID}: SELECT failed: {e}");
                return Vec::new();
            }
        };
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
        match rows {
            Ok(it) => it.flatten().collect(),
            Err(e) => {
                log_debug!("[daemon/boot] {MIGRATION_ID}: query failed: {e}");
                return Vec::new();
            }
        }
    };

    let mut renamed: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for (path, name) in candidates {
        if !BAD_NAMES.contains(&name.as_str()) {
            continue;
        }
        let display = persona_display_name(&path);
        if display.as_deref() != Some(name.as_str()) {
            skipped += 1;
            log_debug!(
                "[daemon/boot] {MIGRATION_ID}: {path} has projects.name {name:?} but persona \
                 display_name {display:?}; left alone"
            );
            continue;
        }
        let basename = std::path::Path::new(path.trim_end_matches('/'))
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "agent".to_string());
        match k2_core::workspace::display::set_agent_display_name(&path, &basename) {
            Ok(()) => {
                log_debug!("[daemon/boot] {MIGRATION_ID}: {path}: {name:?} → {basename:?}");
                renamed.push(path);
            }
            Err(e) => {
                skipped += 1;
                log_debug!("[daemon/boot] {MIGRATION_ID}: {path}: rename to {basename:?} failed: {e}");
            }
        }
    }

    let notes = format!("renamed={} skipped={skipped}", renamed.len());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::mark_code_migration_applied(&conn, MIGRATION_ID, Some(&notes));
    }
    log_debug!("[daemon/boot] {MIGRATION_ID} complete: {notes}");
    renamed
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM code_migrations WHERE id = ?1",
        rusqlite::params![MIGRATION_ID],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registered workspace at a fresh temp folder named `<base>-<uuid>`
    /// with persona `display_name:` = `display` (None = no persona file).
    fn workspace(base: &str, name: &str, display: Option<&str>) -> String {
        let dir = std::env::temp_dir().join(format!("{base}-{}", uuid::Uuid::new_v4()));
        let path = dir.to_string_lossy().into_owned();
        if let Some(d) = display {
            let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&path);
            std::fs::create_dir_all(persona.parent().unwrap()).unwrap();
            std::fs::write(
                &persona,
                format!("---\nname: {base}\ndisplay_name: {d}\n---\n# Persona\n"),
            )
            .unwrap();
        } else {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), name, path],
        )
        .unwrap();
        path
    }

    fn project_name(path: &str) -> String {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT name FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn basename(path: &str) -> String {
        std::path::Path::new(path).file_name().unwrap().to_string_lossy().into_owned()
    }

    /// One test: the repair is a global one-shot keyed in `code_migrations`,
    /// so its cases share a run.
    #[test]
    fn repair_renames_both_store_misnames_once_and_leaves_the_rest() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        reset_for_tests();

        let agent_md = workspace("press", "AGENT.md", Some("AGENT.md"));
        let role_md = workspace("helper", "ROLE.md", Some("ROLE.md"));
        let nora = workspace("nora-ws", "Nora", Some("Nora"));
        let one_store = workspace("half", "AGENT.md", Some("Half Agent"));
        let no_persona = workspace("bare", "AGENT.md", None);

        let renamed = run_once();

        assert_eq!(project_name(&agent_md), basename(&agent_md));
        assert_eq!(persona_display_name(&agent_md).as_deref(), Some(basename(&agent_md).as_str()));
        assert_eq!(project_name(&role_md), basename(&role_md));
        assert_eq!(persona_display_name(&role_md).as_deref(), Some(basename(&role_md).as_str()));
        assert_eq!(project_name(&nora), "Nora", "an owner-chosen name is untouched");
        assert_eq!(project_name(&one_store), "AGENT.md", "only one store matches → left alone");
        assert_eq!(persona_display_name(&one_store).as_deref(), Some("Half Agent"));
        assert_eq!(project_name(&no_persona), "AGENT.md", "no persona file → left alone");
        assert!(renamed.contains(&agent_md) && renamed.contains(&role_md), "{renamed:?}");
        assert!(!renamed.contains(&nora) && !renamed.contains(&one_store));

        // Second boot: marked done, so nothing runs — even a new misname.
        let late = workspace("late", "AGENT.md", Some("AGENT.md"));
        assert!(run_once().is_empty(), "second boot changes nothing");
        assert_eq!(project_name(&late), "AGENT.md");

        for p in [agent_md, role_md, nora, one_store, no_persona, late] {
            std::fs::remove_dir_all(&p).ok();
        }
    }
}
