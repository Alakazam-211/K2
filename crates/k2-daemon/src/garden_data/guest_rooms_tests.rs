//! RS-A15 through the widget door: `guests.rooms` / `guests.setRooms` with
//! owner rights, the confirm every write needs (bound to the exact change,
//! once), creator rights for agent-made widgets, and the manifest cap.

use std::collections::HashMap;

use serde_json::{json, Value};

use k2_core::garden_data::authors::{self, CallerKind, StampSource};
use k2_core::garden_data::MadeBy;
use k2_core::skin::{self, guest_rooms as gr};
use k2_core::zen::{ZenFile, ZenFiles};

use super::CONFIRM_ACTION;
use crate::garden_data::agent_routes::tests::{env, Env};
use crate::zen_data_routes::{handle, DataWho};

const APP: &str = "POST /cli/zen/call HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: tauri://localhost\r\n\r\n";
/// The Garden page with one `guestdesk` placement (its id unique per test:
/// widget pauses are process-wide, keyed by Garden and placement).
fn page(placement: &str) -> String {
    format!(
        "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"{placement}\"\nkind = \"custom\"\nwidget = \"guestdesk\"\ncolumn = 0\n"
    )
}

struct Desk {
    _env: Env,
    f: ZenFiles,
    garden: String,
    placement: String,
    /// Jobs: published in w-builder; Other: in w-keeper.
    jobs: String,
    other: String,
}

fn desk(caps: &[&str], placement: &str) -> Desk {
    let e = env();
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    f.new_widget("guestdesk", Some("hello")).expect("widget folder");
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    std::fs::write(
        f.widget_dir("guestdesk").join("manifest.json"),
        format!("{{\n  \"schema\": 1,\n  \"name\": \"Guest desk\",\n  \"entry\": \"index.html\",\n  \"caps\": [{}]\n}}\n", caps.join(", ")),
    )
    .expect("manifest");
    let garden = f.gardens()[0].id.clone();
    std::fs::write(f.path_of(&ZenFile::Garden(garden.clone())), page(placement)).expect("garden page");
    let (jobs, other) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let j = k2_core::published_services::insert(&conn, "w-builder", "jobs", "node s.js", "/tmp", 4400, "local", "running", "cmd", "").unwrap();
        let o = k2_core::published_services::insert(&conn, "w-keeper", "other", "node s.js", "/tmp", 4401, "local", "running", "cmd", "").unwrap();
        (j.id, o.id)
    };
    skin::add_principal_for_app("dana", None, &jobs, None, "test").unwrap();
    skin::add_principal_for_app("erin", None, &other, None, "test").unwrap();
    Desk { _env: e, f, garden, placement: placement.to_string(), jobs, other }
}

impl Desk {
    fn hash(&self) -> String {
        self.f.garden_widget_status(&self.garden, "guestdesk").hash().to_string()
    }

