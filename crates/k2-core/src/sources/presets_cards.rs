//! Card presets (contract-sources-v1 §11.4): the sources the five built-in
//! cards pair with. `presets::find` (B1) searches here after the base
//! presets; `name`/`label` are filled by the caller (placeholders here).
//!
//! | Preset | Args | Card |
//! |---|---|---|
//! | `k2-fleet` | `boxes=ohana,luzz,…` (a bare label is `<label>.k2.dev`) | `k2:fleet-grid@1`, `k2:version-spread@1` |
//! | `k2-fleet-summary` | `url`, `secret` | the same cards, from K2 Web's one route |
//! | `github-latest-release` | `repo=Owner/Name` | `k2:version-spread@1` (`latest`) |
//! | `vercel-deployments` | `team`, `secret`, `limit?` (≤ 20) | `k2:deploys@1` |
//! | `stripe-past-due` | `secret` | `k2:money-alerts@1` |
//!
//! **With or without `fleet-summary`** (Q6, pending): `k2-fleet` fans out
//! over each box's public `/boot-status`; `k2-fleet-summary` reads one JSON
//! route (`{"boxes": [{"name", "version", "phase", "relay", "up"}],
//! "relays": [...], "dns": [...]}`). Both give items keyed by box with
//! fields `version` and `phase` (the summary adds `relay` and `up`), so the
//! cards don't change when a fleet source switches.

use std::collections::BTreeMap;

use serde_json::json;

use super::spec::{Json, SourceSpec, SpecError};

pub const NAMES: &[&str] =
    &["k2-fleet", "k2-fleet-summary", "github-latest-release", "vercel-deployments", "stripe-past-due"];

/// The Stripe list URL the money card reads (≤ 100 past-due subscriptions).
pub const STRIPE_PAST_DUE_URL: &str = "https://api.stripe.com/v1/subscriptions?status=past_due&limit=100";

fn bad(hint: impl Into<String>) -> SpecError {
    SpecError { code: "bad_preset", field: "presetArgs".into(), hint: hint.into() }
}

fn arg<'a>(args: &'a BTreeMap<String, String>, k: &str) -> Result<&'a str, SpecError> {
    args.get(k).map(|v| v.trim()).filter(|v| !v.is_empty()).ok_or_else(|| bad(format!("this preset needs {k}=…")))
}

/// A vault secret's name: what `k2 vault` names take.
fn secret_arg(args: &BTreeMap<String, String>) -> Result<String, SpecError> {
    let s = arg(args, "secret")?;
    if s.len() > 128 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
        return Err(bad("secret is a vault secret's name (letters, digits, _ - .), never the value"));
    }
    Ok(s.to_string())
}

/// `ohana` → `ohana.k2.dev`; a dotted name is taken as the host.
fn box_host(b: &str) -> Result<String, SpecError> {
    let b = b.trim().to_ascii_lowercase();
    let ok = !b.is_empty()
        && b.len() <= 253
        && !b.starts_with(['.', '-'])
        && !b.ends_with(['.', '-'])
        && !b.contains("..")
        && b.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.');
    if !ok {
        return Err(bad(format!("'{b}' isn't a box name (a k2.dev label like ohana, or a host name)")));
    }
    Ok(if b.contains('.') { b } else { format!("{b}.{}", crate::tunnel::config::SUBDOMAIN_HOST) })
}

/// The item key a box shows as: its label for `<label>.k2.dev`, else the host.
fn box_key(host: &str) -> String {
    host.strip_suffix(&format!(".{}", crate::tunnel::config::SUBDOMAIN_HOST)).unwrap_or(host).to_string()
}

/// The fleet's phase rule: ready is green, any other phase amber; a box
/// that doesn't answer is red by `onTargetError` (per box, GV55).
fn phase_rule() -> Json {
    json!({"field": "phase", "warn": {"ne": "ready"}, "ok": {"eq": "ready"}})
}

