//! JSON Pointer projection and aggregates (PRD §5.4). Only the projection
//! is ever kept; raw bodies never are. Values kept: strings (<= 200
//! characters), numbers, booleans, null. Arrays and objects become null.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::spec::{Aggregate, Extract, Json};
use super::{MAX_ITEMS, MAX_PROJECTION_BYTES, MAX_STRING};

/// What one poll kept from the response: source-level fields (named
/// fields and aggregates) and per-item fields, in item order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Projection {
    #[serde(default)]
    pub fields: BTreeMap<String, Json>,
    /// <= `MAX_ITEMS`.
    #[serde(default)]
    pub items: Vec<ProjectedItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedItem {
    pub key: String,
    #[serde(default)]
    pub fields: BTreeMap<String, Json>,
}

impl Projection {
    pub fn item(&self, key: &str) -> Option<&ProjectedItem> {
        self.items.iter().find(|i| i.key == key)
    }

    /// The same keys with every value dropped (`keep: "states"`).
    pub fn keys_only(&self) -> Projection {
        Projection {
            fields: BTreeMap::new(),
            items: self.items.iter().map(|i| ProjectedItem { key: i.key.clone(), fields: BTreeMap::new() }).collect(),
        }
    }

    /// Stored JSON, cut to `MAX_PROJECTION_BYTES` by dropping items from
    /// the end (and then fields) until it fits.
    pub fn to_stored_json(&self) -> String {
        let mut p = self.clone();
        loop {
            let s = serde_json::to_string(&p).unwrap_or_else(|_| "{}".into());
            if s.len() <= MAX_PROJECTION_BYTES {
                return s;
            }
            if p.items.pop().is_none() && p.fields.pop_last().is_none() {
                return "{}".into();
            }
        }
    }
}

/// A kept scalar: long strings cut at `MAX_STRING` characters; arrays and
/// objects become null.
pub fn keep_value(v: Option<&Json>) -> Json {
    match v {
        Some(Json::String(s)) => {
            if s.chars().count() > MAX_STRING {
                Json::String(s.chars().take(MAX_STRING).collect())
            } else {
                Json::String(s.clone())
            }
        }
        Some(Json::Number(n)) => Json::Number(n.clone()),
        Some(Json::Bool(b)) => Json::Bool(*b),
        _ => Json::Null,
    }
}

fn key_of(v: Option<&Json>) -> Option<String> {
    let s = match v? {
        Json::String(s) => s.clone(),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => b.to_string(),
        _ => return None,
    };
    let s: String = s.chars().take(200).collect();
    (!s.is_empty()).then_some(s)
}

fn item_fields(ex: &Extract, item: Option<&Json>) -> BTreeMap<String, Json> {
    let mut out = BTreeMap::new();
    if let Some(spec) = &ex.items {
        for (name, ptr) in &spec.item_fields {
            out.insert(name.clone(), keep_value(item.and_then(|i| i.pointer(ptr))));
        }
    }
    out
}

/// A single-URL response: named fields from the body, items from the
/// items pointer (an array, or an object whose entries are items keyed by
/// property name), then aggregates.
pub fn extract_single(ex: &Extract, body: &Json) -> Projection {
    let mut p = Projection::default();
    for (name, ptr) in &ex.fields {
        p.fields.insert(name.clone(), keep_value(body.pointer(ptr)));
    }
    if let Some(spec) = &ex.items {
        let ptr = spec.pointer.as_deref().unwrap_or("");
        let mut seen = BTreeSet::new();
        let mut push = |fallback: String, item: &Json, p: &mut Projection| {
            if p.items.len() >= MAX_ITEMS {
                return;
            }
            let key = match &spec.item_key {
                Some(k) => key_of(item.pointer(k)).unwrap_or(fallback),
                None => fallback,
            };
            if !seen.insert(key.clone()) {
                return;
            }
            p.items.push(ProjectedItem { key, fields: item_fields(ex, Some(item)) });
        };
        match body.pointer(ptr) {
            Some(Json::Array(a)) => {
                for (i, item) in a.iter().enumerate() {
                    push(i.to_string(), item, &mut p);
                }
            }
            Some(Json::Object(m)) => {
                for (k, item) in m {
                    push(k.chars().take(200).collect(), item, &mut p);
                }
            }
            _ => {}
        }
    }
    add_aggregates(ex, &mut p);
    p
}

/// Fan-out: one item per target, in target order. `None` = that target
/// gave no body this cycle (its item has no fields). Named fields are not
/// used with fan-out; aggregates run over the items.
pub fn extract_fanout(ex: &Extract, bodies: &[(String, Option<&Json>)]) -> Projection {
    let mut p = Projection::default();
    for (key, body) in bodies.iter().take(MAX_ITEMS) {
        p.items.push(ProjectedItem { key: key.clone(), fields: item_fields(ex, *body) });
    }
    add_aggregates(ex, &mut p);
    p
}

fn num(v: &Json) -> Option<f64> {
    v.as_f64()
}