    fn stamp(&self, maker: MadeBy) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let kind = if maker == MadeBy::Owner { CallerKind::Owner } else { CallerKind::Agent };
        authors::stamp(&conn, "guestdesk", &self.hash(), &maker, StampSource::Sign, kind, 1).expect("stamp");
    }

    fn raw(&self, verb: &str, args: Value, confirm: Option<&str>) -> (u16, Value) {
        let mut body = json!({ "garden": self.garden, "placement": self.placement, "hash": self.hash(), "verb": verb, "args": [args] });
        if let Some(t) = confirm {
            body["confirmToken"] = json!(t);
        }
        let r = handle("/cli/zen/call", DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), APP);
        let code: u16 = r.status.split(' ').next().unwrap().parse().unwrap();
        (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{verb}: {e}: {}", r.body)))
    }

    /// The person's confirm, as K2's sheet gets it (`vault/confirm/finish`).
    fn person_confirms(&self, secret: &str) -> String {
        crate::apps::egress_routes::test_seams::vault_handle().confirm.mint(CONFIRM_ACTION, Some(secret), "owner", "local-auth")
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

#[test]
fn every_write_waits_for_the_persons_confirm_bound_to_the_change() {
    let d = desk(&["guests:rooms"], "gr-confirm");
    d.stamp(MadeBy::Owner);
    // Reads: owner rights, any local App, no confirm.
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["result"]["rooms"], json!([]));
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.other, "username": "erin" }), None);
    assert_eq!(st, 200, "any local App's guests: {v}");

    // A write without the person: refused, with the exact change for K2's sheet.
    let args = json!({ "app": d.jobs, "username": "dana", "rooms": ["board-keeper"] });
    let (st, v) = d.raw("guests.setRooms", args.clone(), None);
    assert_eq!((st, code(&v)), (403, "confirm_required"), "{v}");
    let e = &v["error"];
    assert_eq!(e["action"], json!(CONFIRM_ACTION));
    assert_eq!(e["change"]["username"], json!("dana"));
    assert_eq!(e["change"]["appName"], json!("jobs"));
    assert_eq!(e["change"]["afterHandles"], json!(["board-keeper"]));
    assert_eq!(e["change"]["before"], json!([]));
    let secret = e["secretId"].as_str().unwrap().to_string();
    assert!(gr::read(&d.jobs, "dana").unwrap().rooms.is_empty(), "nothing written before the confirm");

    // A confirm for another change (or another action) doesn't open this one.
    let wrong = d.person_confirms("guests:0000");
    let (st, v) = d.raw("guests.setRooms", args.clone(), Some(&wrong));
    assert_eq!((st, code(&v)), (403, "confirm_required"), "{v}");
    let other_action = crate::apps::egress_routes::test_seams::vault_handle().confirm.mint("source-batch", Some(&secret), "owner", "local-auth");
    let (st, _) = d.raw("guests.setRooms", args.clone(), Some(&other_action));
    assert_eq!(st, 403);
    // A forged token: refused.
    let (st, _) = d.raw("guests.setRooms", args.clone(), Some("k2vc_forged"));
    assert_eq!(st, 403);

    // The person's confirm for exactly this change: written, once.
    let token = d.person_confirms(&secret);
    let (st, v) = d.raw("guests.setRooms", args.clone(), Some(&token));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["result"]["changed"], json!(true));
    assert_eq!(v["result"]["roomHandles"], json!(["board-keeper"]));
    // The same rooms again: a no-op needs no confirm.
    let (st, v) = d.raw("guests.setRooms", args.clone(), None);
    assert_eq!((st, v["result"]["changed"].clone()), (200, json!(false)), "{v}");
    // A token works once: a new change with the spent token is refused.
    let (st, v) = d.raw("guests.setRooms", json!({ "app": d.jobs, "username": "dana", "rooms": [] }), Some(&token));
    assert_eq!((st, code(&v)), (403, "confirm_required"), "{v}");
    // Audited with via: garden and the widget.
    let rows = gr::audit_rows(&d.jobs, 5).unwrap();
    let change = rows.iter().find(|r| r.event == gr::AUDIT_CHANGE).expect("audited");
    assert_eq!(change.actor, "garden:guestdesk");
    assert_eq!(change.payload["via"], json!("garden"));
    assert_eq!(change.payload["after"], json!(["w-keeper"]));
}

#[test]
fn the_cap_the_app_and_the_guest_are_checked() {
    let d = desk(&["store:read"], "gr-cap");
    d.stamp(MadeBy::Owner);
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!((st, code(&v)), (403, "cap_not_granted"), "{v}");
    let d2 = desk_again(&d);
    let (st, v) = d2.raw("guests.rooms", json!({ "username": "dana" }), None);
    assert_eq!((st, code(&v)), (400, "usage"), "a Garden names the App: {v}");
    let (st, v) = d2.raw("guests.rooms", json!({ "app": "no-such-app", "username": "dana" }), None);
    assert_eq!((st, code(&v)), (404, "not_found"), "{v}");
    let (st, v) = d2.raw("guests.rooms", json!({ "app": d.jobs, "username": "erin" }), None);
    assert_eq!((st, code(&v)), (404, "not_found"), "another App's guest: {v}");
}

