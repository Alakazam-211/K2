//! The forward routes end to end against fakes: the mail engine (confirm
//! mails), a fake Sieve (forward_sync) and the verify pages. No mail
//! server, no real mail.

use std::collections::HashMap;

use k2_core::mail_forward as store;
use serde_json::{json, Value as J};

use super::*;
use crate::email_notify::tests::{fx, system_sender_on, Fx};
use crate::email_notify::verify::{self as vpages, STOP_PATH, VERIFY_PATH};
use crate::mail::forward_sync::tests::{install, seed_mailbox, uninstall, FakeSieve, HOST};

const MB: &str = "sales@notify.brand.test";
const ACC: &str = "acc-sales";
const OUT: &str = "me@outside.test";

struct T {
    f: Fx,
    io: std::rc::Rc<FakeSieve>,
}

impl Drop for T {
    fn drop(&mut self) {
        uninstall();
    }
}

/// The email_notify fixture (hosted domain `notify.brand.test`, fake
/// engine), a front door, a fake Sieve and the mailbox `sales@…`.
fn t(tag: &str) -> T {
    let f = fx(tag);
    k2_core::skin::set_front_door("direct", Some("https://box.brand.test"), None, None).expect("door");
    let io = install(false);
    seed_mailbox(MB, "direct");
    T { f, io }
}

fn status(r: &CliResponse) -> u16 {
    r.status.split_whitespace().next().expect("status").parse().expect("n")
}

fn body(r: &CliResponse) -> J {
    serde_json::from_str(&r.body).expect("json")
}

fn post(path: &str, v: J) -> (u16, J) {
    let b = v.to_string();
    let r = match path {
        FORWARD_PATH => handle_forward_set(b.as_bytes()),
        UNSET_PATH => handle_forward_unset(b.as_bytes()),
        PAUSE_PATH => handle_forward_pause(b.as_bytes()),
        RESUME_PATH => handle_forward_resume(b.as_bytes()),
        PERSON_LINK_PATH => handle_person_link(b.as_bytes()),
        _ => panic!("no route {path}"),
    };
    (status(&r), body(&r))
}

fn as_agent<T>(ws: &str, f: impl FnOnce() -> T) -> T {
    crate::caller_workspace::with_request_principal(
        Some(crate::session_token::HookPrincipal { workspace_uuid: ws.to_string(), agent_address: "agent".into() }),
        f,
    )
}

fn show() -> J {
    let params: HashMap<String, String> = [("address".to_string(), MB.to_string())].into();
    let r = handle_forward_get(&params);
    assert_eq!(status(&r), 200, "{}", r.body);
    body(&r)
}

fn last_token(t: &T, prefix: &str) -> String {
    let sent = t.f.engine.sent.lock().unwrap().clone();
    let text = sent.last().expect("a mail").email.to_string();
    let at = text.find(prefix).unwrap_or_else(|| panic!("no {prefix} in {text}"));
    text[at..].chars().take(prefix.len() + 43).collect()
}

