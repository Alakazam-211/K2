//! A route that a live App pass reads for itself (`/cli/skin/me` and the
//! compat layer's claims token): the pass from the query, refused unless it
//! is an App pass, revoked when it no longer resolves, and the handler run
//! off the async thread with whether the request came on the App's own
//! gateway link. Both arms of the dispatcher's `/cli/skin` block used to
//! spell this out in full (moved here as it was, 0.46.18 queue, ratchet).

use crate::cli_response::CliResponse;
use k2_core::skin::SkinPass;

/// The answer for `query` with `handle(pass, on_link)`.
pub(crate) async fn answer(query: &str, handle: fn(&SkinPass, bool) -> CliResponse) -> CliResponse {
    let presented = super::http::extract_token(query);
    if presented.is_some_and(k2_core::skin::is_skin_token) {
        let on_link = crate::apps::gateway::current().is_some();
        match presented.and_then(crate::apps::gateway::resolve_pass) {
            Some(pass) => tokio::task::spawn_blocking(move || handle(&pass, on_link))
                .await
                .unwrap_or_else(|e| CliResponse::internal_error(e)),
            None => crate::skin_routes::revoked_skin_response(),
        }
    } else {
        crate::skin_me_routes::forbidden()
    }
}
