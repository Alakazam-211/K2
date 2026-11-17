//! Shared helpers for the rows: index a drawing, run a read verb, read typed data.
#![allow(dead_code)]

use k2_mpc_base::errors::Value;
use k2_mpc_base::index::{DraftNode, IndexDraft, IndexLimits};
use k2_mpc_dxf::verbs::{self, ReadOut};
use k2_mpc_dxf::CadError;

/// The kit's default index limits, as a host sets them.
pub fn limits() -> IndexLimits {
    IndexLimits::new(100_000, 64, 64 << 20, 64 << 10)
}

/// The index draft of `bytes`.
///
/// # Panics
/// When the pack refuses the drawing.
pub fn draft(bytes: &[u8]) -> IndexDraft {
    match k2_mpc_dxf::pack::index_bytes(bytes, &limits()) {
        Ok(d) => d,
        Err(e) => panic!("the pack refused the drawing: {e}"),
    }
}

/// The node keyed `key`.
///
/// # Panics
/// When no node has that key.
pub fn node<'d>(d: &'d IndexDraft, key: &str) -> &'d DraftNode {
    d.nodes
        .iter()
        .find(|n| n.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no node keyed {key}"))
}

/// A node's attribute, or "".
pub fn attr<'n>(n: &'n DraftNode, k: &str) -> &'n str {
    n.attrs.get(k).map(String::as_str).unwrap_or("")
}

/// A read verb's output, or the panic naming its refusal.
pub fn run(bytes: &[u8], verb: &str, args: &[&str]) -> ReadOut {
    match verbs::run(bytes, verb, args) {
        Ok(o) => o,
        Err(e) => panic!("{verb} refused: {e}"),
    }
}

/// A read verb's refusal.
///
/// # Panics
/// When it succeeds.
pub fn refused(bytes: &[u8], verb: &str, args: &[&str]) -> CadError {
    match verbs::run(bytes, verb, args) {
        Ok(o) => panic!("{verb} {args:?} succeeded: {}", o.stdout),
        Err(e) => e,
    }
}

/// `v[k1][k2]…`, or `Value::Null`.
pub fn at<'v>(v: &'v Value, path: &[&str]) -> &'v Value {
    static NULL: Value = Value::Null;
    let mut cur = v;
    for k in path {
        cur = cur.get(k).unwrap_or(&NULL);
    }
    cur
}

/// The list at `v[path]`.
///
/// # Panics
/// When it isn't a list.
pub fn list<'v>(v: &'v Value, path: &[&str]) -> &'v [Value] {
    match at(v, path) {
        Value::List(l) => l,
        other => panic!("{path:?} is not a list: {}", other.to_json()),
    }
}

/// The number at `v[path]`.
///
/// # Panics
/// When it isn't a number.
pub fn number(v: &Value, path: &[&str]) -> f64 {
    at(v, path)
        .as_f64()
        .unwrap_or_else(|| panic!("{path:?} is not a number: {}", at(v, path).to_json()))
}

/// The string at `v[path]`, or "".
pub fn string<'v>(v: &'v Value, path: &[&str]) -> &'v str {
    at(v, path).as_str().unwrap_or("")
}

/// The row of `rows` whose `key` is `want`.
///
/// # Panics
/// When none is.
pub fn row<'v>(rows: &'v [Value], key: &str, want: &str) -> &'v Value {
    rows.iter()
        .find(|r| string(r, &[key]) == want)
        .unwrap_or_else(|| panic!("no row with {key} = {want}"))
}

/// The four numbers of a box attribute (`x0,y0,x1,y1`).
///
/// # Panics
/// When it isn't one.
pub fn bbox(s: &str) -> [f64; 4] {
    let v: Vec<f64> = s
        .split(',')
        .map(|x| {
            x.parse::<f64>()
                .unwrap_or_else(|_| panic!("not a box: {s:?}"))
        })
        .collect();
    assert_eq!(v.len(), 4, "not a box: {s:?}");
    [v[0], v[1], v[2], v[3]]
}

/// Asserts two boxes are equal within 1e-9.
pub fn same_box(got: [f64; 4], want: [f64; 4], what: &str) {
    for i in 0..4 {
        assert!(
            (got[i] - want[i]).abs() < 1e-9,
            "{what}: box {got:?}, expected {want:?}"
        );
    }
}
