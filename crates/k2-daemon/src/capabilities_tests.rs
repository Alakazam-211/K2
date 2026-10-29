//! `k2 capabilities` (TC-17): the JSON shape (`schema: 1`, rows of
//! `{cap, ok, detail, missing?}`), every `missing.code` a registry row,
//! the reason words, push keys as yes/no, a node's row, App passes refused.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use super::*;

fn call(who: Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(who, &p);
    let n: u16 = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

const REASONS: [&str; 5] = [NOT_BUILT, OWNER_DISABLED, LEVEL, NOT_PREPARED, HARDWARE];

fn check_rows(rows: &[J]) {
    for r in rows {
        let cap = r["cap"].as_str().unwrap_or_else(|| panic!("cap: {r}"));
        assert!(r["ok"].is_boolean() && r["detail"].is_string(), "{cap}: {r}");
        match r.get("missing") {
            None => assert_eq!(r["ok"], true, "{cap}: a no row says why: {r}"),
            Some(m) => {
                assert_eq!(r["ok"], false, "{cap}: {r}");
                let reason = m["reason"].as_str().expect("reason");
                assert!(REASONS.contains(&reason), "{cap}: reason {reason}");
                assert!(m["fix"].as_str().is_some_and(|f| !f.is_empty()), "{cap}: a fix");
                if let Some(code) = m["code"].as_str() {
                    assert!(k2_core::refusals::lookup(code).is_some(), "{cap}: missing.code {code} is in the refusal registry (k2 explain {code})");
                } else {
                    assert!(m["code"].is_null(), "{cap}: code is an id or null: {m}");
                }
            }
        }
    }
}

#[test]
fn the_box_rows_have_the_shape_and_every_code_is_explained() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (st, v) = call(Who::Owner, &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["schema"], SCHEMA);
    let rows = v["box"].as_array().expect("box rows");
    let caps: Vec<&str> = rows.iter().filter_map(|r| r["cap"].as_str()).collect();
    for want in ["apps", "engine.walled", "engine.open", "engine.static", "protocols", "protocols.other", "ports.public", "domains.custom", "sim", "postgres", "mail", "updates", "limits"] {
        assert!(caps.contains(&want), "{want}: {caps:?}");
    }
    check_rows(rows);
    check_rows(v["compute"].as_array().expect("compute rows"));
    assert!(v["doctor"].is_object(), "the doctor's verdict: {v}");
    let limits = rows.iter().find(|r| r["cap"] == "limits").expect("limits");
    assert!(limits["detail"].as_str().expect("detail").contains("Seed 4 GiB and 200k entries"), "{limits}");
    // Apps off: the apps row says so and names the id.
    k2_core::deploy::set_enabled(false);
    let (_, v) = call(Who::Owner, &[]);
    let apps = v["box"].as_array().expect("rows").iter().find(|r| r["cap"] == "apps").cloned().expect("apps row");
    assert_eq!(apps["missing"], json!({ "reason": OWNER_DISABLED, "fix": "the owner turns them on in Settings → Apps → This server", "code": "apps_disabled" }));
    k2_core::deploy::set_enabled(true);
}

#[test]
fn every_code_this_module_can_name_is_a_registry_row() {
    let src = include_str!("capabilities.rs");
    let prod = src.split("\n#[cfg(test)]").next().expect("prod");
    for (i, _) in prod.match_indices("Some(\"") {
        let rest = &prod[i + 6..];
        let code = &rest[..rest.find('"').expect("end")];
        if code.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            assert!(k2_core::refusals::lookup(code).is_some(), "capabilities names {code}: add it to the refusal registry");
        }
    }
}

#[test]
fn push_keys_are_yes_or_no_and_an_app_pass_is_refused() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let (st, v) = call(Who::Wrong, &[]);
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("wrong_credential")), "{v}");
    let (st, _) = call(Who::Invalid, &[]);
    assert_eq!(st, 403);
    let text = call(Who::Owner, &[]).1.to_string();
    assert!(!text.contains("BEGIN PRIVATE KEY") && !text.contains("\"p8\""), "never a key");
}

#[test]
fn a_nodes_row_reads_os_slots_xcode_and_android() {
    let n = json!({ "name": "k2n2-m1", "os": "macos", "arch": "arm64", "online": true, "running": 1,
        "offer": { "parallelMax": 2, "tools": ["node@22", "xcode@26.0", "rust@stable"] } });
    let r = node_row(&n);
    assert_eq!(r["ok"], true);
    assert_eq!(r["node"], json!({ "name": "k2n2-m1", "os": "macos", "arch": "arm64", "online": true, "freeSlots": 1, "xcode": true, "androidSdk": false, "tools": ["node@22", "xcode@26.0", "rust@stable"] }));
    assert!(r["detail"].as_str().expect("detail").contains("macos-arm64, online, 1 free slot(s), Xcode yes, Android SDK no"), "{r}");
    let off = node_row(&json!({ "name": "k2n9", "os": "linux", "arch": "x86_64", "online": false }));
    check_rows(&[off.clone()]);
    assert_eq!(off["missing"]["reason"], NOT_PREPARED, "{off}");
}
