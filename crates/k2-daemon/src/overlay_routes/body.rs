//! `POST` body handling for the overlay routes: a JSON body field of the
//! wrong type is a 400, and a JSON or form body merges into the params.
//! Moved out of overlay_routes.rs as it was (0.46.18 queue, ratchet).

use std::collections::HashMap;

use super::usage;
use crate::cli_response::CliResponse;

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// `POST /cli/thread/post`: a known JSON body field of the wrong type is a
/// 400 (0.46.5 papercut). Before, `attachments` sent as an object was
/// silently dropped (the post went out with no files), and an array or
/// object under `text` was posted as its JSON text. `attachments` takes a
/// JSON array of `{path|relPath, line?, lineEnd?}`, or a string holding
/// that array (the form-body shape `k2 thread --file` sends); `null` is
/// "none". The text fields take strings (a number or boolean still reads
/// as its text, as before). Form bodies are unaffected.
pub(super) fn thread_post_body_type_error(body: &[u8]) -> Option<CliResponse> {
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_slice::<serde_json::Value>(body) else {
        return None;
    };
    for (k, v) in &obj {
        let want = match k.as_str() {
            "attachments" if !(v.is_array() || v.is_string() || v.is_null()) => {
                "a JSON array of {path, line?, lineEnd?}"
            }
            "addr" | "text" | "from" | "via" | "command" if v.is_array() || v.is_object() => "a string",
            // post-idempotency-v1: a number or boolean would merge as its
            // text and pass the id check, so only a string is an id.
            "clientMsgId" if !(v.is_string() || v.is_null()) => "a string of 1-64 A-Z, a-z, 0-9, '-' or '_'",
            // thread-reply-to-v1: a seq, as a number or "#N".
            "replyTo" if !(v.is_string() || v.is_number() || v.is_null()) => "a message's or card's number (N or \"#N\")",
            "replyQuote" if !(v.is_string() || v.is_null()) => "a string (the words of the message you reply to part of)",
            _ => continue,
        };
        return Some(usage(format!("{k} must be {want}, not {}", json_type_name(v))));
    }
    None
}

pub(super) fn restore_param(params: &mut HashMap<String, String>, key: &str, captured: Option<String>) {
    match captured {
        Some(value) => {
            params.insert(key.to_string(), value);
        }
        None => {
            params.remove(key);
        }
    }
}

pub(super) fn merge_body(params: &mut HashMap<String, String>, body: &[u8]) {
    if body.is_empty() {
        return;
    }
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(obj) = v.as_object() {
            for (k, val) in obj {
                if let Some(s) = val.as_str() {
                    params.insert(k.clone(), s.to_string());
                } else if val.is_number() || val.is_boolean() {
                    params.insert(k.clone(), val.to_string());
                } else if val.is_array() || (val.is_object() && (k == "origin" || k == "context" || k == "inject" || k == "meta")) {
                    // `origin` (UWB12a) and `context` (thread-context-v1) are a Thread
                    // post's object fields; `inject` (`{dst, fmt}`) is how a secret card
                    // sends its value (prd-secret-card-injection-v1); `meta` is a choice
                    // card's metadata map (0.46.17). Each rides as its JSON text.
                    params.insert(k.clone(), val.to_string());
                }
            }
            return;
        }
    }
    for (k, v) in crate::routes::http::parse_form_body(body) {
        params.insert(k, v);
    }
}
