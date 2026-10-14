//! Base presets (PRD §5.1): `statuspage`, `k2-boot-status`, `gatus`,
//! `json`. A preset only fills fields; anything the caller names
//! explicitly wins. Card presets live in `presets_cards` (B3); [`find`]
//! searches there after these.

use std::collections::BTreeMap;

use serde_json::json;

use super::spec::{Json, SourceSpec, SpecError};

pub const BASE: &[&str] = &["statuspage", "k2-boot-status", "gatus", "json"];

pub fn is_known(name: &str) -> bool {
    BASE.contains(&name) || super::presets_cards::NAMES.contains(&name)
}

fn bad(hint: impl Into<String>) -> SpecError {
    SpecError { code: "bad_preset", field: "presetArgs".into(), hint: hint.into() }
}

fn arg<'a>(args: &'a BTreeMap<String, String>, k: &str) -> Result<&'a str, SpecError> {
    args.get(k).map(String::as_str).filter(|v| !v.trim().is_empty()).ok_or_else(|| bad(format!("this preset needs {k}=…")))
}

/// `https://host[/base]` without a trailing slash.
fn base_url(v: &str) -> String {
    v.trim().trim_end_matches('/').to_string()
}

/// The export-shaped JSON a base preset fills (no name/label).
fn base(name: &str, args: &BTreeMap<String, String>) -> Option<Result<Json, SpecError>> {
    let r = match name {
        // Atlassian Statuspage: the page's summary, one item per component.
        "statuspage" => arg(args, "url").map(|u| {
            json!({
                "preset": "statuspage",
                "fetch": {"url": format!("{}/api/v2/summary.json", base_url(u))},
                "everySecs": 180,
                "extract": {
                    "fields": {"indicator": "/status/indicator", "description": "/status/description"},
                    "items": {"pointer": "/components", "itemKey": "/name", "itemFields": {"status": "/status"}}
                },
                "rules": [
                    {"field": "status", "crit": {"in": ["major_outage", "partial_outage"]},
                     "warn": {"in": ["degraded_performance", "under_maintenance"]}, "ok": {"eq": "operational"}}
                ]
            })
        }),
        // One K2 server's public readiness probe.
        "k2-boot-status" => arg(args, "url").map(|u| {
            json!({
                "preset": "k2-boot-status",
                "fetch": {"url": format!("{}/boot-status", base_url(u))},
                "everySecs": 60,
                "extract": {"fields": {"version": "/version", "phase": "/phase"}},
                "rules": [{"field": "phase", "ok": {"eq": "ready"}, "warn": {"ne": "ready"}, "crit": {"exists": false}}]
            })
        }),
        // Gatus: one item per endpoint, the newest result's success.
        "gatus" => arg(args, "url").map(|u| {
            json!({
                "preset": "gatus",
                "fetch": {"url": format!("{}/api/v1/endpoints/statuses", base_url(u))},
                "everySecs": 60,
                "extract": {
                    "items": {"pointer": "", "itemKey": "/key", "itemFields": {"name": "/name", "group": "/group", "up": "/results/0/success"}},
                    "aggregates": {"count": {"op": "count"}, "down": {"op": "countWhere", "field": "up", "cond": {"eq": false}}}
                },
                "rules": [{"field": "up", "ok": {"eq": true}, "crit": {"eq": false}}]
            })
        }),
        // Any JSON URL: nothing extracted until the caller adds fields.
        "json" => arg(args, "url").map(|u| json!({"preset": "json", "fetch": {"url": u.trim()}, "everySecs": 60})),
        _ => return None,
    };
    Some(r)
}

/// A preset by name: base presets first, then the card presets.
pub fn find(name: &str, args: &BTreeMap<String, String>) -> Option<Result<Json, SpecError>> {
    if let Some(r) = base(name, args) {
        return Some(r);
    }
    super::presets_cards::find(name, args).map(|r| {
        r.map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or(Json::Null);
            if let Json::Object(m) = &mut v {
                m.remove("name");
                m.remove("label");
                m.insert("preset".into(), Json::String(name.to_string()));
            }
            v
        })
    })
}

/// Turn a request body into a spec. With `preset`, the preset fills the
/// definition and every other key in the body overrides it (top-level
/// keys only). Without it, the body is the export shape.
pub fn expand(body: &Json) -> Result<SourceSpec, SpecError> {
    let mut obj = body.as_object().cloned().ok_or_else(|| SpecError {
        code: "bad_preset",
        field: String::new(),
        hint: "the body is a JSON object".into(),
    })?;
    let preset = obj.remove("preset");
    let args = obj.remove("presetArgs");
    let merged = match preset {
        Some(Json::String(name)) => {
            let args: BTreeMap<String, String> = match args {
                None | Some(Json::Null) => BTreeMap::new(),
                Some(a) => serde_json::from_value(a).map_err(|_| bad("presetArgs is an object of strings"))?,
            };
            let mut base = match find(&name, &args) {
                Some(r) => r?,
                None => return Err(SpecError { code: "bad_preset", field: "preset".into(), hint: format!("unknown preset {name}") }),
            };
            if let Json::Object(m) = &mut base {
                for (k, v) in obj {
                    m.insert(k, v);
                }
            }
            base
        }
        Some(Json::Null) | None => {
            if args.is_some_and(|a| !a.is_null()) {
                return Err(bad("presetArgs needs preset"));
            }
            Json::Object(obj)
        }
        Some(_) => return Err(SpecError { code: "bad_preset", field: "preset".into(), hint: "preset is a name".into() }),
    };
    serde_json::from_value(merged).map_err(|e| SpecError {
        code: "unknown_field",
        field: String::new(),
        hint: format!("the definition doesn't parse: {}", short(&e.to_string())),
    })
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_presets_expand_validate_and_take_overrides() {
        for (preset, url) in [
            ("statuspage", "https://status.example.com"),
            ("k2-boot-status", "https://box.example.com/"),
            ("gatus", "https://gatus.example.com"),
            ("json", "https://api.example.com/v1/health"),
        ] {
            let s = expand(&json!({"preset": preset, "presetArgs": {"url": url}, "name": "p1", "label": "P1"}))
                .unwrap_or_else(|e| panic!("{preset}: {e:?}"));
            assert_eq!(s.preset.as_deref(), Some(preset));
            s.validate(false).unwrap_or_else(|e| panic!("{preset} validates: {e:?}"));
        }
        let s = expand(&json!({"preset": "k2-boot-status", "presetArgs": {"url": "https://box.example.com"},
                               "name": "b", "label": "B", "everySecs": 300})).unwrap();
        assert_eq!(s.every_secs, 300);
        assert_eq!(expand(&json!({"preset": "k2-boot-status", "name": "b", "label": "B"})).unwrap_err().code, "bad_preset");
        assert_eq!(expand(&json!({"preset": "nope", "name": "b", "label": "B"})).unwrap_err().code, "bad_preset");
        assert_eq!(expand(&json!({"name": "b", "label": "B", "fetch": {"url": "x"}, "everySecs": 60, "bogus": 1}))
            .unwrap_err().code, "unknown_field");
    }
}
