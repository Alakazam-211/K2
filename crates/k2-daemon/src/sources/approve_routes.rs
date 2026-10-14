//! `pending`, `approve`, `batch-approve`, `secret`, `pin` and `unpin`
//! handlers (contract §4.2, §4.3). Owner: B2.
//!
//! `crate::sources_routes::handle` has already classified the caller and
//! refused app passes and invalid tokens; both transports refuse a GET to a
//! POST-only path with 405.
//!
//! C0 skeleton: 501 `not_built` for every path.

use std::collections::HashMap;

use crate::compute_routes::Who;

use super::{not_built, SourceErr};

pub fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> Result<serde_json::Value, SourceErr> {
    let _ = (path, who, body, params);
    Err(not_built())
}
