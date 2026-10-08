//! Canonical JSON: object keys sorted by byte order, no whitespace,
//! serde_json's number and string escaping. Signatures and digests are
//! computed over these bytes, so the form must not depend on whether some
//! other crate in the build turned on serde_json's `preserve_order`.

use serde::Serialize;
use serde_json::Value;

/// Canonical bytes of any serializable value.
pub fn to_vec<T: Serialize>(v: &T) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(v).map_err(|e| format!("canonical: {e}"))?;
    let mut out = Vec::with_capacity(256);
    write_value(&value, &mut out)?;
    Ok(out)
}

/// Canonical string of any serializable value.
pub fn to_string<T: Serialize>(v: &T) -> Result<String, String> {
    let bytes = to_vec(v)?;
    String::from_utf8(bytes).map_err(|e| format!("canonical utf8: {e}"))
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
        scalar => {
            let s = serde_json::to_string(scalar).map_err(|e| format!("canonical scalar: {e}"))?;
            out.extend_from_slice(s.as_bytes());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sorted_at_every_level_and_no_whitespace() {
        let v = json!({"b": 1, "a": {"z": [3, {"y": null, "x": "s"}], "c": true}});
        assert_eq!(
            to_string(&v).unwrap(),
            r#"{"a":{"c":true,"z":[3,{"x":"s","y":null}]},"b":1}"#
        );
    }

    #[test]
    fn same_value_from_different_insertion_orders_is_identical() {
        let a: Value = serde_json::from_str(r#"{"k1":1,"k2":2,"k3":{"q":1,"p":2}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"k3":{"p":2,"q":1},"k2":2,"k1":1}"#).unwrap();
        assert_eq!(to_vec(&a).unwrap(), to_vec(&b).unwrap());
    }

    #[test]
    fn strings_escape_like_serde_json() {
        let v = json!({"s": "a\"b\n\u{1}é"});
        assert_eq!(to_string(&v).unwrap(), "{\"s\":\"a\\\"b\\n\\u0001é\"}");
    }
}
