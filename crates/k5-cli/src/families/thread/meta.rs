//! `ask --meta key=value` and `--meta-json '{...}'` (0.46.17), as
//! `_thread_ask_meta` checks and merges them (`cli/src/browser.sh`).
//!
//! The daemon enforces the same limits (k2-core `overlay::card_meta`); the
//! shell checks them first, with the daemon's words, and so does this:
//!
//! 1. `--meta-json`'s value, read as Python's `json.loads` with no `NaN`,
//!    `Infinity` or out-of-range float: anything else, or not an object,
//!    is [`NOT_AN_OBJECT`];
//! 2. each `--meta` pair in order: no `=` is [`NEEDS_PAIR`], a key already
//!    there is "given twice";
//! 3. more than 32 keys;
//! 4. each key in code point order: its name, then its value's type;
//! 5. the map's compact JSON (keys sorted, UTF-8 as is) over 4096 bytes.
//!
//! The merged map is that compact JSON, or `None` when it is empty. The
//! shell answers a map holding a lone surrogate (Python fails on it its own
//! way) and one nested deeper than this reader goes.

use crate::out::json::{self, Options, Text, Value};

/// The refusal for a `--meta-json` that isn't a JSON object.
pub const NOT_AN_OBJECT: &str =
    "meta must be a JSON object of keys to strings, numbers or booleans";
/// A `--meta` with no `=`, or no value at all.
pub const NEEDS_PAIR: &str = "--meta needs key=value";
/// `--meta-json` with no value.
pub const NEEDS_OBJECT: &str = "--meta-json needs a JSON object";
/// `--meta-json` a second time.
pub const JSON_TWICE: &str = "--meta-json is given twice";

const MAX_KEYS: usize = 32;
const MAX_JSON_BYTES: usize = 4096;

