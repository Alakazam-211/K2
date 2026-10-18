//! Rule evaluation (PRD §5.5): crit → warn → ok, else unknown. A missing
//! value (null) is unknown unless a rule asks `exists`.
//!
//! 0.46.1: conditions can compare to another field (`{"field": ...}`), test
//! a timestamp's age (`ageGt`/`ageLt`) or a version (`versionBehind`/
//! `versionAtLeast`), and each rule says how many checks in a row its warn
//! and crit need (`warnAfter`/`critAfter`).

use std::collections::BTreeMap;

use super::extract::Projection;
use super::spec::{json_field_ref, Cond, Json, Operand, Rule, SourceSpec, VersionRef};
use super::state::Health;

/// What a condition can look at besides its own value: the clock (age
/// rules) and the fields a `{"field": ...}` reference names (the item's
/// own fields first, then the source's fields).
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub now: i64,
    pub item: Option<&'a BTreeMap<String, Json>>,
    pub source: Option<&'a BTreeMap<String, Json>>,
}

impl<'a> Ctx<'a> {
    /// No references and no clock (literal conditions only).
    pub fn bare() -> Ctx<'static> {
        Ctx { now: 0, item: None, source: None }
    }

    pub fn lookup(&self, field: &str) -> Option<&'a Json> {
        self.item.and_then(|m| m.get(field)).or_else(|| self.source.and_then(|m| m.get(field)))
    }
}

fn json_eq(a: &Json, b: &Json) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => x == y,
        _ => a == b,
    }
}

/// A time as epoch seconds: a number (seconds, or milliseconds when it is
/// above 1e11), a numeric string, or an RFC 3339 / ISO 8601 string (UTC
/// when it has no offset).
pub fn parse_time(v: &Json) -> Option<i64> {
    let secs = |x: f64| if x.abs() > 1.0e11 { (x / 1000.0) as i64 } else { x as i64 };
    match v {
        Json::Number(n) => n.as_f64().filter(|x| x.is_finite()).map(secs),
        Json::String(s) => {
            let t = s.trim();
            if let Ok(x) = t.parse::<f64>() {
                return x.is_finite().then(|| secs(x));
            }
            if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
                return Some(d.timestamp());
            }
            let t2 = t.trim_end_matches('Z').replace(' ', "T");
            chrono::NaiveDateTime::parse_from_str(&t2, "%Y-%m-%dT%H:%M:%S%.f").ok().map(|d| d.and_utc().timestamp())
        }
        _ => None,
    }
}

/// A version as `(major, minor, patch, is_release)`: a leading `v` is
/// dropped, missing parts are 0, and a pre-release (`-rc1`) sorts before
/// its release. Build metadata (`+…`) is ignored.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64, bool)> {
    let s = s.trim();
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    let s = s.split('+').next()?;
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, !p.is_empty()),
        None => (s, false),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let n = |i: usize| parts.get(i).map_or(Some(0), |p| p.parse::<u64>().ok());
    Some((n(0)?, n(1)?, n(2)?, !pre))
}

fn json_version(v: &Json) -> Option<(u64, u64, u64, bool)> {
    match v {
        Json::String(s) => parse_version(s),
        Json::Number(n) => parse_version(&n.to_string()),
        _ => None,
    }
}

fn operand(o: &Operand, ctx: &Ctx) -> Option<f64> {
    match o {
        Operand::Num(n) => Some(*n),
        Operand::Field(f) => ctx.lookup(&f.field).and_then(Json::as_f64),
    }
}

/// Is `v` behind `t` by at least the reference's distance?
fn version_behind(v: (u64, u64, u64, bool), t: (u64, u64, u64, bool), r: &VersionRef) -> bool {
    let (major, minor, patch) = match r {
        VersionRef::Spec(s) => (s.major, s.minor, s.patch),
        VersionRef::Value(_) => (None, None, None),
    };
    if let Some(n) = major {
        return t.0 >= v.0 + u64::from(n.max(1));
    }
    if let Some(n) = minor {
        return t.0 > v.0 || (t.0 == v.0 && t.1 >= v.1 + u64::from(n.max(1)));
    }
    if let Some(n) = patch {
        return (t.0, t.1) > (v.0, v.1) || ((t.0, t.1) == (v.0, v.1) && t.2 >= v.2 + u64::from(n.max(1)));
    }
    v < t
}