fn spec(v: Json) -> Result<SourceSpec, SpecError> {
    let mut v = v;
    if let Json::Object(m) = &mut v {
        // Placeholders: the caller's name/label win (`presets::expand`).
        m.entry("name").or_insert(json!("preset"));
        m.entry("label").or_insert(json!("Preset"));
    }
    serde_json::from_value(v).map_err(|e| bad(format!("card preset doesn't build: {}", e.to_string().chars().take(160).collect::<String>())))
}

fn build(name: &str, args: &BTreeMap<String, String>) -> Result<SourceSpec, SpecError> {
    match name {
        "k2-fleet" => {
            let boxes = arg(args, "boxes")?;
            let mut targets = Vec::new();
            for b in boxes.split(',').filter(|b| !b.trim().is_empty()) {
                let host = box_host(b)?;
                let key = box_key(&host);
                if targets.iter().any(|t: &Json| t["key"] == key.as_str()) {
                    return Err(bad(format!("{key} is listed twice")));
                }
                targets.push(json!({"key": key, "url": format!("https://{host}/boot-status")}));
            }
            if targets.is_empty() {
                return Err(bad("boxes lists at least one box: boxes=ohana,luzz"));
            }
            if targets.len() > super::MAX_TARGETS_PER_SOURCE {
                return Err(bad(format!("at most {} boxes per source", super::MAX_TARGETS_PER_SOURCE)));
            }
            spec(json!({
                "preset": "k2-fleet",
                "fetch": {"targets": targets},
                "everySecs": 60,
                "timeoutSecs": super::FANOUT_TIMEOUT_SECS,
                "extract": {
                    "items": {"pointer": null, "itemKey": null, "itemFields": {"version": "/version", "phase": "/phase"}},
                    "aggregates": {"count": {"op": "count"}, "versions": {"op": "distinct", "field": "version"}},
                    "trend": ["count"]
                },
                "rules": [phase_rule()],
                "onTargetError": "crit",
            }))
        }
        "k2-fleet-summary" => {
            let url = arg(args, "url")?;
            spec(json!({
                "preset": "k2-fleet-summary",
                "fetch": {"url": url},
                "secret": secret_arg(args)?,
                "everySecs": 60,
                "extract": {
                    "items": {"pointer": "/boxes", "itemKey": "/name",
                              "itemFields": {"version": "/version", "phase": "/phase", "relay": "/relay", "up": "/up"}},
                    "aggregates": {"count": {"op": "count"}, "versions": {"op": "distinct", "field": "version"},
                                   "down": {"op": "countWhere", "field": "up", "cond": {"eq": false}}},
                    "trend": ["count", "down"]
                },
                "rules": [phase_rule()],
            }))
        }
        "github-latest-release" => {
            let repo = arg(args, "repo")?;
            let parts: Vec<&str> = repo.split('/').collect();
            let ok_part = |p: &str| !p.is_empty() && p.len() <= 100 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
            if parts.len() != 2 || !parts.iter().all(|p| ok_part(p)) {
                return Err(bad("repo is Owner/Name, like Alakazam-211/K2"));
            }
            spec(json!({
                "preset": "github-latest-release",
                "fetch": {"url": format!("https://api.github.com/repos/{repo}/releases/latest")},
                "everySecs": 1800,
                "extract": {"fields": {"newest": "/tag_name", "published": "/published_at"}},
                "rules": [{"field": "newest", "ok": {"exists": true}}],
                "link": format!("https://github.com/{repo}/releases"),
            }))
        }
        "vercel-deployments" => {
            let team = arg(args, "team")?;
            if !team.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') || team.len() > 64 {
                return Err(bad("team is a Vercel team id like team_abc123"));
            }
            let limit: u32 = match args.get("limit").map(|v| v.trim()).filter(|v| !v.is_empty()) {
                None => 5,
                Some(v) => v.parse().ok().filter(|n| (1..=20).contains(n)).ok_or_else(|| bad("limit is 1 to 20"))?,
            };
            spec(json!({
                "preset": "vercel-deployments",
                "fetch": {"url": format!("https://api.vercel.com/v6/deployments?teamId={team}&target=production&limit={limit}")},
                "secret": secret_arg(args)?,
                "everySecs": 180,
                "extract": {
                    "items": {"pointer": "/deployments", "itemKey": "/uid",
                              "itemFields": {"state": "/state", "url": "/url", "created": "/created", "target": "/target"}}
                },
                "rules": [{"field": "state", "crit": {"eq": "ERROR"}, "warn": {"in": ["BUILDING", "QUEUED", "INITIALIZING"]}, "ok": {"eq": "READY"}}],
                "link": "https://vercel.com/dashboard",
                "agentAccess": "shape",
                "viewers": "admin",
            }))
        }
        "stripe-past-due" => spec(json!({
            "preset": "stripe-past-due",
            "fetch": {"url": STRIPE_PAST_DUE_URL},
            "secret": secret_arg(args)?,
            "everySecs": 1800,
            "extract": {
                // Index keys only: no Stripe ids are kept, even as item keys.
                "items": {"pointer": "/data", "itemKey": null, "itemFields": {}},
                "fields": {"hasMore": "/has_more"},
                "aggregates": {"pastDue": {"op": "count"}}
            },
            "rules": [{"field": "pastDue", "crit": {"gt": 0.0}, "ok": {"eq": 0}}],
            "link": "https://dashboard.stripe.com/subscriptions",
            "agentAccess": "shape",
            "viewers": "admin",
            // Money values never rest on disk (GV47): states only.
            "keep": "states",
        })),
        _ => Err(bad(format!("unknown card preset {name}"))),
    }
}

