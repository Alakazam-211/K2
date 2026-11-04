//! Live sources through the phone door (contract-gardens-phone rev 2 §D, §E,
//! §G; plan B1): a placement reads only what it binds, every refusal by its
//! exact code, the changes route's `full` rule, and that no token, secret name,
//! fetch URL or header reaches a phone. Temp HOME, a scoped DB, the door's own
//! fixture. Fail loudly.

use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Value as J};

use k2_core::sources::extract::{ProjectedItem, Projection};
use k2_core::sources::{store, Health};
use k2_core::zen::ZenFile;

use super::*;
use crate::compute_routes::Who;

const CAPS: &[&str] = &["sources:read"];

/// A marker for each thing that must never reach a phone.
const SECRET_NAME: &str = "MARKERSECRETNAME";
const SECRET_VALUE: &str = "markersecretvalue-do-not-leak";
const URL_MARK: &str = "markerurlpart";
const HEADER_MARK: &str = "markerheadervalue";
const HEADER_NAME: &str = "X-Markerheadername";

fn page(config: &str, phone: &str) -> String {
    format!(
        "schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\
[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n[widget.props.config]\n{config}\n{phone}"
    )
}

fn write_page(fx: &Fx, text: &str) {
    std::fs::write(fx.d.f.path_of(&ZenFile::Garden(fx.d.garden.clone())), text).expect("write the Garden");
}

fn add(spec: J) -> J {
    let r = crate::sources_routes::handle("/cli/sources/add", Who::Owner, spec.to_string().as_bytes(), &HashMap::new());
    assert!(r.status.starts_with('2'), "add: {}: {}", r.status, r.body);
    serde_json::from_str(&r.body).expect("add answers JSON")
}

fn plain(name: &str) -> J {
    json!({ "name": name, "label": format!("The {name}"), "everySecs": 60, "fetch": { "url": format!("https://{name}.example.com/status") },
            "extract": { "fields": { "version": "/version" } } })
}

/// A source with a secret in every place a definition can hold one, and a
/// live card view with values in it.
fn secret_source(name: &str) -> String {
    add(json!({ "name": name, "label": "Deploys", "everySecs": 180, "secret": SECRET_NAME,
                "headers": [[HEADER_NAME, HEADER_MARK]],
                "fetch": { "url": format!("https://api.example.com/v6/deployments?team={URL_MARK}") },
                "extract": { "fields": { "version": "/version" } } }));
    seed(name, json!({ "version": "1.2.3", "nested": { "a": 1 } }));
    name.to_string()
}

fn seed(name: &str, fields: J) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = store::get_by_name(&conn, name).expect("db").unwrap_or_else(|| panic!("no source {name}"));
    let (mut st, _) = store::load_state(&conn, &row.id).expect("db").expect("a state row");
    st.phase = k2_core::sources::Phase::Live;
    st.health = Health::Ok;
    let p = Projection {
        fields: fields.as_object().expect("fields").iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        items: vec![ProjectedItem { key: "d1".into(), fields: [("state".to_string(), json!("READY"))].into(), list: None }],
    };
    store::save_state(&conn, &st, Some(&p)).expect("save state");
}

fn call_ok(fx: &Fx, verb: &str, args: J) -> J {
    let v = ok_of(CALL, &fx.phone, &[], call_body(fx, verb, args));
    assert_eq!(v["ok"], true, "{v}");
    v["result"].clone()
}

fn call_no(fx: &Fx, verb: &str, args: J) -> (u16, J) {
    let (s, v) = req(CALL, &fx.phone, &[], call_body(fx, verb, args));
    assert_ne!(s, 200, "{verb} should refuse: {v}");
    assert_eq!(v["ok"], false, "the door's envelope: {v}");
    assert!(v["error"]["hint"].is_string(), "a hint: {v}");
    (s, v["error"].clone())
}

fn changes(fx: &Fx, since: &str) -> J {
    let hash = fx.d.hash();
    ok_of(SOURCES_CHANGES, &fx.phone, &[("garden", &fx.d.garden), ("placement", "b1"), ("hash", &hash), ("since", since)], J::Null)
}