fn version_target(r: &VersionRef, ctx: &Ctx) -> Option<(u64, u64, u64, bool)> {
    match r {
        VersionRef::Value(s) => parse_version(s),
        VersionRef::Spec(s) => match (&s.value, &s.field) {
            (Some(v), _) => parse_version(v),
            (None, Some(f)) => ctx.lookup(f).and_then(json_version),
            (None, None) => None,
        },
    }
}

/// Does `cond` hold for `v` (`None` = the field isn't there)? A reference
/// to a missing field, or an unreadable time or version, never holds.
pub fn cond_holds(cond: &Cond, v: Option<&Json>, ctx: &Ctx) -> bool {
    let present = v.is_some_and(|v| !v.is_null());
    let val = v.and_then(Json::as_f64);
    let cmp = |o: &Operand, f: fn(f64, f64) -> bool| matches!((val, operand(o, ctx)), (Some(x), Some(n)) if f(x, n));
    let field_eq = |x: &Json, want: bool| match json_field_ref(x) {
        Some(f) => match (v, ctx.lookup(f)) {
            (Some(a), Some(b)) if !b.is_null() => json_eq(a, b) == want,
            _ => false,
        },
        None => v.is_some_and(|v| json_eq(v, x) == want),
    };
    match cond {
        Cond::Exists(want) => present == *want,
        _ if !present => false,
        Cond::Eq(x) => field_eq(x, true),
        Cond::Ne(x) => field_eq(x, false),
        Cond::In(xs) => v.is_some_and(|v| xs.iter().any(|x| json_eq(v, x))),
        Cond::Lt(o) => cmp(o, |x, n| x < n),
        Cond::Lte(o) => cmp(o, |x, n| x <= n),
        Cond::Gt(o) => cmp(o, |x, n| x > n),
        Cond::Gte(o) => cmp(o, |x, n| x >= n),
        Cond::AgeGt(n) => v.and_then(parse_time).is_some_and(|t| (ctx.now - t) as f64 > *n),
        Cond::AgeLt(n) => v.and_then(parse_time).is_some_and(|t| ((ctx.now - t) as f64) < *n),
        Cond::VersionBehind(r) => match (v.and_then(json_version), version_target(r, ctx)) {
            (Some(x), Some(t)) => version_behind(x, t, r),
            _ => false,
        },
        Cond::VersionAtLeast(r) => match (v.and_then(json_version), version_target(r, ctx)) {
            (Some(x), Some(t)) => x >= t,
            _ => false,
        },
    }
}

/// One rule on one value.
pub fn rate(rule: &Rule, v: Option<&Json>, ctx: &Ctx) -> Health {
    if rule.crit.as_ref().is_some_and(|c| cond_holds(c, v, ctx)) {
        Health::Crit
    } else if rule.warn.as_ref().is_some_and(|c| cond_holds(c, v, ctx)) {
        Health::Warn
    } else if rule.ok.as_ref().is_some_and(|c| cond_holds(c, v, ctx)) {
        Health::Ok
    } else {
        Health::Unknown
    }
}

/// A rated value and how many checks in a row its warn and crit need
/// before they go hard (from the rules that produced it; 1 = at once).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rated {
    pub health: Health,
    pub warn_after: u32,
    pub crit_after: u32,
}

