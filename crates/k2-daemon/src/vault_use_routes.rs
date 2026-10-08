//! `/cli/vault/*` routes that USE a secret on the caller's behalf:
//! `fetch` (V4), `env-file` and `import` (V8). Split from
//! `vault_routes.rs` so the delivery work lands without touching the
//! management routes.

use std::collections::HashMap;
use std::sync::Arc;

use k2_vault::{LocalVault, VaultError};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;

/// Routes this module serves.
pub const ROUTES: &[&str] = &[];
/// POST-only routes among them.
pub const POST_ROUTES: &[&str] = &[];

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub fn handle(
    _vault: &Arc<LocalVault>,
    _path: &str,
    _c: &VaultCaller,
    _body: &[u8],
    _params: &HashMap<String, String>,
    _ingress: &str,
) -> CliResponse {
    CliResponse::not_found()
}

/// A published service's id from its label (`name`) or id. Agents see
/// only their own workspace's services.
pub fn published_service_id(c: &VaultCaller, label: &str) -> Result<String, VaultError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, project_id FROM published_services WHERE id = ?1 OR name = ?1 ORDER BY (id = ?1) DESC LIMIT 1",
            rusqlite::params![label],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (id, project) = row.ok_or_else(|| VaultError::not_found(format!("no published service {label}")))?;
    if let Some(ws) = c.workspace_id() {
        if ws != project {
            return Err(VaultError::not_found(format!("no published service {label} in this workspace")));
        }
    }
    Ok(id)
}