// ── reads ───────────────────────────────────────────────────────────

/// What a widget gets is the catalog's `SourceView`: its keys and nothing else.
#[test]
fn a_read_is_the_card_view_with_its_address_and_reach() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    seed("fleet", json!({ "version": "1.2.3", "nested": { "a": 1 } }));
    write_page(&fx, &page("source = \"fleet\"", ""));
    let v = call_ok(&fx, "sources.read", json!(["fleet"]));
    assert_eq!(v["address"], "fleet");
    assert_eq!(v["reach"], "ok");
    assert_eq!(v["name"], "fleet");
    assert_eq!(v["label"], "The fleet");
    assert_eq!(v["health"], "ok");
    assert_eq!(v["phase"], "live");
    assert_eq!(v["fields"], json!({ "version": "1.2.3", "nested": "{\"a\":1}" }), "scalars only");
    assert_eq!(v["items"][0]["fields"], json!({ "state": "READY" }));
    // The catalog's schema names every key.
    let row = k2_core::contract::catalog().verb("sources.read").expect("the row");
    let allowed: BTreeSet<&String> = row.response["properties"].as_object().expect("properties").keys().collect();
    let got: BTreeSet<&String> = v.as_object().expect("a view").keys().collect();
    assert_eq!(got, allowed, "exactly the catalog's SourceView");
}

#[test]
fn list_shows_what_the_placement_binds_and_says_what_it_can_t_read() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    add(plain("other"));
    add(plain("gone"));
    seed("fleet", json!({ "version": "1" }));
    let r = crate::sources_routes::handle("/cli/sources/remove", Who::Owner, json!({ "name": "gone" }).to_string().as_bytes(), &HashMap::new());
    assert!(r.status.starts_with('2'), "{}", r.body);
    write_page(&fx, &page("sources = \"fleet, gone, never, far::elsewhere.example.com\"", ""));
    let rows = call_ok(&fx, "sources.list", json!([]));
    assert_eq!(
        rows,
        json!([
            { "address": "fleet", "label": "The fleet", "health": "ok", "phase": "live", "display": "ok", "reach": "ok" },
            { "address": "gone", "label": null, "health": null, "phase": null, "display": null, "reach": "removed" },
            { "address": "never", "label": null, "health": null, "phase": null, "display": null, "reach": "unreachable" },
            { "address": "far::elsewhere.example.com", "label": null, "health": null, "phase": null, "display": null, "reach": "server_not_saved" },
        ]),
        "and `other`, which exists, isn't listed"
    );
}

#[test]
fn history_answers_for_a_bound_source() {
    let fx = fx(CAPS);
    add(json!({ "name": "fleet", "label": "Fleet", "everySecs": 60, "fetch": { "targets": [{ "key": "a", "url": "https://a.example.com/status" }] },
                "extract": { "items": { "itemFields": { "version": "/version" } }, "aggregates": { "n": { "op": "count" } }, "trend": ["n"] } }));
    let now = chrono::Utc::now().timestamp();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = store::get_by_name(&conn, "fleet").expect("db").expect("row");
        store::insert_points(&conn, &row.id, now - 30, &[("n".into(), Some(4.0))]).expect("points");
    }
    write_page(&fx, &page("source = \"fleet\"", ""));
    let v = call_ok(&fx, "sources.history", json!(["fleet", "n"]));
    assert_eq!(v["field"], "n");
    assert_eq!(v["points"][0]["v"], json!(4.0), "{v}");
    let older = call_ok(&fx, "sources.history", json!(["fleet", "n", { "fromAt": now - 3600, "toAt": now - 600 }]));
    assert_eq!(older["points"], json!([]), "the window is honoured: {older}");
    let health = call_ok(&fx, "sources.history", json!(["fleet", "health"]));
    assert_eq!(health["field"], "health");
}

#[test]
fn refresh_queues_and_answers_only_that() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    let v = call_ok(&fx, "sources.refresh", json!(["fleet"]));
    assert_eq!(v["queued"], true, "{v}");
    assert_eq!(v.as_object().expect("an object").keys().map(String::as_str).collect::<Vec<_>>(), ["nextAt", "queued"], "{v}");
}

