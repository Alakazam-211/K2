//! Rule evaluation (PRD §5.5): crit → warn → ok, else unknown. A missing
//! value (null) is unknown unless a rule asks `exists`.

use std::collections::BTreeMap;

use super::extract::Projection;
use super::spec::{Cond, Json, Rule, SourceSpec};
use super::state::Health;

fn json_eq(a: &Json, b: &Json) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => x == y,
        _ => a == b,
    }
}

/// Does `cond` hold for `v` (`None` = the field isn't there)?
pub fn cond_holds(cond: &Cond, v: Option<&Json>) -> bool {
    let present = v.is_some_and(|v| !v.is_null());
    match cond {
        Cond::Exists(want) => present == *want,
        _ if !present => false,
        Cond::Eq(x) => v.is_some_and(|v| json_eq(v, x)),
        Cond::Ne(x) => v.is_some_and(|v| !json_eq(v, x)),
        Cond::In(xs) => v.is_some_and(|v| xs.iter().any(|x| json_eq(v, x))),
        Cond::Lt(n) => v.and_then(Json::as_f64).is_some_and(|x| x < *n),
        Cond::Lte(n) => v.and_then(Json::as_f64).is_some_and(|x| x <= *n),
        Cond::Gt(n) => v.and_then(Json::as_f64).is_some_and(|x| x > *n),
        Cond::Gte(n) => v.and_then(Json::as_f64).is_some_and(|x| x >= *n),
    }
}

/// One rule on one value.
pub fn rate(rule: &Rule, v: Option<&Json>) -> Health {
    if rule.crit.as_ref().is_some_and(|c| cond_holds(c, v)) {
        Health::Crit
    } else if rule.warn.as_ref().is_some_and(|c| cond_holds(c, v)) {
        Health::Warn
    } else if rule.ok.as_ref().is_some_and(|c| cond_holds(c, v)) {
        Health::Ok
    } else {
        Health::Unknown
    }
}

/// Worst of several readings, unknown counted apart: the worst known
/// reading wins; all unknown (or none) is `default`.
pub fn worst(readings: impl IntoIterator<Item = Health>, default: Health) -> Health {
    let mut known: Option<Health> = None;
    let mut any_unknown = false;
    for h in readings {
        if h == Health::Unknown {
            any_unknown = true;
        } else {
            known = Some(known.map_or(h, |k| k.max(h)));
        }
    }
    match known {
        Some(h) => h,
        None if any_unknown => Health::Unknown,
        None => default,
    }
}

/// The item field names (from `extract.items.itemFields`).
fn item_field_names(spec: &SourceSpec) -> Vec<&str> {
    spec.extract.items.as_ref().map(|i| i.item_fields.keys().map(String::as_str).collect()).unwrap_or_default()
}

/// Rules that colour items.
pub fn item_rules(spec: &SourceSpec) -> Vec<&Rule> {
    let names = item_field_names(spec);
    spec.rules.iter().filter(|r| names.contains(&r.field.as_str())).collect()
}

/// Rules that colour the source (named fields and aggregates).
pub fn source_rules(spec: &SourceSpec) -> Vec<&Rule> {
    let names = item_field_names(spec);
    spec.rules.iter().filter(|r| !names.contains(&r.field.as_str())).collect()
}

/// Each item's reading. An item with no item rules reads ok (it was
/// fetched); with rules, the worst rule. Unknown when every rule is.
pub fn rate_items(spec: &SourceSpec, p: &Projection) -> BTreeMap<String, Health> {
    let rules = item_rules(spec);
    p.items
        .iter()
        .map(|it| {
            let h = if rules.is_empty() {
                Health::Ok
            } else {
                let hs: Vec<Health> = rules.iter().map(|r| rate(r, it.fields.get(&r.field))).collect();
                if hs.iter().all(|h| *h == Health::Unknown) {
                    Health::Unknown
                } else {
                    worst(hs, Health::Unknown)
                }
            };
            (it.key.clone(), h)
        })
        .collect()
}

/// The source-level reading from its own rules: `None` when it has none
/// (the fetch itself then reads ok).
pub fn rate_source(spec: &SourceSpec, p: &Projection) -> Option<Health> {
    let rules = source_rules(spec);
    if rules.is_empty() {
        return None;
    }
    let hs: Vec<Health> = rules.iter().map(|r| rate(r, p.fields.get(&r.field))).collect();
    Some(if hs.iter().all(|h| *h == Health::Unknown) { Health::Unknown } else { worst(hs, Health::Unknown) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(v: serde_json::Value) -> Rule {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn every_operator_and_the_crit_warn_ok_order() {
        let r = rule(json!({"field": "s", "crit": {"in": ["major_outage", "partial_outage"]},
                            "warn": {"in": ["degraded_performance"]}, "ok": {"eq": "operational"}}));
        assert_eq!(rate(&r, Some(&json!("partial_outage"))), Health::Crit);
        assert_eq!(rate(&r, Some(&json!("degraded_performance"))), Health::Warn);
        assert_eq!(rate(&r, Some(&json!("operational"))), Health::Ok);
        assert_eq!(rate(&r, Some(&json!("weird"))), Health::Unknown);
        assert_eq!(rate(&r, None), Health::Unknown);
        assert_eq!(rate(&r, Some(&Json::Null)), Health::Unknown);

        let days = rule(json!({"field": "d", "crit": {"lte": 7}, "warn": {"lte": 14}, "ok": {"gt": 14}}));
        assert_eq!(rate(&days, Some(&json!(7))), Health::Crit, "crit checked first");
        assert_eq!(rate(&days, Some(&json!(10.5))), Health::Warn);
        assert_eq!(rate(&days, Some(&json!(30))), Health::Ok);
        assert_eq!(rate(&days, Some(&json!("30"))), Health::Unknown, "strings aren't numbers");

        let up = rule(json!({"field": "up", "ok": {"eq": true}, "crit": {"eq": false}}));
        assert_eq!(rate(&up, Some(&json!(false))), Health::Crit);
        let n = rule(json!({"field": "n", "crit": {"gt": 0}, "ok": {"eq": 0}}));
        assert_eq!(rate(&n, Some(&json!(0.0))), Health::Ok, "0.0 == 0");
        assert_eq!(rate(&n, Some(&json!(3))), Health::Crit);
        assert!(cond_holds(&Cond::Lt(1.0), Some(&json!(0))));
        assert!(cond_holds(&Cond::Gte(1.0), Some(&json!(1))));
        assert!(cond_holds(&Cond::Ne(json!("a")), Some(&json!("b"))));
        // warn is checked before ok, so "anything but ready" is `ne`, not
        // `exists: true` (which would also hold for "ready").
        let ex = rule(json!({"field": "phase", "ok": {"eq": "ready"}, "warn": {"ne": "ready"}, "crit": {"exists": false}}));
        assert_eq!(rate(&ex, None), Health::Crit);
        assert_eq!(rate(&ex, Some(&json!("booting"))), Health::Warn);
        assert_eq!(rate(&ex, Some(&json!("ready"))), Health::Ok);
    }

    #[test]
    fn worst_counts_unknown_apart() {
        assert_eq!(worst([Health::Ok, Health::Unknown, Health::Ok], Health::Ok), Health::Ok);
        assert_eq!(worst([Health::Unknown, Health::Unknown], Health::Ok), Health::Unknown);
        assert_eq!(worst([Health::Warn, Health::Crit, Health::Unknown], Health::Ok), Health::Crit);
        assert_eq!(worst([], Health::Ok), Health::Ok);
    }
}
