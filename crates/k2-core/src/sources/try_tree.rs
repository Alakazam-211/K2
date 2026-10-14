//! The shape tree `POST /cli/sources/try` shows (contract §4.3). Owner:
//! B2.
//!
//! One node per value, depth-first in document order, each with its JSON
//! Pointer (RFC 6901, the path syntax `extract` and `rules` use), its type,
//! a length for arrays and objects, and a short sample for scalars. Arrays
//! show their first 3 elements. At most [`MAX_NODES`] nodes; the tree is
//! never stored.

use serde::Serialize;

use super::spec::Json;

/// At most this many nodes; arrays show their first 3 elements.
pub const MAX_NODES: usize = 500;
/// Elements shown per array.
pub const ARRAY_SHOWN: usize = 3;
/// Longest string sample, characters.
pub const SAMPLE_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeNode {
    pub pointer: String,
    /// `object` | `array` | `string` | `number` | `bool` | `null`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<Json>,
}

/// One pointer token, escaped (`~` → `~0`, `/` → `~1`).
pub fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn sample(v: &Json) -> Option<Json> {
    match v {
        Json::String(s) => {
            let short: String = s.chars().take(SAMPLE_CHARS).collect();
            Some(Json::String(if s.chars().count() > SAMPLE_CHARS { format!("{short}…") } else { short }))
        }
        Json::Number(_) | Json::Bool(_) => Some(v.clone()),
        _ => None,
    }
}

fn node(pointer: String, v: &Json) -> TreeNode {
    let (kind, len) = match v {
        Json::Object(m) => ("object", Some(m.len())),
        Json::Array(a) => ("array", Some(a.len())),
        Json::String(_) => ("string", None),
        Json::Number(_) => ("number", None),
        Json::Bool(_) => ("bool", None),
        Json::Null => ("null", None),
    };
    TreeNode { pointer, kind, len, sample: sample(v) }
}

/// `(nodes, truncated)`.
pub fn build(body: &Json) -> (Vec<TreeNode>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    // Explicit stack (deep bodies never recurse the thread's stack away).
    let mut stack: Vec<(String, &Json)> = vec![(String::new(), body)];
    while let Some((ptr, v)) = stack.pop() {
        if out.len() >= MAX_NODES {
            truncated = true;
            break;
        }
        out.push(node(ptr.clone(), v));
        match v {
            Json::Object(m) => {
                for (k, child) in m.iter().rev() {
                    stack.push((format!("{ptr}/{}", escape(k)), child));
                }
            }
            Json::Array(a) => {
                for (i, child) in a.iter().take(ARRAY_SHOWN).enumerate().rev() {
                    stack.push((format!("{ptr}/{i}"), child));
                }
            }
            _ => {}
        }
    }
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pointers_types_and_samples_in_document_order() {
        let body = json!({
            "deployments": [
                {"state": "READY", "uid": "d1"},
                {"state": "ERROR", "uid": "d2"},
                {"state": "BUILDING", "uid": "d3"},
                {"state": "READY", "uid": "d4"}
            ],
            "a/b": {"c~d": true},
            "n": null,
            "count": 4
        });
        let (nodes, cut) = build(&body);
        assert!(!cut);
        let ptrs: Vec<&str> = nodes.iter().map(|n| n.pointer.as_str()).collect();
        assert_eq!(ptrs[0], "");
        assert!(ptrs.contains(&"/deployments/2/state"));
        assert!(!ptrs.contains(&"/deployments/3"), "arrays show the first 3");
        assert!(ptrs.contains(&"/a~1b/c~0d"), "keys are escaped per RFC 6901");
        let dep = nodes.iter().find(|n| n.pointer == "/deployments").unwrap();
        assert_eq!((dep.kind, dep.len), ("array", Some(4)));
        let st = nodes.iter().find(|n| n.pointer == "/deployments/0/state").unwrap();
        assert_eq!((st.kind, st.sample.clone()), ("string", Some(json!("READY"))));
        assert_eq!(nodes.iter().find(|n| n.pointer == "/n").unwrap().kind, "null");
        assert_eq!(nodes.iter().find(|n| n.pointer == "/count").unwrap().sample, Some(json!(4)));
        // Every pointer resolves in the body.
        for n in &nodes {
            assert!(body.pointer(&n.pointer).is_some(), "{}", n.pointer);
        }
        // Depth-first: a parent comes right before its first child.
        let i = ptrs.iter().position(|p| *p == "/deployments/0").unwrap();
        assert_eq!(ptrs[i + 1], "/deployments/0/state");
    }

    #[test]
    fn caps_at_500_nodes_and_long_strings() {
        let big: serde_json::Map<String, Json> = (0..800).map(|i| (format!("k{i:04}"), json!("x".repeat(300)))).collect();
        let (nodes, cut) = build(&Json::Object(big));
        assert!(cut);
        assert_eq!(nodes.len(), MAX_NODES);
        let s = nodes[1].sample.as_ref().unwrap().as_str().unwrap();
        assert_eq!(s.chars().count(), SAMPLE_CHARS + 1);
        assert!(s.ends_with('…'));
        // A deep body doesn't overflow.
        let mut deep = json!(1);
        for _ in 0..1_000 {
            deep = json!([deep]);
        }
        let (n, cut) = build(&deep);
        assert!(cut && n.len() == MAX_NODES);
    }
}
