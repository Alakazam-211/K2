//! The switch and the hide-list (`desktop_settings`, migration 0281).
//!
//! No row means off. Only the daemon's Owner-floor routes write here
//! (`/cli/desktop/enable|disable|policy`); `AppSettings` never mirrors it,
//! so an Admin's `/cli/settings/update` can't reach it (S1, vs-live R8).

use rusqlite::{params, Connection, OptionalExtension};

/// The password apps a viewer never sees on macOS by default (Q6, R6):
/// Passwords, Keychain Access, 1Password (7 and 8), Bitwarden.
pub const DEFAULT_HIDE_APPS: &[&str] = &[
    "com.apple.Passwords",
    "com.apple.keychainaccess",
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.bitwarden.desktop",
];

/// The longest hide-list and the longest bundle id.
pub const MAX_HIDE_APPS: usize = 64;
pub const MAX_BUNDLE_ID: usize = 255;

/// One desktop's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub hide_apps: Vec<String>,
    pub updated_at: i64,
    pub updated_by: String,
}

impl Settings {
    fn default_off() -> Self {
        Settings {
            enabled: false,
            hide_apps: DEFAULT_HIDE_APPS.iter().map(|s| s.to_string()).collect(),
            updated_at: 0,
            updated_by: String::new(),
        }
    }
}

/// Read a desktop's settings (off with the default list when no row).
pub fn get(conn: &Connection, desktop: &str) -> Result<Settings, String> {
    let row: Option<(i64, Option<String>, i64, String)> = conn
        .query_row(
            "SELECT enabled, hide_apps_json, updated_at, updated_by FROM desktop_settings WHERE desktop_id = ?1",
            params![desktop],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| format!("desktop_settings read: {e}"))?;
    let Some((enabled, hide, at, by)) = row else {
        return Ok(Settings::default_off());
    };
    let hide_apps = match hide {
        None => Settings::default_off().hide_apps,
        Some(j) => serde_json::from_str::<Vec<String>>(&j).map_err(|e| format!("desktop_settings hide_apps_json: {e}"))?,
    };
    Ok(Settings { enabled: enabled != 0, hide_apps, updated_at: at, updated_by: by })
}

/// Turn the switch on or off. Returns whether it changed.
pub fn set_enabled(conn: &Connection, desktop: &str, enabled: bool, by: &str) -> Result<bool, String> {
    let before = get(conn, desktop)?.enabled;
    conn.execute(
        "INSERT INTO desktop_settings (desktop_id, enabled, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(desktop_id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        params![desktop, enabled as i64, super::now_secs(), by],
    )
    .map_err(|e| format!("desktop_settings write: {e}"))?;
    Ok(before != enabled)
}

/// Check and normalize a hide-list: trimmed, de-duplicated, each a bundle
/// id (`[A-Za-z0-9.-]`, at least one dot).
pub fn clean_hide_apps(raw: &[String]) -> Result<Vec<String>, String> {
    if raw.len() > MAX_HIDE_APPS {
        return Err(format!("at most {MAX_HIDE_APPS} apps in the hide-list"));
    }
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        let id = r.trim();
        let ok = !id.is_empty()
            && id.len() <= MAX_BUNDLE_ID
            && id.contains('.')
            && !id.starts_with('.')
            && !id.ends_with('.')
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
        if !ok {
            return Err(format!("'{id}' isn't a bundle id (like com.apple.Passwords)"));
        }
        if !out.iter().any(|x| x == id) {
            out.push(id.to_string());
        }
    }
    Ok(out)
}

/// Replace the hide-list. Returns the stored list.
pub fn set_hide_apps(conn: &Connection, desktop: &str, raw: &[String], by: &str) -> Result<Vec<String>, String> {
    let list = clean_hide_apps(raw)?;
    let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO desktop_settings (desktop_id, enabled, hide_apps_json, updated_at, updated_by) VALUES (?1, 0, ?2, ?3, ?4) \
         ON CONFLICT(desktop_id) DO UPDATE SET hide_apps_json = excluded.hide_apps_json, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        params![desktop, json, super::now_secs(), by],
    )
    .map_err(|e| format!("desktop_settings write: {e}"))?;
    Ok(list)
}