impl Rated {
    /// No thresholds: hard at once.
    pub fn now(health: Health) -> Rated {
        Rated { health, warn_after: 1, crit_after: 1 }
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

/// Combine `(rule, reading)` pairs: the worst reading (unknown when all
/// are), warn after the fewest checks any warn-or-crit rule asks, crit
/// after the fewest any crit rule asks.
fn combine(rs: &[(&Rule, Health)]) -> Rated {
    let health = if rs.iter().all(|(_, h)| *h == Health::Unknown) {
        Health::Unknown
    } else {
        worst(rs.iter().map(|(_, h)| *h), Health::Unknown)
    };
    let warn_after = rs.iter().filter(|(_, h)| *h >= Health::Warn).map(|(r, _)| r.warn_after()).min().unwrap_or(1);
    let crit_after = rs.iter().filter(|(_, h)| *h == Health::Crit).map(|(r, _)| r.crit_after()).min().unwrap_or(1);
    Rated { health, warn_after, crit_after }
}

/// Item field name -> its list (`None` = `extract.items`).
pub fn item_field_lists(spec: &SourceSpec) -> BTreeMap<&str, Option<&str>> {
    let mut m: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    if let Some(i) = &spec.extract.items {
        for n in i.item_fields.keys() {
            m.insert(n, None);
        }
    }
    for (ln, l) in &spec.extract.lists {
        for n in l.item_fields.keys() {
            m.insert(n, Some(ln));
        }
    }
    m
}

/// Rules that colour items (of any list).
pub fn item_rules(spec: &SourceSpec) -> Vec<&Rule> {
    let names = item_field_lists(spec);
    spec.rules.iter().filter(|r| names.contains_key(r.field.as_str())).collect()
}

/// Rules that colour the source (named fields, counts and aggregates).
pub fn source_rules(spec: &SourceSpec) -> Vec<&Rule> {
    let names = item_field_lists(spec);
    spec.rules.iter().filter(|r| !names.contains_key(r.field.as_str())).collect()
}

/// Each item's reading. An item with no rules for its list reads ok (it
/// was fetched); with rules, the worst rule. Unknown when every rule is.
pub fn rate_items(spec: &SourceSpec, p: &Projection, now: i64) -> BTreeMap<String, Rated> {
    let lists = item_field_lists(spec);
    let rules = item_rules(spec);
    p.items
        .iter()
        .map(|it| {
            let own = it.list.as_deref();
            let mine: Vec<&Rule> =
                rules.iter().copied().filter(|r| lists.get(r.field.as_str()).copied().flatten() == own).collect();
            let ctx = Ctx { now, item: Some(&it.fields), source: Some(&p.fields) };
            let rated = if mine.is_empty() {
                Rated::now(Health::Ok)
            } else {
                let rs: Vec<(&Rule, Health)> = mine.iter().map(|r| (*r, rate(r, it.fields.get(&r.field), &ctx))).collect();
                combine(&rs)
            };
            (it.key.clone(), rated)
        })
        .collect()
}

/// The source-level reading from its own rules: `None` when it has none
/// (the fetch itself then reads ok).
pub fn rate_source(spec: &SourceSpec, p: &Projection, now: i64) -> Option<Rated> {
    let rules = source_rules(spec);
    if rules.is_empty() {
        return None;
    }
    let ctx = Ctx { now, item: None, source: Some(&p.fields) };
    let rs: Vec<(&Rule, Health)> = rules.iter().map(|r| (*r, rate(r, p.fields.get(&r.field), &ctx))).collect();
    Some(combine(&rs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(v: serde_json::Value) -> Rule {
        serde_json::from_value(v).unwrap()
    }

    fn rate0(r: &Rule, v: Option<&Json>) -> Health {
        rate(r, v, &Ctx::bare())
    }

    fn holds0(c: &Cond, v: Option<&Json>) -> bool {
        cond_holds(c, v, &Ctx::bare())
    }

    fn cond(v: serde_json::Value) -> Cond {
        serde_json::from_value(v).expect("a condition")
    }

    #[test]
    fn field_references_compare_to_the_item_then_the_source() {
        let item: BTreeMap<String, Json> = [("version".to_string(), json!("0.45.5")), ("used".to_string(), json!(90))].into();
        let src: BTreeMap<String, Json> =
            [("target_version".to_string(), json!("0.45.7")), ("used".to_string(), json!(10)), ("cap".to_string(), json!(80))].into();
        let ctx = Ctx { now: 0, item: Some(&item), source: Some(&src) };
        assert!(cond_holds(&cond(json!({"gt": {"field": "cap"}})), Some(&json!(90)), &ctx));
        assert!(!cond_holds(&cond(json!({"lt": {"field": "cap"}})), Some(&json!(90)), &ctx));
        // The item's own `used` wins over the source's.
        assert!(cond_holds(&cond(json!({"eq": {"field": "used"}})), Some(&json!(90)), &ctx));
        assert!(cond_holds(&cond(json!({"ne": {"field": "target_version"}})), Some(&json!("0.45.5")), &ctx));
        assert!(!cond_holds(&cond(json!({"eq": {"field": "target_version"}})), Some(&json!("0.45.5")), &ctx));
        // A reference to a missing field never holds, either way.
        assert!(!cond_holds(&cond(json!({"eq": {"field": "nope"}})), Some(&json!(1)), &ctx));
        assert!(!cond_holds(&cond(json!({"ne": {"field": "nope"}})), Some(&json!(1)), &ctx));
        assert!(!cond_holds(&cond(json!({"gt": {"field": "nope"}})), Some(&json!(1)), &ctx));
        // A literal object that isn't exactly {field} stays a literal.
        assert!(!cond_holds(&cond(json!({"eq": {"field": "cap", "x": 1}})), Some(&json!(80)), &ctx));
    }

    #[test]
    fn age_rules_read_seconds_milliseconds_and_rfc3339() {
        let now = 1_792_200_000; // 2026-10-17
        let ctx = Ctx { now, item: None, source: None };
        let old = cond(json!({"ageGt": 300}));
        let fresh = cond(json!({"ageLt": 300}));
        assert!(cond_holds(&old, Some(&json!(now - 600)), &ctx), "epoch seconds");
        assert!(!cond_holds(&old, Some(&json!(now - 60)), &ctx));
        assert!(cond_holds(&fresh, Some(&json!(now - 60)), &ctx));
        assert!(cond_holds(&old, Some(&json!((now - 600) * 1000)), &ctx), "epoch milliseconds");
        assert!(cond_holds(&fresh, Some(&json!(((now - 10) * 1000).to_string())), &ctx), "numeric string");
        let iso = chrono::DateTime::from_timestamp(now - 3600, 0).unwrap().to_rfc3339();
        assert!(cond_holds(&old, Some(&json!(iso)), &ctx), "RFC 3339");
        let naive = chrono::DateTime::from_timestamp(now - 30, 0).unwrap().format("%Y-%m-%dT%H:%M:%S").to_string();
        assert!(cond_holds(&fresh, Some(&json!(naive)), &ctx), "no offset reads as UTC");
        assert!(!cond_holds(&old, Some(&json!("yesterday")), &ctx), "unreadable never holds");
        assert!(!cond_holds(&fresh, Some(&json!("yesterday")), &ctx));
        assert!(!cond_holds(&old, None, &ctx));
        assert!(!cond_holds(&old, Some(&json!(true)), &ctx));
    }

    #[test]
    fn versions_parse_and_compare_with_a_distance() {
        assert_eq!(parse_version("0.45.7"), Some((0, 45, 7, true)));
        assert_eq!(parse_version("v1.2"), Some((1, 2, 0, true)));
        assert_eq!(parse_version("0.46.0-rc1"), Some((0, 46, 0, false)));
        assert_eq!(parse_version("0.46.0+abc"), Some((0, 46, 0, true)));
        assert_eq!(parse_version("latest"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
        let src: BTreeMap<String, Json> = [("target_version".to_string(), json!("0.46.0"))].into();
        let ctx = Ctx { now: 0, item: None, source: Some(&src) };
        let behind = cond(json!({"versionBehind": {"field": "target_version"}}));
        let at = cond(json!({"versionAtLeast": {"field": "target_version"}}));
        assert!(cond_holds(&behind, Some(&json!("0.45.7")), &ctx));
        assert!(cond_holds(&behind, Some(&json!("0.46.0-rc3")), &ctx), "a pre-release is behind its release");
        assert!(!cond_holds(&behind, Some(&json!("0.46.0")), &ctx));
        assert!(!cond_holds(&behind, Some(&json!("0.46.1")), &ctx));
        assert!(cond_holds(&at, Some(&json!("0.46.0")), &ctx));
        assert!(cond_holds(&at, Some(&json!("v0.47")), &ctx));
        assert!(!cond_holds(&at, Some(&json!("0.45.9")), &ctx));
        assert!(!cond_holds(&behind, Some(&json!("garbage")), &ctx), "unreadable never holds");
        let lit = cond(json!({"versionBehind": "0.45.7"}));
        assert!(cond_holds(&lit, Some(&json!("0.45.5")), &Ctx::bare()));
        // Distances.
        let t: BTreeMap<String, Json> = [("t".to_string(), json!("1.5.6"))].into();
        let ctx = Ctx { now: 0, item: None, source: Some(&t) };
        let minor2 = cond(json!({"versionBehind": {"field": "t", "minor": 2}}));
        assert!(cond_holds(&minor2, Some(&json!("1.3.9")), &ctx));
        assert!(!cond_holds(&minor2, Some(&json!("1.4.0")), &ctx));
        assert!(cond_holds(&minor2, Some(&json!("0.9.0")), &ctx), "an older major is behind any minor distance");
        let patch3 = cond(json!({"versionBehind": {"field": "t", "patch": 3}}));
        assert!(cond_holds(&patch3, Some(&json!("1.5.3")), &ctx));
        assert!(!cond_holds(&patch3, Some(&json!("1.5.4")), &ctx));
        assert!(cond_holds(&patch3, Some(&json!("1.4.9")), &ctx), "an older minor counts for patch");
        let major1 = cond(json!({"versionBehind": {"value": "2.0.0", "major": 1}}));
        assert!(cond_holds(&major1, Some(&json!("1.9.9")), &ctx));
        assert!(!cond_holds(&major1, Some(&json!("2.0.0-rc1")), &ctx));
    }

    #[test]
    fn combined_readings_carry_the_fewest_checks_their_rules_ask() {
        let spec: SourceSpec = serde_json::from_value(json!({
            "name": "x", "label": "X", "fetch": {"url": "https://a.example/"}, "everySecs": 60,
            "extract": {"fields": {"a": "/a", "b": "/b"}},
            "rules": [
                {"field": "a", "warn": {"gt": 1}, "crit": {"gt": 5}, "warnAfter": 2, "critAfter": 4},
                {"field": "b", "warn": {"gt": 1}, "crit": {"gt": 5}, "critAfter": 3}
            ]
        }))
        .unwrap();
        let p = |a: i64, b: i64| Projection { fields: [("a".to_string(), json!(a)), ("b".to_string(), json!(b))].into(), items: vec![] };
        assert_eq!(rate_source(&spec, &p(9, 0), 0), Some(Rated { health: Health::Crit, warn_after: 2, crit_after: 4 }));
        assert_eq!(rate_source(&spec, &p(9, 9), 0), Some(Rated { health: Health::Crit, warn_after: 1, crit_after: 3 }));
        assert_eq!(rate_source(&spec, &p(2, 0), 0), Some(Rated { health: Health::Warn, warn_after: 2, crit_after: 1 }));
        assert_eq!(rate_source(&spec, &p(0, 0), 0).unwrap().health, Health::Unknown);
    }

    #[test]
    fn every_operator_and_the_crit_warn_ok_order() {
        let r = rule(json!({"field": "s", "crit": {"in": ["major_outage", "partial_outage"]},
                            "warn": {"in": ["degraded_performance"]}, "ok": {"eq": "operational"}}));
        assert_eq!(rate0(&r, Some(&json!("partial_outage"))), Health::Crit);
        assert_eq!(rate0(&r, Some(&json!("degraded_performance"))), Health::Warn);
        assert_eq!(rate0(&r, Some(&json!("operational"))), Health::Ok);
        assert_eq!(rate0(&r, Some(&json!("weird"))), Health::Unknown);
        assert_eq!(rate0(&r, None), Health::Unknown);
        assert_eq!(rate0(&r, Some(&Json::Null)), Health::Unknown);

        let days = rule(json!({"field": "d", "crit": {"lte": 7}, "warn": {"lte": 14}, "ok": {"gt": 14}}));
        assert_eq!(rate0(&days, Some(&json!(7))), Health::Crit, "crit checked first");
        assert_eq!(rate0(&days, Some(&json!(10.5))), Health::Warn);
        assert_eq!(rate0(&days, Some(&json!(30))), Health::Ok);
        assert_eq!(rate0(&days, Some(&json!("30"))), Health::Unknown, "strings aren't numbers");

        let up = rule(json!({"field": "up", "ok": {"eq": true}, "crit": {"eq": false}}));
        assert_eq!(rate0(&up, Some(&json!(false))), Health::Crit);
        let n = rule(json!({"field": "n", "crit": {"gt": 0}, "ok": {"eq": 0}}));
        assert_eq!(rate0(&n, Some(&json!(0.0))), Health::Ok, "0.0 == 0");
        assert_eq!(rate0(&n, Some(&json!(3))), Health::Crit);
        assert!(holds0(&Cond::Lt(Operand::Num(1.0)), Some(&json!(0))));
        assert!(holds0(&Cond::Gte(Operand::Num(1.0)), Some(&json!(1))));
        assert!(holds0(&Cond::Ne(json!("a")), Some(&json!("b"))));
        // warn is checked before ok, so "anything but ready" is `ne`, not
        // `exists: true` (which would also hold for "ready").
        let ex = rule(json!({"field": "phase", "ok": {"eq": "ready"}, "warn": {"ne": "ready"}, "crit": {"exists": false}}));
        assert_eq!(rate0(&ex, None), Health::Crit);
        assert_eq!(rate0(&ex, Some(&json!("booting"))), Health::Warn);
        assert_eq!(rate0(&ex, Some(&json!("ready"))), Health::Ok);
    }

    #[test]
    fn worst_counts_unknown_apart() {
        assert_eq!(worst([Health::Ok, Health::Unknown, Health::Ok], Health::Ok), Health::Ok);
        assert_eq!(worst([Health::Unknown, Health::Unknown], Health::Ok), Health::Unknown);
        assert_eq!(worst([Health::Warn, Health::Crit, Health::Unknown], Health::Ok), Health::Crit);
        assert_eq!(worst([], Health::Ok), Health::Ok);
    }
}