// ── which sources (the widget's own only) ───────────────────────────

/// `sources.read` honours the merged config: a phone's own may name a source,
/// and an override takes the desktop's out on the phone.
#[test]
fn the_bound_set_is_the_merged_config() {
    let fx = fx(CAPS);
    for n in ["fleet", "small", "extra"] {
        add(plain(n));
    }
    let phone = "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n[phone.screen.widget.config]\nsource = \"small\"\nlatest = \"extra\"\n";
    write_page(&fx, &page("source = \"fleet\"", phone));
    call_ok(&fx, "sources.read", json!(["small"]));
    call_ok(&fx, "sources.read", json!(["extra"]));
    let (s, e) = call_no(&fx, "sources.read", json!(["fleet"]));
    assert_eq!((s, e["code"].as_str(), e["room"].as_str()), (403, Some("not_bound"), Some("fleet")), "the phone's key won: {e}");
    let names: Vec<String> = call_ok(&fx, "sources.list", json!([])).as_array().expect("rows").iter().map(|r| r["address"].as_str().expect("address").to_string()).collect();
    assert_eq!(names, ["small", "extra"]);

    // With no layout, and for a placement the layout leaves out: the desktop's.
    write_page(&fx, &page("source = \"fleet\"", ""));
    call_ok(&fx, "sources.read", json!(["fleet"]));
    assert_eq!(call_no(&fx, "sources.read", json!(["small"])).1["code"], "not_bound");
    let leaves_out = format!(
        "{}[[widget]]\nid = \"b2\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b2\"\n[phone.screen.widget.config]\nsource = \"small\"\n",
        page("source = \"fleet\"", "")
    );
    write_page(&fx, &leaves_out);
    call_ok(&fx, "sources.read", json!(["fleet"]));
    assert_eq!(call_no(&fx, "sources.read", json!(["small"])).1["code"], "not_bound", "b2's config isn't b1's");
}

