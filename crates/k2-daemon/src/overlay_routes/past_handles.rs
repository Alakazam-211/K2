//! A sidecar conversation's past addresses (`pastAddresses` on Thread
//! reads). A child of `overlay_routes` (moved out unchanged for the line
//! ratchet).

use super::*;

/// `ws/<n>` and each retired `ws/<name>` of a sidecar conversation.
pub(super) fn past_addresses(resolved: &ResolvedOverlay) -> Vec<String> {
    if resolved.canonical_alias {
        return Vec::new();
    }
    let Some((ws, _)) = k2_core::workspace_session_handles::split_workspace_handle(&resolved.addr)
    else {
        return Vec::new();
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace_session_handles::past_handles_for(&conn, &resolved.project_id, &resolved.conversation_id)
        .unwrap_or_default()
        .into_iter()
        .map(|h| k2_core::workspace_session_handles::format_address(ws, Some(&h)))
        .collect()
}
