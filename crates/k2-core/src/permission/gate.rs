//! The two switches (PRD §2; vs-live C1, C8). Owned by lane B after P0.
//!
//! - **Global**, "Permission cards" (Beta): the owner-only server setting
//!   `permissionCards` (`AppSettings::permission_cards`), or
//!   `K2_PERMISSION_CARDS=1` in the daemon's environment. Default OFF.
//! - **Per agent**, "Can ask for permission cards":
//!   `projects.agents_can_request_permissions` (migration 0218). Default
//!   OFF. Read here with a dedicated query; it is deliberately **not** a
//!   field of `Project`, which serializes every switch column (C8).
//!
//! An agent may ask only when both are on: [`enabled_for`]. With the global
//! switch off, the route tables skip `/cli/permission/` (C1, lane B), so
//! the family answers exactly as an unknown route does today.
//!
//! P0: the reads exist; nothing calls them, and nothing syncs the setting
//! into [`set_server_switch`] yet (lane B wires boot + settings updates).

use std::sync::atomic::{AtomicBool, Ordering};

static SERVER_SETTING: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `permissionCards` setting into this process (boot and
/// every settings update/reset — lane B).
pub fn set_server_switch(on: bool) {
    SERVER_SETTING.store(on, Ordering::Relaxed);
}

/// The env force-on: `K2_PERMISSION_CARDS` = `1`/`true`/`yes`/`on`.
fn env_forced_on() -> bool {
    std::env::var(super::ENV_FORCE_ON)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// The global switch: env force-on OR the persisted setting. Default OFF.
pub fn server_on() -> bool {
    env_forced_on() || SERVER_SETTING.load(Ordering::Relaxed)
}

/// The per-agent switch for one workspace id. Unknown workspace, a missing
/// column (a database before 0218) or any read error → `false` (fail
/// closed).
pub fn agent_switch_on(workspace_id: &str) -> bool {
    let Some(db) = crate::db::try_shared() else { return false };
    let conn = db.lock();
    conn.query_row(
        "SELECT agents_can_request_permissions FROM projects WHERE id = ?1",
        rusqlite::params![workspace_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// Both switches on for this workspace: the one rule every surface calls
/// (routes, the refusal wrapper, the skill writer, the CLI probe).
pub fn enabled_for(workspace_id: &str) -> bool {
    server_on() && agent_switch_on(workspace_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_switch_defaults_off_and_env_or_setting_turns_it_on() {
        let _env = crate::test_env::lock();
        let _unset = crate::test_env::EnvVar::remove(super::super::ENV_FORCE_ON);
        set_server_switch(false);
        assert!(!server_on(), "the global switch must default OFF");
        {
            let _on = crate::test_env::EnvVar::set(super::super::ENV_FORCE_ON, "1");
            assert!(server_on());
        }
        {
            let _off = crate::test_env::EnvVar::set(super::super::ENV_FORCE_ON, "off");
            assert!(!server_on());
        }
        set_server_switch(true);
        assert!(server_on(), "the setting turns it on");
        set_server_switch(false);
        assert!(!server_on());
    }

    #[test]
    fn per_agent_switch_defaults_off_and_needs_both() {
        let _env = crate::test_env::lock();
        let _unset = crate::test_env::EnvVar::remove(super::super::ENV_FORCE_ON);
        let _ = crate::db::init_for_tests();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, 'pc-gate', ?2)",
                rusqlite::params![id, format!("/tmp/pc-gate-{id}")],
            )
            .expect("insert project");
        }
        assert!(!agent_switch_on(&id), "0218 default is OFF");
        assert!(!agent_switch_on("no-such-workspace"), "unknown workspace fails closed");
        set_server_switch(true);
        assert!(!enabled_for(&id), "global on, agent off → off");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE projects SET agents_can_request_permissions = 1 WHERE id = ?1", rusqlite::params![id])
                .expect("set column");
        }
        assert!(agent_switch_on(&id));
        assert!(enabled_for(&id), "both on → on");
        set_server_switch(false);
        assert!(!enabled_for(&id), "global off, agent on → off");
    }
}