/// The safety core: nothing is written to the mailbox's Sieve until the
/// destination POSTs Confirm; a link scanner's GET confirms nothing.
#[test]
fn an_outside_forward_waits_for_the_confirm_post() {
    let t = t("fwout");
    let sys = system_sender_on();
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "Me@Outside.test" }));
    assert_eq!((st, v["state"].clone(), v["keep"].clone()), (202, json!("pending_verify"), json!(true)), "{v}");
    assert!(v["message"].as_str().unwrap_or("").contains("confirm link"), "{v}");
    assert_eq!(t.io.forward(ACC), None, "nothing forwarded before the confirm");
    let mail = t.f.engine.sent.lock().unwrap().last().cloned().expect("confirm mail");
    assert_eq!((mail.from.as_str(), mail.rcpt.clone()), (sys.address.as_str(), vec![OUT.to_string()]));
    let text = mail.email.to_string();
    assert!(text.contains(&format!("copies of the mail that arrives at {MB}")), "{text}");
    let tok = last_token(&t, "k2ev_");
    // A GET (link scanner) confirms nothing.
    let p = vpages::page_get(VERIFY_PATH, &format!("t={tok}"));
    assert!(p.status.starts_with("200"), "{p:?}");
    assert_eq!(t.io.forward(ACC), None);
    assert_eq!(show()["forward"]["state"], "pending_verify");
    // The POST confirms: the guarded block is written.
    let p = vpages::page_post(VERIFY_PATH, "", format!("t={tok}").as_bytes());
    assert!(p.html.contains("Confirmed"), "{p:?}");
    let fwd = t.io.forward(ACC).expect("forward written");
    assert_eq!((fwd.dest.as_str(), fwd.keep, fwd.guard.map(|g| g.host)), (OUT, true, Some(HOST.to_string())));
    let s = show();
    assert_eq!((s["forward"]["state"].clone(), s["to"].clone(), s["forward"]["verify"]["state"].clone()), (json!("active"), json!(OUT), json!("verified")), "{s}");
    let events: Vec<&str> = s["events"].as_array().expect("events").iter().rev().filter_map(|e| e["event"].as_str()).collect();
    assert_eq!(events, vec!["set", "verify_sent", "verified", "activated"], "{s}");
}

#[test]
fn the_stop_link_pauses_for_good_and_only_a_new_set_brings_it_back() {
    let t = t("fwstop");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    let tok = last_token(&t, "k2ev_");
    let stop = last_token(&t, "k2es_");
    vpages::page_post(VERIFY_PATH, "", format!("t={tok}").as_bytes());
    assert!(t.io.forward(ACC).is_some());
    let p = vpages::page_post(STOP_PATH, &format!("t={stop}"), b"List-Unsubscribe=One-Click");
    assert!(p.html.contains("Stopped"), "{p:?}");
    assert_eq!(t.io.forward(ACC), None, "stopped = keep-only");
    let s = show();
    assert_eq!((s["forward"]["state"].clone(), s["forward"]["pauseReason"].clone()), (json!("paused"), json!("revoked")), "{s}");
    let (st, v) = post(RESUME_PATH, json!({ "address": MB }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("destination_stopped")), "even the owner: {v}");
    // A new set sends a new confirm.
    let n = t.f.engine.sent.lock().unwrap().len();
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["state"].clone()), (202, json!("pending_verify")), "{v}");
    assert_eq!(t.f.engine.sent.lock().unwrap().len(), n + 1);
}

/// FW-A16: no system sender → the confirm comes from the mailbox itself,
/// by delegation, with no stored copy.
#[test]
fn with_no_system_sender_the_confirm_comes_from_the_mailbox() {
    let t = t("fwself");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT, "keep": false }));
    assert_eq!((st, v["sentVia"].clone(), v["keep"].clone()), (202, json!("mailbox"), json!(false)), "{v}");
    let mail = t.f.engine.sent.lock().unwrap().last().cloned().expect("confirm mail");
    assert_eq!((mail.account.as_str(), mail.from.as_str(), mail.destroy), (ACC, MB, true));
    assert_eq!(mail.rcpt, vec![OUT.to_string()]);
}