/// Re-point the same desk's manifest at `guests:rooms` (a new hash, signed by the owner).
fn desk_again(d: &Desk) -> &Desk {
    std::fs::write(
        d.f.widget_dir("guestdesk").join("manifest.json"),
        "{\n  \"schema\": 1,\n  \"name\": \"Guest desk\",\n  \"entry\": \"index.html\",\n  \"caps\": [\"guests:rooms\"]\n}\n",
    )
    .expect("manifest");
    d.stamp(MadeBy::Owner);
    d
}

#[test]
fn an_agent_made_widget_acts_with_its_makers_rights() {
    let d = desk(&["guests:rooms"], "gr-agent");
    d.stamp(MadeBy::Agent { project_id: "w-builder".into() });
    // Another workspace's App: not reachable.
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.other, "username": "erin" }), None);
    assert_eq!((st, code(&v)), (404, "not_found"), "{v}");
    // Its own workspace's App, without the owner's grant: refused.
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!((st, code(&v)), (403, "grant_required"), "{v}");
    gr::grant(&d.jobs, &[], "owner").unwrap();
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!(st, 200, "{v}");
    // Within the grant only (the App's own room), and still behind the confirm.
    let (st, v) = d.raw("guests.setRooms", json!({ "app": d.jobs, "username": "dana", "rooms": ["board-keeper"] }), None);
    assert_eq!((st, code(&v)), (403, "outside_app_rooms"), "{v}");
    let args = json!({ "app": d.jobs, "username": "dana", "rooms": ["board-builder"] });
    let (st, v) = d.raw("guests.setRooms", args.clone(), None);
    assert_eq!((st, code(&v)), (403, "confirm_required"), "{v}");
    let token = d.person_confirms(v["error"]["secretId"].as_str().unwrap());
    let (st, v) = d.raw("guests.setRooms", args, Some(&token));
    assert_eq!(st, 200, "{v}");
    let change = gr::audit_rows(&d.jobs, 5).unwrap().into_iter().find(|r| r.event == gr::AUDIT_CHANGE).unwrap();
    assert_eq!(change.actor, "garden:guestdesk (agent:board-builder)");
}

/// Round-2 review B4: only an owner-made widget skips the App's 60/min
/// guest-rooms budget; an agent-made one pays it (whatever ceiling Garden
/// access gives it), so it can't starve the App or flood confirm sheets.
#[test]
fn an_agent_made_widget_pays_the_apps_budget() {
    let d = desk(&["guests:rooms"], "gr-budget");
    gr::grant(&d.jobs, &[], "owner").unwrap();
    // The App spends its whole minute.
    for i in 0..gr::RATE_PER_MIN {
        gr::rate_take(&d.jobs).unwrap_or_else(|e| panic!("App call {i}: {e:?}"));
    }
    d.stamp(MadeBy::Agent { project_id: "w-builder".into() });
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!((st, code(&v)), (429, "rate_limited"), "an agent-made widget pays the App's budget: {v}");
    let (st, v) = d.raw("guests.setRooms", json!({ "app": d.jobs, "username": "dana", "rooms": ["board-builder"] }), None);
    assert_eq!((st, code(&v)), (429, "rate_limited"), "no confirm sheet past the budget: {v}");
}

/// Round-2 review B4, the other half (its own desk: stamps are
/// first-write-wins per version): the owner's own widget still works when
/// the App has spent its minute.
#[test]
fn an_owner_made_widget_doesnt_spend_the_apps_budget() {
    let d = desk(&["guests:rooms"], "gr-budget-owner");
    d.stamp(MadeBy::Owner);
    for i in 0..gr::RATE_PER_MIN {
        gr::rate_take(&d.jobs).unwrap_or_else(|e| panic!("App call {i}: {e:?}"));
    }
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!(st, 200, "the owner's own widget doesn't spend the App's budget: {v}");
    let (st, v) = d.raw("guests.setRooms", json!({ "app": d.jobs, "username": "dana", "rooms": ["board-builder"] }), None);
    assert_eq!((st, code(&v)), (403, "confirm_required"), "past the App's budget, the owner's widget still reaches the confirm: {v}");
}