// ── the process-wide DB ─────────────────────────────────────────────

/// [`get`] on the shared DB.
pub fn settings(desktop: &str) -> Result<Settings, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    get(&conn, desktop)
}

/// Is the desktop on?
pub fn is_enabled(desktop: &str) -> Result<bool, String> {
    settings(desktop).map(|s| s.enabled)
}

/// [`set_enabled`] on the shared DB.
pub fn enable(desktop: &str, enabled: bool, by: &str) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    set_enabled(&conn, desktop, enabled, by)
}

/// [`set_hide_apps`] on the shared DB.
pub fn hide_apps(desktop: &str, raw: &[String], by: &str) -> Result<Vec<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    set_hide_apps(&conn, desktop, raw, by)
}

// ── the agent switch (0301) ─────────────────────────────────────────

/// "Allow a virtual desktop" for this workspace (0301, LD-38; default off):
/// its agent may have its own desktop, `virtual:<workspace id>`.
pub fn agents_can_use_desktop(conn: &Connection, project_id: &str) -> bool {
    conn.query_row("SELECT agents_can_use_desktop FROM projects WHERE id = ?1", params![project_id], |r| r.get::<_, i64>(0))
        .map(|v| v != 0)
        .unwrap_or(false)
}

pub fn set_agents_can_use_desktop(conn: &Connection, project_id: &str, on: bool) -> Result<(), String> {
    let n = conn
        .execute("UPDATE projects SET agents_can_use_desktop = ?2 WHERE id = ?1", params![project_id, on as i64])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("workspace {project_id} not found"));
    }
    Ok(())
}

