//! Sew chat is for people (prd-home-people-channels-v1 §13; HP-31, HP-32,
//! HP-34, HP-36): `k2 chat` refuses an agent every DM with a box person
//! (one that already exists included) and every K5 channel, `follow` and
//! `wait` refuse K5 channels, and the hub drops K5 deliveries and DMs from
//! box people. A child of `tests`: same env and helpers.

use super::*;
use k2_core::sew_chat::hub::{self as core_hub, Delivery};

const ROSSON_EXT: &str = "k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e";

/// The DM `c-dm-ar` (alpha and rosson) exists from before this build;
/// rosson is a Connect login of this server.
fn rosson_is_a_box_person(e: &Env) {
    k2_core::tool_hook::map_member_for_tests(ROSSON_EXT, &e.rosson, "person", "rosson", k2_core::tool_hook::members::ACTIVE)
        .expect("map rosson");
}

fn k5(e: &Env) {
    e.fake.add_space("sp-k5", "k5-admin", false);
    e.fake.add_channel("sp-k5", "c-k5-team", "team", None, true);
    grant(WA, "sp-k5", "k5-admin", Level::Send, ApprovalMode::None);
}

/// HP-31: opening (find-or-create) a DM with a box person is refused before
/// Sew is asked to open anything.
#[test]
fn dm_to_a_box_person_is_refused_before_sew_opens_it() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    rosson_is_a_box_person(&e);
    let (s, v) = post("/cli/sew/chat/dm", &agent(WA), json!({"to": "rosson", "text": "hi"}));
    assert_eq!((s, code(&v)), (403, "dm_not_for_agents"), "{v}");
    assert!(!e.fake.calls().iter().any(|c| c.op == "open_dm"), "Sew never opened the DM");
    assert!(e.fake.writes().is_empty(), "nothing posted");
}

/// The DM that already exists: reading a message in it, following a thread
/// in it, and listing it are all refused or left out.
#[test]
fn a_dm_with_a_box_person_from_before_is_off_limits() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    let id = e.fake.post(&e.rosson, "c-dm-ar", "a dm from before", None, &[]);
    // Before K2 knows rosson is a box person, the DM is listed (today's rule).
    let (s, v) = get("/cli/sew/chat/unread", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    assert!(v["channels"].as_array().unwrap().iter().any(|c| c["kind"] == "dm"), "{v}");
    rosson_is_a_box_person(&e);
    let (s, v) = get("/cli/sew/chat/read", &agent(WA), &[("id", &format!("msg_{id}"))]);
    assert_eq!((s, code(&v)), (403, "dm_not_for_agents"), "read: {v}");
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": format!("msg_{id}")}));
    assert_eq!((s, code(&v)), (403, "dm_not_for_agents"), "follow a thread in it: {v}");
    let (s, v) = post("/cli/sew/chat/reply", &agent(WA), json!({"to": format!("msg_{id}"), "text": "re"}));
    assert_eq!((s, code(&v)), (403, "dm_not_for_agents"), "reply in it: {v}");
    let (s, v) = post("/cli/sew/chat/dm", &agent(WA), json!({"to": "@rosson", "text": "again"}));
    assert_eq!((s, code(&v)), (403, "dm_not_for_agents"), "send to it: {v}");
    let (s, v) = get("/cli/sew/chat/unread", &agent(WA), &[("all", "1")]);
    assert_eq!(s, 200, "{v}");
    assert!(!v["channels"].as_array().unwrap().iter().any(|c| c["kind"] == "dm"), "the DM is not listed: {v}");
    let (s, v) = post("/cli/sew/chat/mark-read", &agent(WA), json!({"all": true}));
    assert_eq!(s, 200, "{v}");
    assert!(!v["marked"].as_array().unwrap().iter().any(|a| a.as_str().unwrap().starts_with("dm/")), "{v}");
    assert!(e.fake.writes().is_empty(), "nothing posted");
}

