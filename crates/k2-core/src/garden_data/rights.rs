//! Creator rights for widget data calls (contract §3.5, §7.1 step 6; PRD
//! §4.2 table, BG3). C0 → B2.
//!
//! A widget's `store.*` calls use its live hash's maker's rights:
//! owner or K2 → the whole keyspace; an agent → that agent's effective
//! grants on this keyspace; unclaimed → `author_unknown`. `save.*` needs no
//! rights (it is private to the widget and never granted).

use rusqlite::Connection;

use super::{not_built, Access, DataError, MadeBy, Space};

/// What a widget made by `maker` may touch in `space`.
pub fn rights_for_widget(_conn: &Connection, _space: &Space, _maker: &MadeBy) -> Result<Access, DataError> {
    Err(not_built())
}