#[test]
fn every_verb_refuses_a_source_the_placement_does_not_bind() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    add(plain("private"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    for (verb, args) in [
        ("sources.read", json!(["private"])),
        ("sources.history", json!(["private", "health"])),
        ("sources.refresh", json!(["private"])),
        ("sources.read", json!(["private::Elsewhere.example.com"])),
    ] {
        let (s, e) = call_no(&fx, verb, args.clone());
        assert_eq!((s, e["code"].as_str()), (403, Some("not_bound")), "{verb} {args}: {e}");
    }
    // The manifest's fixed names bind the same way.
    fx.d.f.widget_dir("board");
    let manifest = "{\n  \"schema\": 1,\n  \"name\": \"Board\",\n  \"entry\": \"index.html\",\n  \"caps\": [\"sources:read\"],\n  \"sources\": [\"private\"]\n}\n";
    std::fs::write(fx.d.f.widget_dir("board").join("manifest.json"), manifest).expect("manifest");
    fx.d.stamp(k2_core::garden_data::MadeBy::Owner);
    call_ok(&fx, "sources.read", json!(["private"]));
    assert_eq!(call_no(&fx, "sources.read", json!(["fleet"])).1["code"], "not_bound", "the config isn't read for a fixed list");
}

#[test]
fn another_server_is_server_not_saved() {
    let fx = fx(CAPS);
    write_page(&fx, &page("source = \"far::elsewhere.example.com\"", ""));
    for (verb, args) in [("sources.read", json!(["far::elsewhere.example.com"])), ("sources.refresh", json!(["far::ELSEWHERE.example.com"])), ("sources.history", json!(["far::elsewhere.example.com", "health"]))] {
        let (s, e) = call_no(&fx, verb, args);
        assert_eq!((s, e["code"].as_str()), (409, Some("server_not_saved")), "{verb}: {e}");
    }
}

// ── every other refusal ─────────────────────────────────────────────

#[test]
fn no_cap_is_cap_not_granted() {
    let fx = fx(&["store:read"]);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    for (verb, args) in [("sources.list", json!([])), ("sources.read", json!(["fleet"])), ("sources.history", json!(["fleet", "health"])), ("sources.refresh", json!(["fleet"]))] {
        let (s, e) = call_no(&fx, verb, args);
        assert_eq!((s, e["code"].as_str()), (403, Some("cap_not_granted")), "{verb}: {e}");
    }
    let hash = fx.d.hash();
    let g = fx.d.garden.clone();
    assert_eq!(refused(SOURCES_CHANGES, &fx.phone, &[("garden", &g), ("placement", "b1"), ("hash", &hash)], J::Null), (403, "cap_not_granted".into()));
}

#[test]
fn an_unsigned_version_is_needs_editor() {
    struct SwitchOn;
    impl Drop for SwitchOn {
        fn drop(&mut self) {
            k2_core::zen::garden_access::set_switch(false);
        }
    }
    let d = crate::garden_data::call::door_tests::door(CAPS);
    login("rosphone", Role::Admin);
    link(&["rosphone"]);
    let phone = pair("rosphone", "ios-rosson-1");
    add(plain("fleet"));
    std::fs::write(d.f.path_of(&ZenFile::Garden(d.garden.clone())), page("source = \"fleet\"", "")).expect("page");
    k2_core::zen::garden_access::set_switch(true);
    let _on = SwitchOn;
    let hash = d.hash();
    let body = json!({ "garden": d.garden, "placement": "b1", "hash": hash, "verb": "sources.read", "args": ["fleet"] });
    let (s, c) = refused(CALL, &phone, &[], body);
    assert_eq!((s, c.as_str()), (403, "needs_editor"));
    let g = d.garden.clone();
    let (s, c) = refused(SOURCES_CHANGES, &phone, &[("garden", &g), ("placement", "b1"), ("hash", &hash)], J::Null);
    assert_eq!((s, c.as_str()), (403, "needs_editor"));
}

#[test]
fn a_stale_hash_or_placement_is_refused() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    let g = fx.d.garden.clone();
    let stale = "0".repeat(64);
    let body = json!({ "garden": g, "placement": "b1", "hash": stale, "verb": "sources.read", "args": ["fleet"] });
    assert_eq!(refused(CALL, &fx.phone, &[], body), (409, "stale_widget".into()));
    assert_eq!(refused(SOURCES_CHANGES, &fx.phone, &[("garden", &g), ("placement", "b1"), ("hash", &stale)], J::Null), (409, "stale_widget".into()));
    let hash = fx.d.hash();
    let (s, c) = refused(SOURCES_CHANGES, &fx.phone, &[("garden", &g), ("placement", "nope"), ("hash", &hash)], J::Null);
    assert_eq!((s, c.as_str()), (404, "not_found"));
    for missing in [vec![("placement", "b1"), ("hash", hash.as_str())], vec![("garden", g.as_str()), ("hash", hash.as_str())], vec![("garden", g.as_str()), ("placement", "b1")]] {
        assert_eq!(refused(SOURCES_CHANGES, &fx.phone, &missing, J::Null), (400, "usage".into()), "{missing:?}");
    }
}

#[test]
fn bad_arguments_are_failed() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    for (verb, args) in [
        ("sources.read", json!([])),
        ("sources.read", json!([7])),
        ("sources.read", json!([" "])),
        ("sources.read", json!(["Not An Address"])),
        ("sources.history", json!(["fleet"])),
        ("sources.history", json!(["fleet", ""])),
    ] {
        let (s, e) = call_no(&fx, verb, args.clone());
        assert_eq!((s, e["code"].as_str()), (400, Some("failed")), "{verb} {args}: {e}");
    }
}

#[test]
fn a_removed_or_unknown_source_says_so() {
    let fx = fx(CAPS);
    add(plain("gone"));
    let r = crate::sources_routes::handle("/cli/sources/remove", Who::Owner, json!({ "name": "gone" }).to_string().as_bytes(), &HashMap::new());
    assert!(r.status.starts_with('2'), "{}", r.body);
    write_page(&fx, &page("sources = \"gone, never\"", ""));
    let (s, e) = call_no(&fx, "sources.read", json!(["gone"]));
    assert_eq!((s, e["code"].as_str()), (410, Some("removed")), "{e}");
    let (s, e) = call_no(&fx, "sources.read", json!(["never"]));
    assert_eq!((s, e["code"].as_str(), e["cause"].as_str()), (404, Some("failed"), Some("source_not_found")), "{e}");
}