/// HP-32 until S8: no K5 channel answers an agent, even with a K5 access
/// row; lists leave them out.
#[test]
fn k5_channels_are_not_granted() {
    let e = env();
    k5(&e);
    let (s, v) = get("/cli/sew/chat/channels", &agent(WA), &[("space", "k5-admin")]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["channels"], json!([]), "{v}");
    let (s, v) = get("/cli/sew/chat/messages", &agent(WA), &[("in", "k5-admin/team")]);
    assert_eq!((s, code(&v)), (404, "not_granted"), "{v}");
    let (s, v) = send(&agent(WA), "k5-admin/team", "hello people");
    assert_eq!((s, code(&v)), (404, "not_granted"), "{v}");
    let id = e.fake.post(&e.rosson, "c-k5-team", "people talk", None, &[]);
    let (s, v) = get("/cli/sew/chat/read", &agent(WA), &[("id", &format!("msg_{id}"))]);
    assert_eq!((s, code(&v)), (404, "not_granted"), "{v}");
    let (s, v) = get("/cli/sew/chat/unread", &agent(WA), &[("all", "1")]);
    assert_eq!(s, 200, "{v}");
    assert!(!v["channels"].as_array().unwrap().iter().any(|c| c["addr"].as_str().unwrap().starts_with("k5-admin/")), "{v}");
    assert!(e.fake.writes().is_empty());
}

/// D17, HP-36: `follow` and `wait --in` refuse K5 channels.
#[test]
fn follow_and_wait_refuse_k5_channels() {
    let e = env();
    k5(&e);
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": "k5-admin/team"}));
    assert_eq!((s, code(&v)), (403, "no_channel_notifications"), "{v}");
    let (s, v) = get("/cli/sew/chat/wait", &agent(WA), &[("in", "k5-admin/team"), ("timeout", "1")]);
    assert_eq!((s, code(&v)), (403, "no_channel_notifications"), "{v}");
    let id = e.fake.post(&e.rosson, "c-k5-team", "a thread", None, &[]);
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": format!("msg_{id}")}));
    assert_eq!((s, code(&v)), (403, "no_channel_notifications"), "a thread in K5: {v}");
    assert!(!e.fake.follows_channel(&e.alpha, "c-k5-team"));
}

fn delivery(place: &str, space: Option<&str>, addr: &str, author: &str, kind: &str) -> Delivery {
    let mut m = json!({"id": "91", "channel_id": "c-x", "author_id": author, "author_display_name": "R",
        "author_username": "rosson", "author_kind": kind, "addr": addr, "place": place, "body": "b",
        "created_at": "2026-11-14T10:00:00.000Z"});
    if let Some(s) = space {
        m["space_id"] = json!(s);
    }
    core_hub::decode_delivery(&json!({"seq": 1, "reason": "dm", "message": m})).unwrap()
}

/// HP-34, HP-31: the hub drops every K5 delivery and every DM from a box
/// person; others pass.
#[test]
fn the_hub_drops_k5_deliveries_and_dms_from_box_people() {
    let e = env();
    let k5_msg = delivery("space_channel", Some("sp-k5"), "k5-admin/#team", &e.rosson, "person");
    assert!(!super::super::home_gate::hub_keeps(&k5_msg), "K5 channel");
    let hearth_msg = delivery("space_channel", Some("sp-hearth"), "hearth/#general", &e.rosson, "person");
    assert!(super::super::home_gate::hub_keeps(&hearth_msg), "another Space");
    let dm = delivery("dm", None, "dm/@rosson", &e.rosson, "person");
    assert!(super::super::home_gate::hub_keeps(&dm), "not known as a box person yet");
    rosson_is_a_box_person(&e);
    assert!(!super::super::home_gate::hub_keeps(&dm), "a DM from a box person");
    let from_agent = delivery("dm", None, "dm/@beta", "m-beta", "agent");
    assert!(super::super::home_gate::hub_keeps(&from_agent), "an agent's DM");
}
