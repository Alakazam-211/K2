//! A choice card's metadata map (`k2 thread ask --meta`, 0.46.17).
//!
//! An App asked for a way to tag the cards an agent posts and read the tags
//! back: `choice.meta` is a small flat map of keys to strings, numbers or
//! booleans, stored with the card and returned wherever the card goes (the
//! Thread reads, deltas and catch-up, the overlay socket and the room
//! streams). It is NOT for secrets: whoever can see the card sees it.
//!
//! The limits live here, once. The daemon enforces them on every ask; the
//! CLI checks the same limits early, with the same words, so a bad map is
//! refused before anything is sent. A refusal is a usage error and nothing
//! is stored.

use serde_json::{Map, Value};

/// The stored map. Keys sort (serde_json's map is ordered by key here), so
/// the stored and answered bytes don't depend on how the caller ordered it.
pub type CardMeta = Map<String, Value>;

/// Most keys one card's map may hold.
pub const MAX_KEYS: usize = 32;
/// Longest key, in bytes (keys are ASCII).
pub const MAX_KEY_LEN: usize = 64;
/// Largest map, in bytes of its compact JSON (`{"k":"v",...}`).
pub const MAX_JSON_BYTES: usize = 4096;

/// The refusal for a body `meta` that isn't a JSON object.
pub const NOT_AN_OBJECT: &str = "meta must be a JSON object of keys to strings, numbers or booleans";

/// One key's rule: 1 to 64 of `a-z`, `0-9`, `_`, `.`, `-`.
pub fn key_ok(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'))
}

fn quoted(key: &str) -> String {
    serde_json::to_string(key).unwrap_or_else(|_| format!("\"{key}\""))
}

/// Check a whole map: the key count, then each key in order (its name, then
/// its value's type), then the size. `Ok(None)` for an empty map: no field
/// is stored, so the card stays exactly what it was without one.
pub fn check(meta: &CardMeta) -> Result<Option<CardMeta>, String> {
    if meta.len() > MAX_KEYS {
        return Err(format!("meta takes at most {MAX_KEYS} keys"));
    }
    for (key, value) in meta {
        if !key_ok(key) {
            return Err(format!("meta key {} must match [a-z0-9_.-]{{1,{MAX_KEY_LEN}}}", quoted(key)));
        }
        if !matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
            return Err(format!("meta value for {} must be a string, number or boolean", quoted(key)));
        }
    }
    let bytes = serde_json::to_string(meta).map(|s| s.len()).unwrap_or(usize::MAX);
    if bytes > MAX_JSON_BYTES {
        return Err(format!("meta takes at most {MAX_JSON_BYTES} bytes of JSON"));
    }
    Ok((!meta.is_empty()).then(|| meta.clone()))
}

/// The request's `meta` value as the daemon's param map carries it: the
/// object's JSON text (a JSON body), or a string holding that object (a
/// form body). Anything that isn't an object is [`NOT_AN_OBJECT`].
pub fn parse_param(raw: &str) -> Result<Option<CardMeta>, String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => check(&map),
        _ => Err(NOT_AN_OBJECT.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> CardMeta {
        v.as_object().expect("object").clone()
    }

    #[test]
    fn strings_numbers_and_booleans_pass_and_sort_by_key() {
        let got = check(&map(json!({"z": "last", "a.b-c_9": 3, "ok": true, "f": 1.5}))).expect("ok").expect("some");
        assert_eq!(serde_json::to_string(&got).expect("json"), r#"{"a.b-c_9":3,"f":1.5,"ok":true,"z":"last"}"#);
    }

    #[test]
    fn an_empty_map_stores_nothing() {
        assert_eq!(check(&CardMeta::new()), Ok(None));
        assert_eq!(parse_param("{}"), Ok(None));
    }

    #[test]
    fn keys_match_the_rule() {
        let long = "k".repeat(65);
        for bad in ["", "Upper", "sp ace", "slash/", "ü", long.as_str()] {
            let err = check(&map(json!({ bad: "v" }))).expect_err(bad);
            assert_eq!(err, format!("meta key {} must match [a-z0-9_.-]{{1,64}}", serde_json::to_string(bad).expect("q")));
        }
        let most = "k".repeat(64);
        assert!(check(&map(json!({ most: "v" }))).is_ok(), "64 is the most");
    }

    #[test]
    fn values_are_strings_numbers_or_booleans_only() {
        for bad in [json!(null), json!([1]), json!({"x": 1})] {
            let err = check(&map(json!({ "k": bad }))).expect_err("refused");
            assert_eq!(err, r#"meta value for "k" must be a string, number or boolean"#);
        }
    }

    #[test]
    fn at_most_32_keys() {
        let ok: CardMeta = (0..32).map(|i| (format!("k{i}"), json!(i))).collect();
        assert!(check(&ok).is_ok());
        let over: CardMeta = (0..33).map(|i| (format!("k{i}"), json!(i))).collect();
        assert_eq!(check(&over), Err("meta takes at most 32 keys".to_string()));
    }

    #[test]
    fn at_most_4096_bytes_of_compact_json() {
        // {"k":"<v>"} is 8 bytes around the value.
        let at = map(json!({ "k": "v".repeat(MAX_JSON_BYTES - 8) }));
        assert_eq!(serde_json::to_string(&at).expect("json").len(), MAX_JSON_BYTES);
        assert!(check(&at).is_ok());
        let over = map(json!({ "k": "v".repeat(MAX_JSON_BYTES - 7) }));
        assert_eq!(check(&over), Err("meta takes at most 4096 bytes of JSON".to_string()));
    }

    #[test]
    fn the_param_must_hold_an_object() {
        for bad in ["[1]", "3", "true", "\"s\"", "null", "not json"] {
            assert_eq!(parse_param(bad), Err(NOT_AN_OBJECT.to_string()), "{bad}");
        }
        assert_eq!(parse_param(r#"{"a":"1"}"#).expect("ok"), Some(map(json!({"a": "1"}))));
    }
}