#[test]
fn an_unsigned_widget_and_a_paused_placement_are_refused() {
    let d = desk(&["guests:rooms"], "gr-paused");
    let (st, v) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!((st, code(&v)), (403, "author_unknown"), "{v}");
    d.stamp(MadeBy::Owner);
    crate::zen_routes::pause_for_writes(&d.garden, &d.placement);
    let (st, v) = d.raw("guests.setRooms", json!({ "app": d.jobs, "username": "dana", "rooms": [] }), None);
    assert_eq!((st, code(&v)), (423, "paused"), "{v}");
    let (st, _) = d.raw("guests.rooms", json!({ "app": d.jobs, "username": "dana" }), None);
    assert_eq!(st, 200, "reads still work while paused");
}

/// prd-app-caps-0464-v1 AC3: `app.rooms` / `guests.list` through the widget
/// door. Reads (no confirm), the manifest's guests:rooms, owner rights for
/// an owner-made widget, creator rights for an agent-made one, and an
/// unsigned version is author_unknown.
#[test]
fn app_rooms_and_guests_list_in_a_garden() {
    let d = desk(&["guests:rooms"], "gr-reads");
    // Unsigned: author_unknown for both.
    for verb in ["app.rooms", "guests.list"] {
        let (st, v) = d.raw(verb, json!({ "app": d.jobs }), None);
        assert_eq!((st, code(&v)), (403, "author_unknown"), "{verb}: {v}");
    }
    d.stamp(MadeBy::Owner);
    // Owner rights: any local App, no grant needed, no confirm.
    let (st, v) = d.raw("app.rooms", json!({ "app": d.other }), None);
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["result"]["granted"].clone(), v["result"]["ceiling"].clone()), (json!(false), json!([])));
    assert_eq!(v["result"]["ownRoom"]["room"], json!("w-keeper"));
    let (st, v) = d.raw("guests.list", json!({ "app": d.jobs }), None);
    assert_eq!(st, 200, "{v}");
    let names: Vec<Value> = v["result"]["guests"].as_array().unwrap().iter().map(|g| g["username"].clone()).collect();
    assert_eq!(names, vec![json!("dana")], "only this App's guests: {v}");
    let (st, v) = d.raw("guests.list", json!({}), None);
    assert_eq!((st, code(&v)), (400, "usage"), "a Garden names the App: {v}");
    let (st, v) = d.raw("guests.list", json!({ "app": d.jobs, "limit": 0 }), None);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
}

#[test]
fn app_rooms_and_guests_list_need_the_cap_and_an_agent_widget_its_own_app() {
    let d = desk(&["store:read"], "gr-reads-cap");
    d.stamp(MadeBy::Owner);
    for verb in ["app.rooms", "guests.list"] {
        let (st, v) = d.raw(verb, json!({ "app": d.jobs }), None);
        assert_eq!((st, code(&v)), (403, "cap_not_granted"), "{verb}: {v}");
    }
    drop(d);
    let d = desk(&["guests:rooms"], "gr-reads-agent");
    d.stamp(MadeBy::Agent { project_id: "w-builder".into() });
    // Another workspace's App: not reachable for either.
    for verb in ["app.rooms", "guests.list"] {
        let (st, v) = d.raw(verb, json!({ "app": d.other }), None);
        assert_eq!((st, code(&v)), (404, "not_found"), "{verb}: {v}");
    }
    // Its own App: app.rooms reads without the grant; guests.list needs it.
    let (st, v) = d.raw("app.rooms", json!({ "app": d.jobs }), None);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["result"]["granted"], json!(false));
    let (st, v) = d.raw("guests.list", json!({ "app": d.jobs }), None);
    assert_eq!((st, code(&v)), (403, "grant_required"), "{v}");
    gr::grant(&d.jobs, &[], "owner").unwrap();
    let (st, v) = d.raw("guests.list", json!({ "app": d.jobs }), None);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["result"]["guests"][0]["username"], json!("dana"));
    let (st, v) = d.raw("app.rooms", json!({ "app": d.jobs }), None);
    assert_eq!((st, v["result"]["granted"].clone()), (200, json!(true)), "{v}");
}