#[test]
fn a_source_that_keeps_states_has_no_history() {
    let fx = fx(CAPS);
    let mut spec = plain("states");
    spec["keep"] = json!("states");
    add(spec);
    write_page(&fx, &page("source = \"states\"", ""));
    let (s, e) = call_no(&fx, "sources.history", json!(["states", "version"]));
    assert_eq!((s, e["code"].as_str()), (409, Some("no_history")), "{e}");
}

/// The owner person's login reads an `admin` source; taken down to Member it's
/// no longer the owner person, and the door itself shuts on every row.
#[test]
fn a_login_taken_down_to_member_loses_the_door() {
    let fx = fx(CAPS);
    let mut spec = plain("admins");
    spec["viewers"] = json!("admin");
    add(spec);
    write_page(&fx, &page("source = \"admins\"", ""));
    call_ok(&fx, "sources.read", json!(["admins"]));
    k2_core::connect_users::set_role("rosphone", Role::Member).expect("set_role");
    for (verb, args) in [("sources.read", json!(["admins"])), ("sources.list", json!([])), ("sources.refresh", json!(["admins"]))] {
        let (s, e) = call_no(&fx, verb, args);
        assert_eq!((s, e["code"].as_str()), (403, Some("garden_person_only")), "{verb}: {e}");
    }
    let hash = fx.d.hash();
    let g = fx.d.garden.clone();
    assert_eq!(refused(SOURCES_CHANGES, &fx.phone, &[("garden", &g), ("placement", "b1"), ("hash", &hash)], J::Null), (403, "garden_person_only".into()));
    // The floor itself is the sources routes' (`can_read`), which this row calls.
    let member = Who::Login { username: "rosphone".into(), role: Role::Member };
    let params: HashMap<String, String> = [("name".to_string(), "admins".to_string())].into();
    assert_eq!(crate::sources_routes::read(&member, &params).expect_err("a Member can't").code, "cap_not_granted");
}

/// The placement's read budget: a drained one answers `rate_limited`.
#[test]
fn reads_spend_the_placement_s_budget() {
    use crate::garden_data::budget::{self, BudgetKey, Spend};
    let fx = fx(CAPS);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    let key = BudgetKey::Placement { garden: fx.d.garden.clone(), placement: "b1".into() };
    let mut drained = false;
    for _ in 0..1000 {
        if budget::spend(&key, Spend::Read).is_err() {
            drained = true;
            break;
        }
    }
    assert!(drained, "the bucket has a bottom");
    let (s, e) = call_no(&fx, "sources.read", json!(["fleet"]));
    assert_eq!((s, e["code"].as_str()), (429, Some("rate_limited")), "{e}");
}

/// Only the door answers these rows. A desktop frame's bridge is the renderer's,
/// and the local door still refuses them, so nothing widened.
#[test]
fn the_local_door_still_refuses_them_and_the_table_says_where_each_runs() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    for verb in ["sources.list", "sources.read", "sources.history", "sources.refresh"] {
        assert_eq!(fx.d.refuse(verb, json!(["fleet"])), "not_exposed", "{verb} on /cli/zen/call");
        assert_eq!(k2_core::contract::phone::phone_of(catalog(), verb), Some(Phone::Door), "{verb}");
    }
    assert_eq!(k2_core::contract::phone::phone_of(catalog(), "sources.subscribe"), Some(Phone::Host));
    let (s, e) = call_no(&fx, "sources.subscribe", json!(["fleet"]));
    assert_eq!((s, e["code"].as_str()), (400, Some("not_exposed")), "the phone answers a subscribe: {e}");
    let (s, e) = call_no(&fx, "sources.ask", json!(["fleet"]));
    assert_eq!((s, e["code"].as_str()), (403, Some("not_exposed")), "{e}");
}

// ── secrets ─────────────────────────────────────────────────────────

