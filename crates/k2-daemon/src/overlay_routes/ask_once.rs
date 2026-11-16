//! Idempotent sends (0.46.15, [`crate::send_once`]): `k2 thread ask` posts
//! one card per `idempotency_key`. A CLI that got no answer asks again with
//! the same key and gets the first card back (`"replayed": true`) instead
//! of a second card. The sender is the credential's, as for a Thread post's
//! `clientMsgId` ([`thread_client_ns`]): one agent's key never answers
//! another's.

use super::*;

/// `POST /cli/thread/ask` behind the ledger. `stamped`: the dispatcher
/// stamped a passport on `params` (not a body claim).
pub(super) fn handle(path: &str, params: &HashMap<String, String>, stamped: bool, session_author: &str) -> CliResponse {
    let principal = crate::caller_workspace::principal_from_params(params);
    let sender = thread_client_ns(principal.as_ref(), stamped, "ask", session_author);
    let key = params.get(crate::send_once::PARAM).map(|s| s.trim()).filter(|s| !s.is_empty());
    crate::send_once::once_keyed(path, &sender, key, || handle_ask(params, session_author))
}