/// `json.dumps(k, ensure_ascii=False)`.
fn quoted(key: &Text) -> String {
    json::to_string(
        &Value::Str(key.clone()),
        &Options {
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

fn key_ok(key: &Text) -> bool {
    key.as_str().is_some_and(|k| {
        (1..=64).contains(&k.len())
            && k.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
            })
    })
}

/// True when `v` holds a lone surrogate (in a key or a string) anywhere.
fn has_loose_text(v: &Value) -> bool {
    match v {
        Value::Str(t) => t.as_str().is_none(),
        Value::Array(items) => items.iter().any(has_loose_text),
        Value::Object(pairs) => pairs
            .iter()
            .any(|(k, v)| k.as_str().is_none() || has_loose_text(v)),
        _ => false,
    }
}

/// True when a float anywhere in `v` is NaN or infinite (Python's
/// `parse_constant` and `parse_float` refuse those).
fn has_non_finite(v: &Value) -> bool {
    match v {
        Value::Float(f) => !f.is_finite(),
        Value::Array(items) => items.iter().any(has_non_finite),
        Value::Object(pairs) => pairs.iter().any(|(_, v)| has_non_finite(v)),
        _ => false,
    }
}

/// The merged map (`given`: `--meta-json`'s value when given; `pairs`: the
/// `--meta` values in order). `None`: the shell answers it. `Some(Err)`:
/// the refusal's words (the line is `Error: <words>`, exit 2).
pub fn merge(given: Option<&str>, pairs: &[String]) -> Option<Result<Option<String>, String>> {
    let mut meta: Vec<(Text, Value)> = Vec::new();
    if let Some(raw) = given {
        match json::parse(raw) {
            Err(json::Error::TooDeep { .. }) => return None,
            Err(_) => return Some(Err(NOT_AN_OBJECT.to_string())),
            Ok(v) if has_non_finite(&v) => return Some(Err(NOT_AN_OBJECT.to_string())),
            Ok(v) if has_loose_text(&v) => return None,
            Ok(Value::Object(obj)) => meta = obj,
            Ok(_) => return Some(Err(NOT_AN_OBJECT.to_string())),
        }
    }
    for pair in pairs {
        let Some((k, v)) = pair.split_once('=') else {
            return Some(Err(NEEDS_PAIR.to_string()));
        };
        let key = Text::from(k);
        if meta.iter().any(|(have, _)| *have == key) {
            return Some(Err(format!("meta key {} is given twice", quoted(&key))));
        }
        meta.push((key, Value::Str(Text::from(v))));
    }
    if meta.len() > MAX_KEYS {
        return Some(Err(format!("meta takes at most {MAX_KEYS} keys")));
    }
    meta.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, v) in &meta {
        if !key_ok(k) {
            return Some(Err(format!(
                "meta key {} must match [a-z0-9_.-]{{1,64}}",
                quoted(k)
            )));
        }
        if !matches!(
            v,
            Value::Str(_) | Value::Bool(_) | Value::Int(_) | Value::Float(_)
        ) {
            return Some(Err(format!(
                "meta value for {} must be a string, number or boolean",
                quoted(k)
            )));
        }
    }
    let compact = Options {
        compact: true,
        ensure_ascii: false,
        ..Options::default()
    };
    let empty = meta.is_empty();
    let out = json::to_string(&Value::Object(meta), &compact).ok()?;
    if out.len() > MAX_JSON_BYTES {
        return Some(Err(format!(
            "meta takes at most {MAX_JSON_BYTES} bytes of JSON"
        )));
    }
    Some(Ok((!empty).then_some(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn strings_and_json_merge_sorted() {
        assert_eq!(
            merge(
                Some(r#"{"qty": 3, "w": 1.5, "ok": true}"#),
                &pairs(&["order=A-17", "note=a=b"])
            ),
            Some(Ok(Some(
                r#"{"note":"a=b","ok":true,"order":"A-17","qty":3,"w":1.5}"#.to_string()
            )))
        );
        assert_eq!(merge(Some("{}"), &[]), Some(Ok(None)));
    }

    #[test]
    fn the_shell_s_refusals() {
        let refused =
            |g: Option<&str>, p: &[&str]| merge(g, &pairs(p)).expect("ours").expect_err("refused");
        assert_eq!(refused(Some("[1]"), &[]), NOT_AN_OBJECT);
        assert_eq!(refused(Some(r#"{"n": NaN}"#), &[]), NOT_AN_OBJECT);
        assert_eq!(refused(Some(r#"{"n": 1e400}"#), &[]), NOT_AN_OBJECT);
        assert_eq!(refused(Some(""), &[]), NOT_AN_OBJECT);
        assert_eq!(refused(None, &["order"]), NEEDS_PAIR);
        assert_eq!(
            refused(Some(r#"{"a": "1"}"#), &["a=2"]),
            r#"meta key "a" is given twice"#
        );
        assert_eq!(
            refused(None, &["Up=1"]),
            r#"meta key "Up" must match [a-z0-9_.-]{1,64}"#
        );
        assert_eq!(
            refused(Some(r#"{"k": null}"#), &[]),
            r#"meta value for "k" must be a string, number or boolean"#
        );
        let many: Vec<String> = (0..33).map(|i| format!("k{i}=1")).collect();
        assert_eq!(
            merge(None, &many).expect("ours").expect_err("33"),
            "meta takes at most 32 keys"
        );
        let big = format!("k={}", "v".repeat(4089));
        assert_eq!(
            merge(None, &[big]).expect("ours").expect_err("big"),
            "meta takes at most 4096 bytes of JSON"
        );
        let at = format!("k={}", "v".repeat(4088));
        assert!(matches!(merge(None, &[at]), Some(Ok(Some(_)))));
    }

    #[test]
    fn a_lone_surrogate_goes_to_the_shell() {
        assert_eq!(merge(Some(r#"{"k": "\ud83d"}"#), &[]), None);
    }
}
