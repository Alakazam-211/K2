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

use rusqlite::Connection;

use super::{grants, Access, DataError, MadeBy, Space};

/// What a widget made by `maker` may touch in `space`.
pub fn rights_for_widget(conn: &Connection, space: &Space, maker: &MadeBy) -> Result<Access, DataError> {
    match maker {
        MadeBy::Owner | MadeBy::K2 => Ok(Access::Full),
        MadeBy::Agent { project_id } => grants::access_for_agent(conn, space, project_id),
        MadeBy::Unclaimed => Err(DataError::AuthorUnknown),
    }
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
}