#[test]
fn destinations_k2_refuses() {
    let _t = t("fwrefuse");
    system_sender_on();
    let sender = "system@notify.brand.test";
    for (to, code) in [
        (MB, "usage"),                              // itself
        ("ghost@notify.brand.test", "usage"),       // this box's domain, not a mailbox
        ("not an address", "usage"),
        (sender, "usage"),                          // a notification sender is on a hosted domain too
    ] {
        let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": to }));
        assert_eq!(v["error"]["code"], code, "{to}: {v}");
        assert!(st == 400 || st == 409, "{to}: {st} {v}");
    }
    let (_, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "ghost@notify.brand.test" }));
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("not a minted mailbox on this host"), "{v}");
    // Air-gap on: no outside forward.
    crate::push_routes::set_test_airgap(true);
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    crate::push_routes::set_test_airgap(false);
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("air_gap")), "{v}");
    // A receive-only domain: no outside forward.
    seed_mailbox("ro@quiet.test", "receive-only");
    let (st, v) = post(FORWARD_PATH, json!({ "address": "ro@quiet.test", "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("receive_only")), "{v}");
}

#[test]
fn a_same_box_forward_is_active_at_once_without_a_guard() {
    let t = t("fwin");
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test", "keep": true }));
    assert_eq!((st, v["state"].clone(), v["kind"].clone()), (200, json!("active"), json!("internal")), "{v}");
    let fwd = t.io.forward(ACC).expect("written");
    assert_eq!((fwd.dest.as_str(), fwd.guard.is_none()), ("team@notify.brand.test", true));
    assert!(t.f.engine.sent.lock().unwrap().is_empty(), "no confirm mail for a same-box forward");
    let (st, v) = post(UNSET_PATH, json!({ "address": MB }));
    assert_eq!((st, v["removed"].clone()), (200, json!(true)), "{v}");
    assert_eq!(t.io.forward(ACC), None);
}

/// Rosson 2026-10-30: a mail-manage agent sets an outside forward end to
/// end (no owner card); the confirm link still applies. It can't resume
/// an automatic pause.
#[test]
fn a_mail_manage_agent_manages_outside_forwards_but_not_auto_pauses() {
    let t = t("fwagent");
    system_sender_on();
    // The mailbox is minted by `ws-fw` (seed_mailbox), so the agent of
    // that workspace may manage it (authorize_mailbox: minting workspace).
    let (st, v) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": OUT })));
    assert_eq!((st, v["state"].clone()), (202, json!("pending_verify")), "{v}");
    let text = t.f.engine.sent.lock().unwrap().last().expect("mail").email.to_string();
    assert!(text.contains("The AI agent"), "the confirm says an agent asked: {text}");
    assert_eq!(t.io.forward(ACC), None, "still waits for the confirm");
    let tok = last_token(&t, "k2ev_");
    vpages::page_post(VERIFY_PATH, "", format!("t={tok}").as_bytes());
    assert!(t.io.forward(ACC).is_some());
    let id = show()["forward"]["id"].as_str().expect("id").to_string();
    // K2 pauses it (the destination refused) → the agent can't resume.
    crate::mail::forward_sync::pause(&id, store::P_DEST_GONE, "k2", None).expect("pause");
    let (st, v) = as_agent("ws-fw", || post(RESUME_PATH, json!({ "address": MB })));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    let (st, v) = post(RESUME_PATH, json!({ "address": MB }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "the owner can: {v}");
    // The agent's own (manual) pause it can undo; and it can remove.
    let (st, _) = as_agent("ws-fw", || post(PAUSE_PATH, json!({ "address": MB })));
    assert_eq!(st, 200);
    let (st, v) = as_agent("ws-fw", || post(RESUME_PATH, json!({ "address": MB })));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    let (st, _) = as_agent("ws-fw", || post(UNSET_PATH, json!({ "address": MB })));
    assert_eq!(st, 200);
    assert_eq!(t.io.forward(ACC), None);
    // Owner-only lists refuse an agent.
    let r = as_agent("ws-fw", || handle_forwards_list(&HashMap::new()));
    assert_eq!(status(&r), 403, "{}", r.body);
    let r = as_agent("ws-fw", || handle_person_link(json!({ "address": MB, "person": "owner" }).to_string().as_bytes()));
    assert_eq!(status(&r), 403, "{}", r.body);
}

#[test]
fn a_person_acts_only_on_a_mailbox_linked_to_them() {
    let t = t("fwperson");
    system_sender_on();
    let call = |person: &str, is_post: bool, path: &str, v: J| {
        let r = self_route(is_post, path, v.to_string().as_bytes(), person, &HashMap::new());
        (status(&r), body(&r))
    };
    // Not linked yet.
    let (st, v) = call("owner", true, SELF_PATH, json!({ "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (404, json!("not_found")), "{v}");
    // GET on a POST-only person route.
    let (st, _) = call("owner", false, SELF_UNSET_PATH, json!({}));
    assert_eq!(st, 405);
    let (st, v) = post(PERSON_LINK_PATH, json!({ "address": MB, "person": "owner" }));
    assert_eq!((st, v["person"].clone()), (200, json!("owner")), "{v}");
    let (st, v) = call("owner", true, SELF_PATH, json!({ "to": OUT }));
    assert_eq!((st, v["state"].clone()), (202, json!("pending_verify")), "{v}");
    let (st, v) = call("owner", false, SELF_PATH, json!({}));
    assert_eq!((st, v["forward"]["to"].clone()), (200, json!(OUT)), "{v}");
    // Someone else's mailbox: refused.
    seed_mailbox("other@notify.brand.test", "direct");
    let (st, v) = call("owner", true, SELF_PATH, json!({ "address": "other@notify.brand.test", "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("forbidden")), "{v}");
    let (st, _) = call("owner", true, SELF_UNSET_PATH, json!({}));
    assert_eq!(st, 200);
    assert_eq!(t.io.forward(ACC), None);
}

#[test]
fn the_list_and_a_get_on_a_mutation() {
    let _t = t("fwlist");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    let r = handle_forwards_list(&HashMap::new());
    let v = body(&r);
    assert_eq!(v["forwards"].as_array().map(Vec::len), Some(1), "{v}");
    let params: HashMap<String, String> = [("state".to_string(), "paused".to_string())].into();
    assert_eq!(body(&handle_forwards_list(&params))["forwards"].as_array().map(Vec::len), Some(0));
    // GET on every mutation is 405 (the mail_routes guard).
    for p in [UNSET_PATH, PAUSE_PATH, RESUME_PATH, PERSON_LINK_PATH] {
        let r = crate::mail_routes::dispatch(p, &HashMap::new()).expect("claimed");
        assert_eq!(r.status, "405 Method Not Allowed", "{p}: {}", r.body);
    }
}

#[test]
fn set_without_to_or_address_is_usage() {
    let _t = t("fwusage");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB }));
    assert_eq!((st, v["error"]["code"].clone()), (400, json!("usage")), "{v}");
    let (st, _) = post(FORWARD_PATH, json!({ "to": OUT }));
    assert_eq!(st, 400);
    let (st, _) = post(FORWARD_PATH, json!({ "address": "nobody@no-such-fwd.example.test", "to": OUT }));
    assert_eq!(st, 404);
}

/// Outside forwards are proven on Stalwart 0.16.25: an older (or unknown)
/// mail server refuses them with the upgrade hint; same-box forwards still
/// work there.
#[test]
fn an_older_mail_server_refuses_outside_forwards_but_not_same_box_ones() {
    let t = t("fwver");
    system_sender_on();
    for v in [Some("0.16.10".to_string()), Some("v0.16.20".to_string()), None] {
        crate::mail::forward_sync::set_test_version(Some(v.clone()));
        let n = t.f.engine.sent.lock().unwrap().len();
        let (st, body) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
        assert_eq!((st, body["error"]["code"].clone()), (409, json!("upgrade_required")), "{v:?}: {body}");
        assert!(body["error"]["hint"].as_str().unwrap_or("").contains("k2 hostmail upgrade"), "{body}");
        assert_eq!(t.f.engine.sent.lock().unwrap().len(), n, "no confirm mail on {v:?}");
    }
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "same-box still works: {v}");
    assert!(t.io.forward(ACC).is_some());
    crate::mail::forward_sync::set_test_version(Some(Some("0.16.25".to_string())));
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!(st, 202, "the pinned version: {v}");
}