fn assert_clean(what: &str, text: &str) {
    for marker in [SECRET_NAME, SECRET_VALUE, URL_MARK, HEADER_MARK, HEADER_NAME, "api.example.com", "secretName", "urlSecret", "\"fetch\"", "\"headers\"", "madeBy"] {
        assert!(!text.contains(marker), "{what} leaks {marker}: {text}");
    }
}

/// §D.1: a source with a marker in its secret's name, its URL and its header.
/// Every door answer about it is searched: no marker, no banned field, and the
/// data is there.
#[test]
fn no_token_secret_name_url_or_header_reaches_a_phone() {
    let fx = fx(CAPS);
    let name = secret_source("deploys");
    write_page(&fx, &page(&format!("source = \"{name}\""), ""));
    // The fixture really holds them: the owner's own export has them.
    let export = crate::sources_routes::handle("/cli/sources/export", Who::Owner, b"", &[("name".to_string(), name.clone())].into());
    assert!(export.body.contains(SECRET_NAME) && export.body.contains(URL_MARK), "the test would prove nothing: {}", export.body);

    let read = call_ok(&fx, "sources.read", json!([name]));
    assert_eq!(read["fields"]["version"], "1.2.3", "a read with a credential returns the data");
    let hash = fx.d.hash();
    let g = fx.d.garden.clone();
    let mut answers: Vec<(&str, J)> = vec![
        ("read", read),
        ("list", call_ok(&fx, "sources.list", json!([]))),
        ("history", call_ok(&fx, "sources.history", json!([name, "health"]))),
        ("refresh", call_ok(&fx, "sources.refresh", json!([name]))),
        ("changes", ok_of(SOURCES_CHANGES, &fx.phone, &[("garden", &g), ("placement", "b1"), ("hash", &hash), ("since", "0")], J::Null)),
        ("page", ok_of(PAGE, &fx.phone, &[("garden", &g)], J::Null)),
    ];
    let (_, refusal) = call_no(&fx, "sources.history", json!([name, "not-a-field"]));
    answers.push(("a refusal", refusal));
    let banned = &catalog().banned_fields;
    for (what, v) in &answers {
        assert_clean(what, &v.to_string());
        let mut found = Vec::new();
        banned_keys(v, "", banned, &mut found);
        assert!(found.is_empty(), "{what}: {found:?}");
    }
    assert_eq!(answers[4].1["changed"][0]["fields"]["version"], "1.2.3", "the changes carry the data too");
    // Nor do the audit lines name the source.
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit log");
    assert!(!audit.contains(URL_MARK) && !audit.contains(SECRET_NAME), "{audit}");
}

// ── the audit log ───────────────────────────────────────────────────

fn audit_text() -> String {
    std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit log")
}

/// A refresh that queues is a real fetch, so each logs; one refused logs its code.
#[test]
fn a_refresh_that_queues_always_logs() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    for _ in 0..3 {
        call_ok(&fx, "sources.refresh", json!(["fleet"]));
    }
    let queued = audit_text().lines().filter(|l| l.contains("verb=sources.refresh queued")).count();
    assert_eq!(queued, 3, "every one, unthinned");
    call_no(&fx, "sources.refresh", json!(["private"]));
    assert!(audit_text().contains("verb=sources.refresh refused code=not_bound"), "{}", audit_text());
    assert!(!audit_text().contains("private"), "its args never go in the log");
}

#[test]
fn a_refused_sources_changes_logs_its_code() {
    let fx = fx(CAPS);
    let g = fx.d.garden.clone();
    let stale = "0".repeat(64);
    assert_eq!(refused(SOURCES_CHANGES, &fx.phone, &[("garden", &g), ("placement", "b1"), ("hash", &stale)], J::Null), (409, "stale_widget".into()));
    assert!(audit_text().lines().any(|l| l.contains("zen.remote.sources_changes") && l.contains("refused code=stale_widget")), "{}", audit_text());
}

// ── sources-changes (§G) ────────────────────────────────────────────

