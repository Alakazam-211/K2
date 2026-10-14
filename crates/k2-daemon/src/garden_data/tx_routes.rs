//! Garden data routes: tx/begin, tx/add, tx/commit, tx/abort for agents and the person (contract §4.5; PRD §5.3.1). C0 → B2.
//!
//! The gates in `zen_data_routes::handle` already ran (credential kind,
//! person-only rows, POST-only, body cap). Handlers answer the 200 body or
//! a `DataError` (contract §4.6).

use std::collections::HashMap;

use k2_core::garden_data::{not_built, DataError};

use crate::zen_data_routes::DataWho;

pub fn handle(
    _path: &str,
    _who: &DataWho,
    _body: &[u8],
    _params: &HashMap<String, String>,
) -> Result<serde_json::Value, DataError> {
    Err(not_built())
}
