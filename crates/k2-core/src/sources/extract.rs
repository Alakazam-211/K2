//! JSON Pointer projection and aggregates (PRD §5.4). Only the projection
//! is ever kept; raw bodies never are. Values kept: strings (<= 200
//! characters), numbers, booleans, null. Arrays and objects become null
//! (name them in `extract.counts` to keep their length instead).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::spec::{Aggregate, Extract, ItemsSpec, Json};
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
    /// The `extract.lists` entry this item came from (`None` = `items`).
    /// Its key is then `<list>/<itemKey>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<String>,
}

impl Projection {
    pub fn item(&self, key: &str) -> Option<&ProjectedItem> {
        self.items.iter().find(|i| i.key == key)
    }

    /// The same keys with every value dropped (`keep: "states"`).
    pub fn keys_only(&self) -> Projection {
        Projection {
            fields: BTreeMap::new(),
            items: self
                .items
                .iter()
                .map(|i| ProjectedItem { key: i.key.clone(), fields: BTreeMap::new(), list: i.list.clone() })
                .collect(),
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

fn item_fields(spec: Option<&ItemsSpec>, item: Option<&Json>) -> BTreeMap<String, Json> {
    let mut out = BTreeMap::new();
    if let Some(spec) = spec {
        for (name, ptr) in &spec.item_fields {
            out.insert(name.clone(), keep_value(item.and_then(|i| i.pointer(ptr))));
        }
    }
    out
}

/// The length of the array or object at `ptr`, else null.
fn count_at(body: &Json, ptr: &str) -> Json {
    match body.pointer(ptr) {
        Some(Json::Array(a)) => Json::from(a.len()),
        Some(Json::Object(m)) => Json::from(m.len()),
        _ => Json::Null,
    }
}

/// Push one list's items (`list` = None for `extract.items`).
fn push_list(spec: &ItemsSpec, list: Option<&str>, body: &Json, p: &mut Projection) {
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
        let key = match list {
            Some(l) => format!("{l}/{key}").chars().take(200).collect(),
            None => key,
        };
        p.items.push(ProjectedItem { key, fields: item_fields(Some(spec), Some(item)), list: list.map(str::to_string) });
    };
    match body.pointer(ptr) {
        Some(Json::Array(a)) => {
            for (i, item) in a.iter().enumerate() {
                push(i.to_string(), item, p);
            }
        }
        Some(Json::Object(m)) => {
            for (k, item) in m {
                push(k.chars().take(200).collect(), item, p);
            }
        }
        _ => {}
    }
}

/// A single-URL response: named fields and counts from the body, items
/// from the items pointer (an array, or an object whose entries are items
/// keyed by property name), then each named list, then aggregates (`now`
/// for age conditions in `countWhere`).
pub fn extract_single(ex: &Extract, body: &Json, now: i64) -> Projection {
    let mut p = Projection::default();
    for (name, ptr) in &ex.fields {
        p.fields.insert(name.clone(), keep_value(body.pointer(ptr)));
    }
    for (name, ptr) in &ex.counts {
        p.fields.insert(name.clone(), count_at(body, ptr));
    }
    if let Some(spec) = &ex.items {
        push_list(spec, None, body, &mut p);
    }
    for (name, spec) in &ex.lists {
        push_list(spec, Some(name), body, &mut p);
    }
    add_aggregates(ex, &mut p, now);
    p
}

/// Fan-out: one item per target, in target order. `None` = that target
/// gave no body this cycle (its item has no fields). Named fields are not
/// used with fan-out; aggregates run over the items.
pub fn extract_fanout(ex: &Extract, bodies: &[(String, Option<&Json>)], now: i64) -> Projection {
    let mut p = Projection::default();
    for (key, body) in bodies.iter().take(MAX_ITEMS) {
        p.items.push(ProjectedItem { key: key.clone(), fields: item_fields(ex.items.as_ref(), *body), list: None });
    }
    add_aggregates(ex, &mut p, now);
    p
}

fn num(v: &Json) -> Option<f64> {
    v.as_f64()
}

/// (Re)compute the aggregates over `p`'s items into `p.fields`. A
/// `countWhere` condition's `{"field": ...}` reads the item, then the
/// named fields and counts; `now` serves age conditions.
pub fn add_aggregates(ex: &Extract, p: &mut Projection, now: i64) {
    let base: BTreeMap<String, Json> = p.fields.iter().filter(|(k, _)| !ex.aggregates.contains_key(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    for (name, agg) in &ex.aggregates {
        let vals = |f: &str| -> Vec<&Json> { p.items.iter().filter_map(|i| i.fields.get(f)).collect() };
        let v = match agg {
            Aggregate::Count { list } => Json::from(p.items.iter().filter(|i| i.list.as_deref() == list.as_deref()).count()),
            Aggregate::CountWhere { field, cond } => Json::from(
                p.items
                    .iter()
                    .filter_map(|i| i.fields.get(field.as_str()).map(|v| (i, v)))
                    .filter(|(i, v)| {
                        let ctx = super::rules::Ctx { now, item: Some(&i.fields), source: Some(&base) };
                        super::rules::cond_holds(cond, Some(v), &ctx)
                    })
                    .count(),
            ),
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
    use crate::sources::spec::Cond;
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
        let p = extract_single(&e, &body, 0);
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
        let p = extract_single(&e, &obj, 0);
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.item("ohana").unwrap().fields["v"], json!("0.45.5"));
    }

    #[test]
    fn caps_on_items_strings_and_bytes() {
        let arr: Vec<_> = (0..250).map(|i| json!({"id": i, "s": "x".repeat(500), "t": "y".repeat(500), "u": "z".repeat(500)})).collect();
        let e = ex(json!({"items": {"pointer": "", "itemKey": "/id", "itemFields": {"s": "/s", "t": "/t", "u": "/u"}}}));
        let p = extract_single(&e, &Json::Array(arr), 0);
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
        let p = extract_fanout(&e, &[("a".into(), Some(&a)), ("b".into(), Some(&b)), ("c".into(), None)], 0);
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.items[2].fields["version"], Json::Null);
        assert_eq!(p.fields["versions"], json!(2));
        assert_eq!(p.keys_only().items[0].fields.len(), 0);
        let _ = Cond::Exists(true);
    }

    #[test]
    fn named_lists_counts_and_list_scoped_aggregates() {
        let e = ex(json!({
            "fields": {"target_version": "/target_version"},
            "counts": {"past_due_n": "/past_due", "boxes_n": "/boxes", "nope_n": "/target_version"},
            "items": {"pointer": "/boxes", "itemKey": "/name", "itemFields": {"version": "/version"}},
            "lists": {
                "certs": {"pointer": "/certs", "itemKey": "/host", "itemFields": {"days_left": "/days"}},
                "backups": {"pointer": "/backups", "itemFields": {"backup_ts": "/ts"}}
            },
            "aggregates": {
                "boxes": {"op": "count"},
                "certs_n": {"op": "count", "list": "certs"},
                "behind": {"op": "countWhere", "field": "version", "cond": {"versionBehind": {"field": "target_version"}}},
                "stale_backups": {"op": "countWhere", "field": "backup_ts", "cond": {"ageGt": 86400}}
            }
        }));
        let now = 1_792_200_000;
        let body = json!({
            "target_version": "0.46.0",
            "past_due": [{"id": 1}, {"id": 2}],
            "boxes": [{"name": "a", "version": "0.46.0"}, {"name": "b", "version": "0.45.7"}, {"name": "c", "version": "0.45.5"}],
            "certs": [{"host": "x.k2.dev", "days": 40}, {"host": "y.k2.dev", "days": 3}],
            "backups": [{"ts": now - 100}, {"ts": now - 200_000}]
        });
        let p = extract_single(&e, &body, now);
        assert_eq!(p.fields["past_due_n"], json!(2), "an array kept as its length");
        assert_eq!(p.fields["boxes_n"], json!(3));
        assert_eq!(p.fields["nope_n"], Json::Null, "a string has no length");
        assert_eq!(p.fields["boxes"], json!(3), "count is items only, not lists");
        assert_eq!(p.fields["certs_n"], json!(2));
        assert_eq!(p.fields["behind"], json!(2), "versionBehind against a source field");
        assert_eq!(p.fields["stale_backups"], json!(1), "ageGt against now");
        let keys: Vec<&str> = p.items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b", "c", "backups/0", "backups/1", "certs/x.k2.dev", "certs/y.k2.dev"]);
        let cert = p.item("certs/y.k2.dev").unwrap();
        assert_eq!(cert.list.as_deref(), Some("certs"));
        assert_eq!(cert.fields.keys().collect::<Vec<_>>(), vec!["days_left"], "a list item has only its list's fields");
        assert_eq!(p.item("a").unwrap().list, None);
        let back: Projection = serde_json::from_str(&p.to_stored_json()).unwrap();
        assert_eq!(back, p, "lists survive the stored JSON");
        assert!(!serde_json::to_string(&p.items[0]).unwrap().contains("list"), "items without a list store as before");
    }
}