/// (Re)compute the aggregates over `p`'s items into `p.fields`.
pub fn add_aggregates(ex: &Extract, p: &mut Projection) {
    for (name, agg) in &ex.aggregates {
        let vals = |f: &str| -> Vec<&Json> { p.items.iter().filter_map(|i| i.fields.get(f)).collect() };
        let v = match agg {
            Aggregate::Count => Json::from(p.items.len()),
            Aggregate::CountWhere { field, cond } => {
                Json::from(vals(field).into_iter().filter(|v| super::rules::cond_holds(cond, Some(v))).count())
            }
            Aggregate::Sum { field } => {
                let ns: Vec<f64> = vals(field).into_iter().filter_map(num).collect();
                if ns.is_empty() {
                    Json::Null
                } else {
                    number(ns.iter().sum())
                }
            }
            Aggregate::Min { field } => vals(field).into_iter().filter_map(num).reduce(f64::min).map(number).unwrap_or(Json::Null),
            Aggregate::Max { field } => vals(field).into_iter().filter_map(num).reduce(f64::max).map(number).unwrap_or(Json::Null),
            Aggregate::Distinct { field } => {
                let set: BTreeSet<String> =
                    vals(field).into_iter().filter(|v| !v.is_null()).map(|v| v.to_string()).collect();
                Json::from(set.len())
            }
        };
        p.fields.insert(name.clone(), v);
    }
}

/// A whole number stays an integer in JSON.
fn number(x: f64) -> Json {
    if x.fract() == 0.0 && x.abs() < 9.0e15 {
        Json::from(x as i64)
    } else {
        serde_json::Number::from_f64(x).map(Json::Number).unwrap_or(Json::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::spec::{Cond, ItemsSpec};
    use serde_json::json;

    fn ex(v: serde_json::Value) -> Extract {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn fields_items_from_array_and_object_and_aggregates() {
        let e = ex(json!({
            "fields": {"indicator": "/status/indicator", "missing": "/nope", "obj": "/status"},
            "items": {"pointer": "/components", "itemKey": "/id", "itemFields": {"status": "/status", "n": "/n"}},
            "aggregates": {
                "count": {"op": "count"},
                "down": {"op": "countWhere", "field": "status", "cond": {"ne": "operational"}},
                "total": {"op": "sum", "field": "n"},
                "lo": {"op": "min", "field": "n"},
                "hi": {"op": "max", "field": "n"},
                "kinds": {"op": "distinct", "field": "status"}
            }
        }));
        let body = json!({
            "status": {"indicator": "minor"},
            "components": [
                {"id": "api", "status": "operational", "n": 2},
                {"id": "web", "status": "degraded", "n": 3.5},
                {"id": "api", "status": "dup", "n": 100},
                {"status": "operational"}
            ]
        });
        let p = extract_single(&e, &body);
        assert_eq!(p.fields["indicator"], json!("minor"));
        assert_eq!(p.fields["missing"], Json::Null);
        assert_eq!(p.fields["obj"], Json::Null, "objects aren't kept");
        let keys: Vec<&str> = p.items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["api", "web", "3"], "dup key dropped; no key → index");
        assert_eq!(p.fields["count"], json!(3));
        assert_eq!(p.fields["down"], json!(1));
        assert_eq!(p.fields["total"], json!(5.5));
        assert_eq!(p.fields["lo"], json!(2));
        assert_eq!(p.fields["hi"], json!(3.5));
        assert_eq!(p.fields["kinds"], json!(2));

        let obj = json!({"boxes": {"ohana": {"v": "0.45.5"}, "luzz": {"v": "0.45.6"}}});
        let e = Extract {
            items: Some(ItemsSpec {
                pointer: Some("/boxes".into()),
                item_key: None,
                item_fields: [("v".to_string(), "/v".to_string())].into(),
            }),
            ..Default::default()
        };
        let p = extract_single(&e, &obj);
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.item("ohana").unwrap().fields["v"], json!("0.45.5"));
    }

    #[test]
    fn caps_on_items_strings_and_bytes() {
        let arr: Vec<_> = (0..250).map(|i| json!({"id": i, "s": "x".repeat(500), "t": "y".repeat(500), "u": "z".repeat(500)})).collect();
        let e = ex(json!({"items": {"pointer": "", "itemKey": "/id", "itemFields": {"s": "/s", "t": "/t", "u": "/u"}}}));
        let p = extract_single(&e, &Json::Array(arr));
        assert_eq!(p.items.len(), MAX_ITEMS);
        assert_eq!(p.items[0].fields["s"].as_str().unwrap().chars().count(), MAX_STRING);
        let stored = p.to_stored_json();
        assert!(stored.len() <= MAX_PROJECTION_BYTES);
        let back: Projection = serde_json::from_str(&stored).unwrap();
        assert!(back.items.len() < MAX_ITEMS && !back.items.is_empty(), "items dropped from the end to fit");
    }

    #[test]
    fn fanout_items_follow_targets_and_failed_targets_have_no_fields() {
        let e = ex(json!({"items": {"itemFields": {"version": "/version", "phase": "/phase"}},
                          "aggregates": {"versions": {"op": "distinct", "field": "version"}}}));
        let a = json!({"version": "0.45.5", "phase": "ready"});
        let b = json!({"version": "0.45.6", "phase": "ready"});
        let p = extract_fanout(&e, &[("a".into(), Some(&a)), ("b".into(), Some(&b)), ("c".into(), None)]);
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.items[2].fields["version"], Json::Null);
        assert_eq!(p.fields["versions"], json!(2));
        assert_eq!(p.keys_only().items[0].fields.len(), 0);
        let _ = Cond::Exists(true);
    }
}