#[test]
fn changes_cover_only_what_the_placement_binds() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    add(plain("other"));
    add(plain("gone"));
    write_page(&fx, &page("sources = \"fleet, gone\"", ""));
    let first = changes(&fx, "0");
    assert_eq!(first["full"], true, "{first}");
    let names = |v: &J| -> Vec<String> { v["changed"].as_array().expect("changed").iter().map(|c| c["address"].as_str().expect("address").to_string()).collect() };
    assert_eq!(names(&first), ["fleet", "gone"], "not `other`");
    assert_eq!(first["changed"][0]["reach"], "ok");
    assert_eq!(first["removed"], json!([]));
    let rev = first["rev"].as_i64().expect("rev").to_string();
    let quiet = changes(&fx, &rev);
    assert_eq!((quiet["full"].as_bool(), quiet["changed"].clone(), quiet["removed"].clone()), (Some(false), json!([]), json!([])), "{quiet}");

    // A change to a source it doesn't bind isn't its business.
    seed("other", json!({ "version": "9" }));
    let v = changes(&fx, &rev);
    assert_eq!((v["full"].as_bool(), names(&v).len()), (Some(false), 0), "{v}");
    seed("fleet", json!({ "version": "2" }));
    let v = changes(&fx, &rev);
    assert_eq!(names(&v), ["fleet"], "{v}");
    assert_eq!(v["changed"][0]["fields"]["version"], "2");
    let rev = v["rev"].as_i64().expect("rev").to_string();
    for removed in ["gone", "other"] {
        let r = crate::sources_routes::handle("/cli/sources/remove", Who::Owner, json!({ "name": removed }).to_string().as_bytes(), &HashMap::new());
        assert!(r.status.starts_with('2'), "{}", r.body);
    }
    let v = changes(&fx, &rev);
    assert_eq!(v["removed"], json!(["gone"]), "not `other`: {v}");
}

/// Any `since` the daemon can't honour answers `full: true`: at or below zero,
/// not a number, older than the last reaped removal, or ahead of `rev`.
#[test]
fn a_since_it_cannot_honour_is_full() {
    let fx = fx(CAPS);
    add(plain("fleet"));
    write_page(&fx, &page("source = \"fleet\"", ""));
    let rev = changes(&fx, "0")["rev"].as_i64().expect("rev");
    let ahead = (rev + 1000).to_string();
    for since in ["0", "-5", "soon", "", "99999999999999999999999", ahead.as_str()] {
        let v = changes(&fx, since);
        assert_eq!(v["full"], true, "since={since}: {v}");
        assert_eq!(v["changed"][0]["name"], "fleet", "since={since}: {v}");
        assert_eq!(v["rev"], json!(rev), "since={since}: {v}");
    }
    let hash = fx.d.hash();
    let g = fx.d.garden.clone();
    let none = ok_of(SOURCES_CHANGES, &fx.phone, &[("garden", &g), ("placement", "b1"), ("hash", &hash)], J::Null);
    assert_eq!(none["full"], true, "no since at all: {none}");
    assert_eq!(changes(&fx, &rev.to_string())["full"], false);
    assert!(!crate::sources_routes::since_can_be_honoured(5, 6, 10), "older than the last reaped removal");
    assert!(crate::sources_routes::since_can_be_honoured(6, 6, 10));
    assert!(crate::sources_routes::since_can_be_honoured(10, 6, 10));
    assert!(!crate::sources_routes::since_can_be_honoured(11, 6, 10), "ahead of rev");
    assert!(!crate::sources_routes::since_can_be_honoured(0, 0, 10));
}

/// The widget's frame can't call this route: it isn't a verb.
#[test]
fn sources_changes_is_not_a_verb() {
    let fx = fx(CAPS);
    let (s, e) = call_no(&fx, "sources.changes", json!([]));
    assert_eq!((s, e["code"].as_str()), (400, Some("unknown_verb")), "{e}");
    assert!(GET_ROUTES.contains(&SOURCES_CHANGES) && !POST_ROUTES.contains(&SOURCES_CHANGES));
    assert!(crate::routes::route_policy::lookup(SOURCES_CHANGES).is_some(), "a policy row");
}

/// The hello says `sources-v1` now that its rows answer.
#[test]
fn the_phone_hello_lists_sources() {
    assert!(phone_features().contains(&"sources-v1"), "{:?}", phone_features());
}
