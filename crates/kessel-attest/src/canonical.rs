//! Canonical JSON, the form every signed body is in: object keys sorted by
//! byte order, no whitespace, serde_json's number and string escaping, no
//! floats. A reader parses a body, writes it back in this form and refuses
//! it unless the bytes are the same, so one document has one spelling.

use serde::Serialize;
use serde_json::Value;

/// Canonical bytes of any serializable value.
pub fn to_vec<T: Serialize>(v: &T) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(v).map_err(|e| format!("canonical: {e}"))?;
    let mut out = Vec::with_capacity(256);
    write_value(&value, &mut out)?;
    Ok(out)
}

fn write_value(v: &Value, out: &mut Vec<u8>) -> Result<(), String> {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                let ks = serde_json::to_string(k).map_err(|e| format!("canonical key: {e}"))?;
                out.extend_from_slice(ks.as_bytes());
                out.push(b':');
                write_value(&map[k.as_str()], out)?;
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(item, out)?;
            }
            out.push(b']');
        }
        Value::Number(n) if !(n.is_i64() || n.is_u64()) => return Err("canonical: a float".into()),
        scalar => {
            let s = serde_json::to_string(scalar).map_err(|e| format!("canonical scalar: {e}"))?;
            out.extend_from_slice(s.as_bytes());
        }
    }
    Ok(())
}

/// Parse `body` as `T` (unknown fields refused by `T`), and refuse it unless
/// `body` is exactly `T`'s canonical form.
pub fn parse_exact<T: Serialize + serde::de::DeserializeOwned>(what: &str, body: &[u8]) -> Result<T, String> {
    let doc: T = serde_json::from_slice(body).map_err(|e| format!("{what}: {e}"))?;
    if to_vec(&doc)? != body {
        return Err(format!("{what}: the body is not its canonical form"));
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sorted_at_every_level_and_no_whitespace() {
        let v = json!({"b": 1, "a": {"z": [3, {"y": null, "x": "s"}], "c": true}});
        assert_eq!(String::from_utf8(to_vec(&v).unwrap()).unwrap(), r#"{"a":{"c":true,"z":[3,{"x":"s","y":null}]},"b":1}"#);
    }

    #[test]
    fn strings_escape_like_serde_json_and_floats_are_refused() {
        let v = json!({"s": "a\"b\n\u{1}é"});
        assert_eq!(String::from_utf8(to_vec(&v).unwrap()).unwrap(), "{\"s\":\"a\\\"b\\n\\u0001é\"}");
        assert!(to_vec(&json!({"f": 1.5})).is_err());
    }

    #[test]
    fn parse_exact_refuses_another_spelling() {
        let ok: Value = parse_exact("doc", br#"{"a":1,"b":[]}"#).unwrap();
        assert_eq!(ok, json!({"a": 1, "b": []}));
        assert!(parse_exact::<Value>("doc", br#"{"b":[],"a":1}"#).unwrap_err().contains("canonical"));
        assert!(parse_exact::<Value>("doc", br#"{"a": 1,"b":[]}"#).unwrap_err().contains("canonical"));
    }
}
