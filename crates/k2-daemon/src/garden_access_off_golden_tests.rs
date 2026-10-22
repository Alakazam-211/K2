//! Garden access (prd-garden-access-v1 §8, G0a item 4): the switch-off golden
//! for the Garden rows of the permission catalog,
//! `tests/golden/garden_access_off/catalog_rows.txt`.
//!
//! With `gardenAccess` off (the default):
//! - `garden.keep-mine` and `garden.proposal.approve` check exactly as at
//!   0.46.3. Their lines were captured from integ/0.46.3-r9 (no Garden
//!   access code).
//! - G0a item 4, the named exception: `garden.access`, its twin
//!   `garden.access-off` and `garden.delete` are listed (they are not in
//!   0.46.3) and every check answers `not_available` with K2's off words.
//!   Those lines are written out explicitly, not captured.
//!
//! Temp HOME, this thread's DB, a temp Zen folder. Fails loudly on any
//! difference or a missing golden. `GARDEN_ACCESS_GOLDEN_BLESS=1` writes the
//! file (only ever from a base without Garden access, for the 0.46.3 lines).

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::permission::actions::{self, Check, Ctx};

const WS_ID: &str = "11111111-2222-3333-4444-555555555555";
const WS_HANDLE: &str = "golden-billing";

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/garden_access_off/catalog_rows.txt")
}

fn ctx(params: Value) -> Ctx {
    Ctx {
        workspace_id: WS_ID.into(),
        workspace_handle: WS_HANDLE.into(),
        params: params.as_object().cloned().expect("params are an object"),
    }
}

/// `<id> <params>: <check>`, or `not registered`.
fn line(id: &str, params: Value) -> String {
    let shown = params.to_string();
    match actions::find(id) {
        None => format!("{id} {shown}: not registered\n"),
        Some(r) => format!("{id} {shown}: {:?}\n", (r.check)(&ctx(params))),
    }
}

#[test]
fn garden_catalog_rows_with_the_switch_off() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    assert!(!k2_core::zen::garden_access::enabled(), "the switch is off by default");
    let f = k2_core::zen::ZenFiles::local();
    f.setup().expect("zen setup");
    let g = f.new_garden("Sales board", None, None, None).expect("a Garden");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![WS_ID, WS_HANDLE, "/tmp/garden-access-off-golden/billing"],
        )
        .expect("seed the asking workspace");
    }

    let mut text = String::from("# Garden rows of the permission catalog, Garden access lists OFF\n");
    text.push_str("# 0.46.3 (captured from integ/0.46.3-r9)\n");
    let version = "ab".repeat(32);
    text.push_str(&line("garden.keep-mine", json!({ "widget": "board", "version": version })));
    text.push_str(&line("garden.proposal.approve", json!({ "proposal": "zp-none" })));
    text.push_str(&line("garden.proposal.approve", json!({})));
    text.push_str("# G0a item 4 (named exception, not in 0.46.3): listed; every check answers not_available\n");
    for id in ["garden.access", "garden.access-off", "garden.delete"] {
        text.push_str(&line(id, json!({ "garden": g.id })));
    }
    let text = text.replace(&g.id, "<GARDEN>");

    if std::env::var("GARDEN_ACCESS_GOLDEN_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(golden().parent().expect("parent")).expect("mkdir");
        std::fs::write(golden(), &text).expect("write golden");
        return;
    }
    let want = std::fs::read_to_string(golden())
        .unwrap_or_else(|e| panic!("no golden at {} ({e}); capture the 0.46.3 lines from a base without Garden access", golden().display()));
    assert_eq!(text, want, "the Garden catalog rows drifted with the switch off");

    // The exception, asserted directly as well (not only through the file).
    for id in ["garden.access", "garden.access-off", "garden.delete"] {
        let r = actions::find(id).unwrap_or_else(|| panic!("{id} is listed while off"));
        assert_eq!(
            (r.check)(&ctx(json!({ "garden": g.id }))),
            Check::Unavailable { code: "not_available", why: crate::zen_garden_cards::OFF_WORDS.to_string() },
            "{id}"
        );
    }
}