/// The spec a card preset makes from its `presetArgs`; `None` when `name`
/// is not a card preset. `name`/`label` are placeholders the caller fills.
pub fn find(name: &str, args: &BTreeMap<String, String>) -> Option<Result<SourceSpec, SpecError>> {
    if !NAMES.contains(&name) {
        return None;
    }
    Some(build(name, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{Keep, OnTargetError};

    fn args(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn built(name: &str, kv: &[(&str, &str)]) -> SourceSpec {
        find(name, &args(kv)).expect("a card preset").unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    /// Every card preset makes a spec that validates (contract §12.3).
    #[test]
    fn every_card_preset_validates() {
        let cases: [(&str, &[(&str, &str)]); 5] = [
            ("k2-fleet", &[("boxes", "ohana, luzz,box.example.test")]),
            ("k2-fleet-summary", &[("url", "https://cp.example.test/fleet-summary"), ("secret", "FLEET_RO")]),
            ("github-latest-release", &[("repo", "Alakazam-211/K2")]),
            ("vercel-deployments", &[("team", "team_x"), ("secret", "VERCEL_RO")]),
            ("stripe-past-due", &[("secret", "STRIPE_RK")]),
        ];
        for (name, kv) in cases {
            let mut s = built(name, kv);
            s.name = "card".into();
            s.label = "Card".into();
            s.validate(false).unwrap_or_else(|e| panic!("{name} validates: {e:?}"));
            // And through the base `find` + `expand` path, as the routes do.
            let mut body = serde_json::Map::new();
            body.insert("preset".into(), json!(name));
            body.insert("presetArgs".into(), json!(args(kv)));
            body.insert("name".into(), json!("card"));
            body.insert("label".into(), json!("Card"));
            let e = super::super::presets::expand(&Json::Object(body)).unwrap_or_else(|e| panic!("{name} expands: {e:?}"));
            assert_eq!(e.preset.as_deref(), Some(name));
            assert_eq!(e.name, "card");
        }
    }

    #[test]
    fn fleet_fans_out_per_box_and_matches_the_summary_fields() {
        let s = built("k2-fleet", &[("boxes", "ohana,luzz")]);
        let crate::sources::FetchTargets::Targets(t) = &s.fetch else { panic!("fan-out") };
        assert_eq!(t.iter().map(|t| (t.key.as_str(), t.url.as_str())).collect::<Vec<_>>(), [
            ("ohana", "https://ohana.k2.dev/boot-status"),
            ("luzz", "https://luzz.k2.dev/boot-status")
        ]);
        assert_eq!(s.on_target_error, OnTargetError::Crit, "a down box is a red tile (GV55)");
        assert_eq!(s.timeout_secs, crate::sources::FANOUT_TIMEOUT_SECS);
        let summary = built("k2-fleet-summary", &[("url", "https://cp.example.test/x"), ("secret", "S")]);
        let fields = |s: &SourceSpec| s.extract.items.as_ref().map(|i| i.item_fields.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
        for f in fields(&s) {
            assert!(fields(&summary).contains(&f), "the summary has fleet field {f}");
        }
        assert!(find("k2-fleet", &args(&[("boxes", "")])).expect("preset").is_err());
        assert!(find("k2-fleet", &args(&[("boxes", "a,a")])).expect("preset").is_err());
        assert!(find("k2-fleet", &args(&[("boxes", "https://x")])).expect("preset").is_err());
    }

    #[test]
    fn money_keeps_states_and_no_stripe_ids() {
        let s = built("stripe-past-due", &[("secret", "STRIPE_RK")]);
        assert_eq!(s.keep, Keep::States);
        assert_eq!(s.secret.as_deref(), Some("STRIPE_RK"));
        assert_eq!(s.extract.items.as_ref().and_then(|i| i.item_key.clone()), None, "index keys, never sub_… ids");
        assert!(find("stripe-past-due", &args(&[])).expect("preset").is_err(), "a secret is required");
        assert!(find("stripe-past-due", &args(&[("secret", "sk_live abc")])).expect("preset").is_err(), "a name, not a value");
    }

    /// The engine meets the card (B1 x B3): the k2-fleet preset polled by
    /// the real engine (`apply_poll`, `view::build`) with one box timing
    /// out gives a card view whose keys are exactly the catalog's
    /// `SourceView` (minus the reader's `address` and `reach`), and the
    /// down box is a red item with its reason while the source stays live
    /// (GV55): what `k2:fleet-grid@1` draws as a red tile.
    #[test]
    fn the_engine_view_of_a_fleet_matches_the_catalog_card_view() {
        use crate::sources::state::{apply_poll, FetchOutcome, Health, Phase, PollInput, Reason, SourceState};
        use crate::sources::view::{build, Viewer};
        use crate::sources::{AgentAccess, SourceKind, SourceRow, SourceStatus, Viewers};
        let mut spec = built("k2-fleet", &[("boxes", "ohana,luzz,acv")]);
        spec.name = "fleet".into();
        spec.label = "Fleet boxes".into();
        let v = spec.validate(false).expect("validates");
        let ok = |ver: &str| FetchOutcome::Ok {
            status: 200,
            body: json!({"version": ver, "phase": "ready"}),
            not_modified: false,
            etag: None,
            last_modified: None,
            elapsed_ms: 40,
        };
        let mut st = SourceState::new("s1");
        let mut proj = None;
        for i in 0..3 {
            let out = apply_poll(&st, PollInput {
                spec: &v.spec,
                outcomes: vec![
                    (Some("ohana".into()), FetchOutcome::Err { reason: Reason::Timeout, http_status: None, retry_after_secs: None }),
                    (Some("luzz".into()), ok("0.45.6")),
                    (Some("acv".into()), ok("0.45.5")),
                ],
                now: 1_000 + i * 60,
                simulated: false,
                prev_projection: proj.as_ref(),
            });
            st = out.state;
            if out.projection.is_some() {
                proj = out.projection;
            }
        }
        let row = SourceRow {
            id: "s1".into(),
            name: "fleet".into(),
            label: "Fleet boxes".into(),
            kind: SourceKind::HttpJson,
            preset: Some("k2-fleet".into()),
            spec_json: v.spec.canonical_json(),
            spec_hash: v.spec.hash(),
            origins: v.origins.clone(),
            lan: false,
            secret_name: None,
            url_secret_name: None,
            secret_bound: false,
            every_secs: v.spec.every_secs,
            timeout_secs: v.spec.timeout_secs,
            max_bytes: v.spec.max_bytes,
            fail_after: v.spec.fail_after,
            on_target_error: v.spec.on_target_error,
            link: None,
            viewers: Viewers::Member,
            agent_access: AgentAccess::Values,
            keep: v.spec.keep,
            pinned: false,
            pinned_by: None,
            pinned_at: None,
            status: SourceStatus::Active,
            pause_reason: None,
            made_by: "owner".into(),
            made_by_project: None,
            approved_by: None,
            approved_at: None,
            approved_spec_hash: None,
            simulate_state: None,
            simulate_until: None,
            simulate_by: None,
            mute_until: None,
            mute_by: None,
            mute_reason: None,
            rev: 3,
            created_at: 0,
            updated_at: 0,
            removed_at: None,
        };
        let view = build(&row, &st, proj.as_ref(), Viewer::Person, 1_200);
        assert_eq!(view.phase, Phase::Live, "one box down never stales the source (GV55)");
        let j = serde_json::to_value(&view).expect("view to JSON");

        let cat = crate::contract::catalog().verb("sources.read").expect("sources.read row");
        let want: std::collections::BTreeSet<String> = cat.response["properties"]
            .as_object()
            .expect("SourceView properties")
            .keys()
            .filter(|k| *k != "address" && *k != "reach")
            .cloned()
            .collect();
        let got: std::collections::BTreeSet<String> = j.as_object().expect("object").keys().cloned().collect();
        assert_eq!(got, want, "the engine's card view and the catalog's SourceView drift");
        let item_want: std::collections::BTreeSet<String> =
            cat.response["properties"]["items"]["items"]["properties"].as_object().expect("SourceItem").keys().cloned().collect();
        for it in j["items"].as_array().expect("items") {
            let keys: std::collections::BTreeSet<String> = it.as_object().expect("item").keys().cloned().collect();
            assert_eq!(keys, item_want, "item {it}");
        }
        let ohana = j["items"].as_array().and_then(|a| a.iter().find(|i| i["key"] == "ohana")).cloned().expect("ohana tile");
        assert_eq!((ohana["health"].as_str(), ohana["reason"].as_str()), (Some("crit"), Some("timeout")), "{ohana}");
        let luzz = j["items"].as_array().and_then(|a| a.iter().find(|i| i["key"] == "luzz")).cloned().expect("luzz tile");
        assert_eq!(luzz["fields"]["version"], "0.45.6");
        assert_eq!(view.server_now, 1_200);
        assert!(st.items.get("ohana").is_some_and(|i| i.health == Health::Crit));
    }

    #[test]
    fn deploys_and_release_shapes() {
        let s = built("vercel-deployments", &[("team", "team_x"), ("secret", "V"), ("limit", "20")]);
        assert!(matches!(&s.fetch, crate::sources::FetchTargets::Url(u) if u.ends_with("limit=20")));
        assert_eq!(s.agent_access, Some(crate::sources::AgentAccess::Shape));
        assert!(find("vercel-deployments", &args(&[("team", "t"), ("secret", "V"), ("limit", "21")])).expect("p").is_err());
        let r = built("github-latest-release", &[("repo", "Alakazam-211/K2")]);
        assert!(matches!(&r.fetch, crate::sources::FetchTargets::Url(u) if u == "https://api.github.com/repos/Alakazam-211/K2/releases/latest"));
        assert!(find("github-latest-release", &args(&[("repo", "nope")])).expect("p").is_err());
        assert!(find("not-a-card", &args(&[])).is_none());
    }
}
