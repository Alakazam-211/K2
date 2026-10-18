//! Creator rights for widget data calls (contract §3.5, §7.1 step 6; PRD
//! §4.2 table, BG3). C0 → B2.
//!
//! A widget's `store.*` calls use its live hash's maker's rights:
//! owner or K2 → the whole keyspace; an agent → that agent's effective
//! grants on this keyspace; unclaimed → `author_unknown`. `save.*` needs no
//! rights (it is private to the widget and never granted).
//!
//! The maker's grants are re-read on every call (no cache), so a revoke or
//! a project leave takes effect on the widget's next call too: an
//! agent-made widget can never read or write past what its maker could
//! reach through `k2 store` itself.
//!
//! **The maker's own widget (0.46.1, gap 7).** An agent with the person's
//! "Manage Gardens and data sources" switch (`gardens_manage`, migration
//! 0181) reaches its OWN widget's keyspace in full with no grant: the
//! widget's calls (its live version is that agent's), and the agent's own
//! `k2 store --widget <w>` when it made the newest stamped version of `w`
//! ([`maker_owns_widget_data`]). That lets an agent seed a widget it built.
//! The switch is re-read on every call, so turning it off ends this at
//! once; the daemon writes an Access Audit line when it is used.

use rusqlite::{params, Connection, OptionalExtension};

use super::{authors, grants, Access, DataError, MadeBy, Space};

/// Does `project_id`'s agent hold the `gardens_manage` switch? Read on
/// `conn` (callers already hold the shared lock). Unknown workspace → false.
pub fn agent_manages_gardens(conn: &Connection, project_id: &str) -> Result<bool, DataError> {
    let v: Option<i64> = conn
        .query_row("SELECT agents_can_manage_gardens FROM projects WHERE id = ?1", params![project_id], |r| r.get(0))
        .optional()?;
    Ok(v == Some(1))
}

/// What a widget made by `maker` may touch in `space`. An agent maker with
/// the `gardens_manage` switch gets its own widget's keyspace in full.
pub fn rights_for_widget(conn: &Connection, space: &Space, maker: &MadeBy) -> Result<Access, DataError> {
    match maker {
        MadeBy::Owner | MadeBy::K2 => Ok(Access::Full),
        MadeBy::Agent { project_id } if agent_manages_gardens(conn, project_id)? => Ok(Access::Full),
        MadeBy::Agent { project_id } => grants::access_for_agent(conn, space, project_id),
        MadeBy::Unclaimed => Err(DataError::AuthorUnknown),
    }
}

/// What a widget made by `maker` may READ in a neighbour's `space` (a
/// `{from}` read, 0.46.1 gap 5): the owner's and K2's widgets everything;
/// an agent's widget its maker's grants there, or everything when that
/// agent also made the neighbour and holds the switch. Callers never write
/// through this.
pub fn rights_for_neighbour(conn: &Connection, space: &Space, maker: &MadeBy) -> Result<Access, DataError> {
    match maker {
        MadeBy::Owner | MadeBy::K2 => Ok(Access::Full),
        MadeBy::Agent { project_id } if maker_owns_widget_data(conn, space, project_id)? => Ok(Access::Full),
        MadeBy::Agent { project_id } => grants::access_for_agent(conn, space, project_id),
        MadeBy::Unclaimed => Err(DataError::AuthorUnknown),
    }
}

/// Does `project_id`'s agent reach `space` as its widget's maker: a `data:`
/// space of a folder widget whose newest stamped version that agent made,
/// with the `gardens_manage` switch on? (Its `k2 store --widget` then gets
/// [`Access::Full`] without a grant.)
pub fn maker_owns_widget_data(conn: &Connection, space: &Space, project_id: &str) -> Result<bool, DataError> {
    if !space.is_data() || authors::is_builtin(space.widget()) {
        return Ok(false);
    }
    let mine = matches!(authors::newest_maker(conn, space.widget())?, Some(MadeBy::Agent { project_id: p }) if p == project_id);
    Ok(mine && agent_manages_gardens(conn, project_id)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_k2_get_the_whole_keyspace_and_unclaimed_gets_nothing() {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::db::run_migrations(&conn).expect("migrations");
        let space = Space::data("board").expect("space");
        assert_eq!(rights_for_widget(&conn, &space, &MadeBy::Owner).expect("owner"), Access::Full);
        assert_eq!(rights_for_widget(&conn, &space, &MadeBy::K2).expect("k2"), Access::Full);
        assert_eq!(rights_for_widget(&conn, &space, &MadeBy::Unclaimed), Err(DataError::AuthorUnknown));
    }

    /// Gap 7 (0.46.1): the maker agent with the switch reaches its own
    /// widget's data; without the switch, only its grants (none here).
    #[test]
    fn a_maker_agent_with_the_switch_reaches_its_own_widgets_data() {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::db::run_migrations(&conn).expect("migrations");
        for id in ["w-builder", "w-other"] {
            conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?1, ?2)", params![id, format!("/w/{id}")])
                .expect("project row");
        }
        let space = Space::data("fleet-ops-lists").expect("space");
        let builder = MadeBy::Agent { project_id: "w-builder".into() };
        let hash = "a".repeat(64);
        authors::stamp(&conn, "fleet-ops-lists", &hash, &builder, authors::StampSource::Sign, authors::CallerKind::Agent, 10)
            .expect("stamp");
        // Switch off: grants only, and there are none.
        assert_eq!(rights_for_widget(&conn, &space, &builder).expect("rights"), Access::None);
        assert!(!maker_owns_widget_data(&conn, &space, "w-builder").expect("own"));
        // Switch on: the whole keyspace, for the widget and for the agent.
        conn.execute("UPDATE projects SET agents_can_manage_gardens = 1 WHERE id = 'w-builder'", []).expect("switch");
        assert_eq!(rights_for_widget(&conn, &space, &builder).expect("rights"), Access::Full);
        assert!(maker_owns_widget_data(&conn, &space, "w-builder").expect("own"));
        // Another agent with the switch doesn't own it; nor does anyone own a
        // built-in's or a save space.
        conn.execute("UPDATE projects SET agents_can_manage_gardens = 1 WHERE id = 'w-other'", []).expect("switch");
        assert!(!maker_owns_widget_data(&conn, &space, "w-other").expect("not theirs"));
        assert!(!maker_owns_widget_data(&conn, &Space::data("k2:diary").expect("space"), "w-builder").expect("builtin"));
        assert!(!maker_owns_widget_data(&conn, &Space::save("fleet-ops-lists").expect("space"), "w-builder").expect("save"));
        // The person's claim on a newer version moves ownership to them.
        authors::claim(&conn, "fleet-ops-lists", &"b".repeat(64), 20).expect("claim");
        assert!(!maker_owns_widget_data(&conn, &space, "w-builder").expect("claimed newer"));
        assert_eq!(authors::newest_maker(&conn, "fleet-ops-lists").expect("newest"), Some(MadeBy::Owner));
        assert_eq!(authors::newest_maker(&conn, "never-signed").expect("none"), None);
    }
}
