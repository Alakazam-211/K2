//! Outbound queue — `x:QueuedMessage` (not mailbox Trash, not EXPUNGE).
//!
//! list = query+get. retry = `nextRetry` now. drop = destroy.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::hosted::{self, err_json};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct IdBody {
    id: Option<String>,
}

fn require_id(body: &[u8]) -> Result<String, CliResponse> {
    let b: IdBody = serde_json::from_slice(body).map_err(|e| {
        err_json(
            "400 Bad Request",
            "usage",
            format!("invalid JSON body: {e}"),
        )
    })?;
    b.id.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            err_json(
                "400 Bad Request",
                "usage",
                "missing 'id' — queued message id".to_string(),
            )
        })
}

fn now_utc_date() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// GET `/cli/mail/queue`
pub fn handle_queue_get(_params: &HashMap<String, String>) -> CliResponse {
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let ids = match engine.queued_message_query() {
        Ok(ids) => ids,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    let rows = match engine.queued_message_get(&ids) {
        Ok(r) => r,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    let messages: Vec<serde_json::Value> = rows
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "nextRetry": m.next_retry,
                "returnPath": m.return_path,
                "recipients": m.recipients,
            })
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "messages": messages,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/queue/retry` `{id}`
pub fn handle_retry(body: &[u8]) -> CliResponse {
    let id = match require_id(body) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let next = now_utc_date();
    if let Err(e) = engine.queued_message_retry(&id, &next) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": id,
            "nextRetry": next,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/queue/drop` `{id}` — `x:QueuedMessage` destroy.
/// Not mailbox Trash. Not EXPUNGE. Not `k2 mail delete`.
pub fn handle_drop(body: &[u8]) -> CliResponse {
    let id = match require_id(body) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    if let Err(e) = engine.queued_message_destroy(&id) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": id,
            "dropped": true,
            "trash": false,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_without_id_is_usage() {
        let r = handle_retry(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("id"), "{}", r.body);
    }

    #[test]
    fn drop_without_id_is_usage() {
        let r = handle_drop(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn get_without_engine_is_not_405() {
        let r = handle_queue_get(&HashMap::new());
        assert_ne!(r.status, "405 Method Not Allowed");
        assert_ne!(r.status, "404 Not Found");
    }

    #[test]
    fn now_utc_date_is_zulu() {
        let s = now_utc_date();
        assert!(s.ends_with('Z'), "{s}");
        assert!(s.contains('T'), "{s}");
    }
}