/// Workspaces with the switch on: `(id, name)`, by name.
pub fn desktop_agents(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut st = conn
        .prepare("SELECT id, name FROM projects WHERE agents_can_use_desktop = 1 ORDER BY name COLLATE NOCASE, id")
        .map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// The workspace behind an agent's desktop id: `(project id, name)`, or
/// `None` when no workspace has that desktop (removed, or not an agent's
/// id). Ids are `virtual:<project id>`, or a hashed form for a project id
/// outside the grammar (`virtual_desktop::agent_desktop`).
pub fn desktop_workspace(conn: &Connection, desktop: &str) -> Option<(String, String)> {
    use crate::desktop::virtual_desktop::{agent_desktop, workspace_of};
    let tail = workspace_of(desktop)?;
    let direct: Option<(String, String)> =
        conn.query_row("SELECT id, name FROM projects WHERE id = ?1", params![tail], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten();
    if let Some(p) = direct {
        return Some(p);
    }
    if !tail.starts_with('x') {
        return None;
    }
    let mut st = conn.prepare("SELECT id, name FROM projects").ok()?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).ok()?;
    let all: Vec<(String, String)> = rows.flatten().collect();
    all.into_iter().find(|(id, _)| agent_desktop(id).as_deref() == Some(desktop))
}

/// A workspace's name, or `None` when it no longer exists.
pub fn workspace_name(conn: &Connection, project_id: &str) -> Option<String> {
    conn.query_row("SELECT name FROM projects WHERE id = ?1", params![project_id], |r| r.get(0)).optional().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::CONSOLE;

    #[test]
    fn the_agent_switch_defaults_off_and_round_trips() {
        let db = crate::db::scoped_for_test();
        let h = db.handle();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('w1', 'scientist', '/tmp/w1'), ('w2', 'Atlas', '/tmp/w2')", []).unwrap();
        assert!(!agents_can_use_desktop(&conn, "w1"), "off by default");
        set_agents_can_use_desktop(&conn, "w1", true).expect("on");
        set_agents_can_use_desktop(&conn, "w2", true).expect("on");
        assert!(agents_can_use_desktop(&conn, "w1"));
        assert_eq!(desktop_agents(&conn).unwrap(), vec![("w2".to_string(), "Atlas".to_string()), ("w1".to_string(), "scientist".to_string())]);
        set_agents_can_use_desktop(&conn, "w2", false).expect("off");
        assert_eq!(desktop_agents(&conn).unwrap().len(), 1);
        assert!(set_agents_can_use_desktop(&conn, "nope", true).is_err());
        assert_eq!(workspace_name(&conn, "w1").as_deref(), Some("scientist"));
        assert_eq!(workspace_name(&conn, "gone"), None);
        // Back from a desktop id, the plain form and the hashed one.
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('Old_ID.7', 'legacy', '/tmp/old')", []).unwrap();
        assert_eq!(desktop_workspace(&conn, "virtual:w1"), Some(("w1".to_string(), "scientist".to_string())));
        let odd = crate::desktop::virtual_desktop::agent_desktop("Old_ID.7").unwrap();
        assert_eq!(desktop_workspace(&conn, &odd), Some(("Old_ID.7".to_string(), "legacy".to_string())));
        assert_eq!(desktop_workspace(&conn, "virtual:gone"), None);
        assert_eq!(desktop_workspace(&conn, "virtual"), None);
    }

    #[test]
    fn the_switch_defaults_off_and_round_trips() {
        let _db = crate::db::scoped_for_test();
        let s = settings(CONSOLE).expect("read");
        assert!(!s.enabled, "off with no row");
        assert_eq!(s.hide_apps, DEFAULT_HIDE_APPS.iter().map(|s| s.to_string()).collect::<Vec<_>>());

        assert!(enable(CONSOLE, true, "login:rosson").expect("on"), "changed");
        assert!(!enable(CONSOLE, true, "login:rosson").expect("on again"), "no change");
        let s = settings(CONSOLE).expect("read");
        assert!(s.enabled);
        assert_eq!(s.updated_by, "login:rosson");
        assert!(s.updated_at > 0);
        assert_eq!(s.hide_apps.len(), DEFAULT_HIDE_APPS.len(), "turning it on keeps the default list");

        assert!(enable(CONSOLE, false, "owner").expect("off"));
        assert!(!is_enabled(CONSOLE).expect("read"));
        assert!(!is_enabled("agent:other").expect("read"), "another desktop is its own row");
    }

    #[test]
    fn the_hide_list_is_checked_and_kept_apart_from_the_switch() {
        let _db = crate::db::scoped_for_test();
        let list = hide_apps(CONSOLE, &[" com.apple.Passwords ".into(), "com.apple.Passwords".into(), "com.example.Vault".into()], "owner")
            .expect("set");
        assert_eq!(list, vec!["com.apple.Passwords".to_string(), "com.example.Vault".to_string()]);
        assert!(!is_enabled(CONSOLE).unwrap(), "a policy change never turns it on");
        enable(CONSOLE, true, "owner").unwrap();
        assert_eq!(settings(CONSOLE).unwrap().hide_apps, list, "turning it on keeps the list");
        for bad in ["", "nodots", ".com.x", "com.x.", "com x.y", "com/x.y"] {
            assert!(hide_apps(CONSOLE, &[bad.into()], "owner").is_err(), "{bad:?}");
        }
        let many: Vec<String> = (0..=MAX_HIDE_APPS).map(|i| format!("com.example.a{i}")).collect();
        assert!(hide_apps(CONSOLE, &many, "owner").is_err());
        assert_eq!(hide_apps(CONSOLE, &[], "owner").unwrap(), Vec::<String>::new(), "an empty list is allowed");
        assert!(settings(CONSOLE).unwrap().hide_apps.is_empty(), "and stays empty (not the default)");
    }
}
