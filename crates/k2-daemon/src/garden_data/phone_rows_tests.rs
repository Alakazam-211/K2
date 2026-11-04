//! Rows only the phone door answers: the page they read, which addresses a
//! placement binds, and a card view as a frame gets it. The door's own tests
//! (who gets in, every refusal, no secret reaches a phone) are in
//! `remote_door_sources_tests.rs`. Fail loudly.

use super::*;
use crate::garden_data::call::door_tests::door;

#[test]
fn the_page_a_row_reads_is_the_resolved_page_s_layout_and_widgets() {
    let d = door(&["store:read"]);
    let page = d.f.page_of(&d.garden).expect("page");
    let resolved = d.f.resolve(Some(&d.garden)).expect("resolve");
    assert_eq!(page["layout"], resolved["page"]["layout"]);
    let ids = |p: &serde_json::Value| -> Vec<String> {
        p["widgets"].as_array().expect("widgets").iter().filter_map(|w| w["id"].as_str().map(str::to_string)).collect()
    };
    assert_eq!(ids(&page), ids(&resolved["page"]));
    assert_eq!(ids(&page), ["b1"]);
    assert!(d.f.page_of("g-ffffffff").is_err(), "an unknown Garden");
}

fn addresses(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn addresses_are_canonical_and_this_server_s_are_local() {
    let got = of_addresses(&addresses(&["fleet", "money::Me.K2.dev", "far::other.k2.dev", "fleet", "bad name"]), Some("me.k2.dev"));
    assert_eq!(
        got,
        vec![
            Bound { address: "fleet".into(), name: "fleet".into(), local: true },
            Bound { address: "money::me.k2.dev".into(), name: "money".into(), local: true },
            Bound { address: "far::other.k2.dev".into(), name: "far".into(), local: false },
        ],
        "once each; a bad one binds nothing"
    );
    let got = of_addresses(&addresses(&["money::me.k2.dev"]), None);
    assert!(!got[0].local, "with no Connect name nothing with a host is this server");
}

#[test]
fn a_frame_gets_scalars() {
    let v = frame_view(
        "fleet",
        json!({ "name": "fleet", "fields": { "n": 3, "ok": true, "s": "x", "none": null, "list": [1, 2], "map": { "a": 1 } },
                "items": [{ "key": "a", "fields": { "v": { "b": 2 }, "w": "y" } }] }),
    );
    assert_eq!(v["address"], "fleet");
    assert_eq!(v["reach"], "ok");
    assert_eq!(v["fields"], json!({ "n": 3, "ok": true, "s": "x", "none": null, "list": "[1,2]", "map": "{\"a\":1}" }));
    assert_eq!(v["items"][0]["fields"], json!({ "v": "{\"b\":2}", "w": "y" }));
}
