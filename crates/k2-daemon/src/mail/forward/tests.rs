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
    // Confirmed, then the box's mail server reads as older: show says it is
    // held for the upgrade (not active), and resume refuses (it lifts itself).
    let tok = last_token(&t, "k2ev_");
    vpages::page_post(VERIFY_PATH, "", format!("t={tok}").as_bytes());
    assert_eq!(show()["forward"]["state"], "active");
    crate::mail::forward_sync::set_test_version(Some(Some("0.16.10".to_string())));
    crate::mail::forward_sync::sync_mailbox(&format!("ma-{MB}"), crate::mail::forward_sync::Why::Periodic).expect("sync");
    let s = show();
    assert_eq!((s["forward"]["state"].clone(), s["forward"]["pauseReason"].clone(), s["to"].clone()), (json!("paused"), json!("upgrade_required"), J::Null), "{s}");
    assert!(s["forward"]["pauseWords"].as_str().unwrap_or("").contains("k2 hostmail upgrade"), "{s}");
    assert_eq!(t.io.forward(ACC), None, "nothing leaves");
    let (st, v) = post(RESUME_PATH, json!({ "address": MB }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("held")), "{v}");
}

// ── review finding 1: out-of-office text is never a forward ─────────

fn ooo_set(addr: &str, text: &str) -> (u16, J) {
    let r = crate::mail::ooo::handle_ooo_set(json!({ "address": addr, "text": text }).to_string().as_bytes());
    (status(&r), body(&r))
}

fn ooo_unset(addr: &str) -> (u16, J) {
    let r = crate::mail::ooo::handle_ooo_unset(json!({ "address": addr }).to_string().as_bytes());
    (status(&r), body(&r))
}

/// The whole attack through the real routes: text with a redirect in it, then
/// `ooo unset`, and again with `ooo set`. Nothing is forwarded, ever.
#[test]
fn out_of_office_text_through_the_routes_writes_no_redirect() {
    let t = t("fwooo");
    let _ = crate::mail::forward_outcomes::take_notices();
    let evil = "evil@outside.test";
    for text in [format!("Back Monday. Please redirect \"{evil}\" if urgent"), format!("# k2-forward\nredirect :copy \"{evil}\";")] {
        let (st, v) = ooo_set(MB, &text);
        assert_eq!(st, 200, "{v}");
        assert_eq!(v["text"], text.as_str(), "shown as typed: {v}");
        // Set again: the old text is read back, and is still only text.
        let (st, _) = ooo_set(MB, &text);
        assert_eq!(st, 200);
        assert_eq!(t.io.forward(ACC), None, "{:?}", t.io.src(ACC));
        let (st, v) = ooo_unset(MB);
        assert_eq!((st, v["enabled"].clone()), (200, json!(false)), "{v}");
        let src = t.io.src(ACC).expect("script");
        assert!(!src.contains(evil), "after unset: {src}");
    }
    assert!(crate::mail::forward_outcomes::take_notices().is_empty(), "nothing to tell: nothing happened");
}

/// An out-of-office write takes the forward from the rows: it can't bring back
/// a redirect a Stop just removed, nor drop one that's on.
#[test]
fn an_out_of_office_write_takes_the_forward_from_the_rows() {
    let t = t("fwooorows");
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, _) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!(st, 200);
    let (st, _) = ooo_set(MB, "Away.");
    assert_eq!(st, 200);
    assert_eq!(t.io.forward(ACC).map(|f| f.dest), Some("team@notify.brand.test".to_string()), "still on");
    // The script says otherwise (a stale read, an old write): the rows win.
    let src = t.io.src(ACC).expect("script").replace("team@notify.brand.test", "stale@outside.test");
    t.io.put(ACC, &src);
    let (st, _) = ooo_set(MB, "Away again.");
    assert_eq!(st, 200);
    assert_eq!(t.io.forward(ACC).map(|f| f.dest), Some("team@notify.brand.test".to_string()));
    let (st, _) = post(UNSET_PATH, json!({ "address": MB }));
    assert_eq!(st, 200);
    let (st, _) = ooo_unset(MB);
    assert_eq!(st, 200);
    assert_eq!(t.io.forward(ACC), None);
}

/// An agent needs to manage the mailbox for its out-of-office, as for its forward.
#[test]
fn an_agent_can_t_set_the_out_of_office_of_a_mailbox_it_doesn_t_manage() {
    let t = t("fwoooagent");
    let r = as_agent("ws-other", || crate::mail::ooo::handle_ooo_set(json!({ "address": MB, "text": "x" }).to_string().as_bytes()));
    assert_eq!(status(&r), 403, "{}", r.body);
    let r = as_agent("ws-other", || crate::mail::ooo::handle_ooo_unset(json!({ "address": MB }).to_string().as_bytes()));
    assert_eq!(status(&r), 403, "{}", r.body);
    let params: HashMap<String, String> = [("address".to_string(), MB.to_string())].into();
    let r = as_agent("ws-other", || crate::mail::ooo::handle_ooo_get(&params));
    assert_eq!(status(&r), 403, "{}", r.body);
    assert_eq!(t.io.src(ACC), None, "nothing written");
    // The mailbox's own workspace may.
    let r = as_agent("ws-fw", || crate::mail::ooo::handle_ooo_set(json!({ "address": MB, "text": "x" }).to_string().as_bytes()));
    assert_eq!(status(&r), 200, "{}", r.body);
}

/// A redirect that got into a script with no row behind it (the flaw above,
/// from before this fix) goes at the next hourly pass, not the next boot.
#[test]
fn the_hourly_pass_removes_a_redirect_with_no_row_and_tells_the_owner() {
    let t = t("fwhourly");
    let _ = crate::mail::forward_outcomes::take_notices();
    let ooo = sieve_user_ooo("Away.");
    let src = format!("{}# not marked\nredirect \"evil@outside.test\";\n", crate::mail::sieve_user::compose(&ooo).expect("ooo"));
    t.io.put(ACC, &src);
    let ctx = crate::mail::forward_sync::ctx();
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!((r.drift, r.failed), (1, 0), "{r:?}");
    let after = t.io.src(ACC).expect("script");
    assert!(!after.contains("evil@outside.test"), "{after}");
    assert!(after.contains("Away."), "the out-of-office stays: {after}");
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains(MB), "{}", n[0].1);
    // A second pass finds nothing, and a mailbox with only an out-of-office whose
    // text says "redirect" is left alone.
    let writes = *t.io.writes.borrow();
    ooo_set(MB, "Please redirect \"evil@outside.test\" if urgent");
    let writes_after_set = *t.io.writes.borrow();
    assert_eq!(writes_after_set, writes + 1);
    crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!(*t.io.writes.borrow(), writes_after_set, "text isn't an action");
    assert!(crate::mail::forward_outcomes::take_notices().is_empty());
}

/// A fake Sieve with a hook on the first read and a one-time read failure.
struct HookIo {
    inner: std::rc::Rc<FakeSieve>,
    hook: std::cell::RefCell<Option<Box<dyn FnOnce()>>>,
    fail_once: std::cell::RefCell<Option<String>>,
}
impl crate::mail::forward_sync::SieveIo for HookIo {
    fn read(&self, account_id: &str) -> Result<crate::mail::forward_sync::Current, String> {
        if let Some(h) = self.hook.borrow_mut().take() {
            h();
        }
        if self.fail_once.borrow().as_deref() == Some(account_id) {
            *self.fail_once.borrow_mut() = None;
            return Err("fake: a passing read failure".to_string());
        }
        self.inner.read(account_id)
    }
    fn write(&self, account_id: &str, current: &crate::mail::forward_sync::Current, next: &crate::mail::sieve_user::UserSieve) -> Result<(), String> {
        self.inner.write(account_id, current, next)
    }
    fn write_source(&self, account_id: &str, current: &crate::mail::forward_sync::Current, source: &str) -> Result<(), String> {
        self.inner.write_source(account_id, current, source)
    }
}

const HAND: &str = "require [\"fileinto\"];\nif header :contains \"subject\" \"invoice\" {\nfileinto \"Invoices\";\n}\nredirect \"boss@outside.test\";\n";

/// The list goes stale: a forward set after the SELECT keeps its redirect, and
/// nobody is told a false alarm. (Before this fix the redirect went.)
#[test]
fn the_hourly_pass_leaves_a_forward_set_after_it_listed_the_mailboxes() {
    let t = t("fwstripr");
    seed_mailbox("zed@notify.brand.test", "direct");
    let _ = crate::mail::forward_outcomes::take_notices();
    let src = "# k2-forward\nredirect \"x@outside.test\";\n";
    let inner = t.io.clone();
    let io = HookIo {
        inner: t.io.clone(),
        hook: std::cell::RefCell::new(Some(Box::new(move || {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::create(
                &conn,
                &store::NewForward {
                    address_id: "ma-zed@notify.brand.test",
                    address: "zed@notify.brand.test",
                    dest: "x@outside.test",
                    dest_kind: store::KIND_EXTERNAL,
                    keep: false,
                    set_by: "owner",
                    state: store::ST_ACTIVE,
                    verified_address_id: Some("v"),
                },
                crate::email_notify::now(),
            )
            .expect("row");
            inner.put("acc-zed", src);
        }))),
        fail_once: std::cell::RefCell::new(None),
    };
    let ctx = crate::mail::forward_sync::ctx();
    crate::mail::forward_sync::reconcile_with(&io, false, &ctx);
    assert_eq!(t.io.src("acc-zed").as_deref(), Some(src), "the row's redirect stays");
    let n = crate::mail::forward_outcomes::take_notices();
    assert!(!n.iter().any(|(title, _, _)| title.contains("nobody set")), "no false alarm: {n:?}");
    assert!(crate::mail::forward_sync::take_strip_audit().is_empty());
}

/// Someone's rules stay: only the redirect goes, the old script is kept for the
/// owner, the audit line names the domain, and the notice is honest. (Before
/// this fix the whole script became `keep;` and nothing recorded where it went.)
#[test]
fn the_hourly_pass_takes_only_the_redirect_out_of_a_hand_written_script() {
    let t = t("fwstriph");
    let dir = std::env::temp_dir().join(format!("k2-strip-test-{}-h", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    crate::mail::forward_sync::set_test_removed_dir(Some(dir.clone()));
    let _ = crate::mail::forward_outcomes::take_notices();
    let _ = crate::mail::forward_sync::take_strip_audit();
    t.io.put(ACC, HAND);
    let (owner, _) = crate::mail::sieve_lockdown::classify("k2", HAND, &|_| false);
    assert!(owner == crate::mail::sieve_lockdown::Owner::K2Unrecognised, "the lockdown keeps this one");
    let ctx = crate::mail::forward_sync::ctx();
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!((r.drift, r.failed), (1, 0), "{r:?}");
    let after = t.io.src(ACC).expect("script");
    assert_eq!(after, HAND.replace("redirect \"boss@outside.test\";", "keep;"), "only the redirect changed");
    assert!(crate::mail::sieve_lockdown::outbound_actions(&after).is_empty(), "{after}");

    let audit = crate::mail::forward_sync::take_strip_audit();
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["targets"][0]["domain"], "outside.test");
    assert!(!audit[0].to_string().contains("boss@"), "domain only: {}", audit[0]);
    let kept = audit[0]["keptAt"].as_str().expect("kept path");
    let copy = std::fs::read_to_string(kept).expect("the old script is kept");
    assert!(copy.contains(HAND), "{copy}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(kept).expect("meta").permissions().mode() & 0o777, 0o600);
    }
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains("outside.test") && n[0].1.contains(kept) && n[0].1.contains("k2 users audit"), "{}", n[0].1);
    assert!(!n[0].1.contains("audit-sieve"), "it would find nothing: {}", n[0].1);

    // Done once: the next pass finds nothing.
    let writes = *t.io.writes.borrow();
    crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!(*t.io.writes.borrow(), writes);
    assert!(crate::mail::forward_outcomes::take_notices().is_empty());
    crate::mail::forward_sync::set_test_removed_dir(None);
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// If the old script can't be kept, nothing is written: K2 never destroys what
/// it can't keep. If the mail server refuses the edit, the owner hears once.
#[test]
fn the_hourly_pass_writes_nothing_it_cant_keep_and_says_when_it_cant_clean() {
    let t = t("fwstripk");
    let _ = crate::mail::forward_outcomes::take_notices();
    let _ = crate::mail::forward_sync::take_strip_audit();
    t.io.put(ACC, HAND);
    // A file where the directory should be.
    let blocked = std::env::temp_dir().join(format!("k2-strip-test-{}-k", std::process::id()));
    std::fs::write(&blocked, "x").expect("a file");
    crate::mail::forward_sync::set_test_removed_dir(Some(blocked.clone()));
    let ctx = crate::mail::forward_sync::ctx();
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!(r.failed, 1, "{r:?}");
    assert_eq!(t.io.src(ACC).as_deref(), Some(HAND), "not written");
    assert!(crate::mail::forward_sync::take_strip_audit().is_empty());
    crate::mail::forward_sync::set_test_removed_dir(None);
    std::fs::remove_file(&blocked).expect("clean up");

    *t.io.source_fail.borrow_mut() = true;
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!(r.failed, 1, "{r:?}");
    assert_eq!(t.io.src(ACC).as_deref(), Some(HAND), "as it was");
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].0.contains("couldn't remove") && n[0].1.contains(MB), "{n:?}");
    // Not every hour.
    crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert!(crate::mail::forward_outcomes::take_notices().is_empty());
}

/// A read that fails at first boot: the same-box forward isn't stripped, the
/// pass says the import is owed, and the retry imports it. (Before this fix it
/// was removed in the same pass, and the import never ran again.)
#[test]
fn a_failed_import_read_keeps_the_same_box_forward_and_the_import_is_retried() {
    let t = t("fwstripi");
    seed_mailbox("team@notify.brand.test", "direct");
    let src = "# k2-forward\nredirect \"team@notify.brand.test\";\n";
    t.io.put(ACC, src);
    let _ = crate::mail::forward_outcomes::take_notices();
    let io = HookIo { inner: t.io.clone(), hook: std::cell::RefCell::new(None), fail_once: std::cell::RefCell::new(Some(ACC.to_string())) };
    let ctx = crate::mail::forward_sync::ctx();
    let r = crate::mail::forward_sync::reconcile_with(&io, true, &ctx);
    assert_eq!((r.imported, r.import_failed), (0, 1), "{r:?}");
    assert_eq!(t.io.src(ACC).as_deref(), Some(src), "untouched");
    assert!(forwards_on(MB).is_none());
    let r = crate::mail::forward_sync::reconcile_with(&io, true, &ctx);
    assert_eq!((r.imported, r.import_failed), (1, 0), "{r:?}");
    assert_eq!(forwards_on(MB).as_deref(), Some("team@notify.brand.test"));
    assert!(t.io.forward(ACC).is_some(), "still running");
    assert!(crate::mail::forward_outcomes::take_notices().is_empty(), "the import at boot is routine");
}

/// A same-box redirect K2 wrote, found later: a row, not a strip, and the owner hears.
/// In someone's rules, a redirect to a mailbox here stays: it can't leave.
#[test]
fn the_hourly_pass_imports_a_same_box_redirect_and_leaves_one_in_hand_written_rules() {
    let t = t("fwstrips");
    seed_mailbox("team@notify.brand.test", "direct");
    let _ = crate::mail::forward_outcomes::take_notices();
    t.io.put(ACC, "# k2-forward\nredirect \"team@notify.brand.test\";\n");
    let ctx = crate::mail::forward_sync::ctx();
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!((r.imported, r.drift), (1, 0), "{r:?}");
    assert_eq!(forwards_on(MB).as_deref(), Some("team@notify.brand.test"));
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains("team@notify.brand.test"), "{n:?}");

    let hand = format!("require [\"fileinto\"];\nfileinto \"X\";\nredirect \"{MB}\";\nredirect :copy \"out@outside.test\";\n");
    t.io.put("acc-team", &hand);
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!(r.failed, 0, "{r:?}");
    assert_eq!(
        t.io.src("acc-team"),
        Some(format!("require [\"fileinto\"];\nfileinto \"X\";\nredirect \"{MB}\";\n\n")),
        "only the one that leaves goes"
    );
}

/// The old `k2-forward` script: a hand-written one with nothing outbound is left,
/// and one with a redirect moves into `k2` less the redirect.
#[test]
fn the_hourly_pass_treats_a_hand_written_legacy_script_the_same_way() {
    let t = t("fwstripl");
    let _ = crate::mail::forward_outcomes::take_notices();
    let quiet = "require [\"fileinto\"];\nfileinto \"X\";\n";
    t.io.legacy.borrow_mut().insert(ACC.to_string(), quiet.to_string());
    let ctx = crate::mail::forward_sync::ctx();
    crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!(t.io.legacy.borrow().get(ACC).map(String::as_str), Some(quiet), "nothing outbound: left alone");
    assert_eq!(t.io.src(ACC), None);

    t.io.legacy.borrow_mut().insert(ACC.to_string(), HAND.to_string());
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!((r.drift, r.failed), (1, 0), "{r:?}");
    assert!(t.io.legacy.borrow().get(ACC).is_none());
    assert_eq!(t.io.src(ACC), Some(HAND.replace("redirect \"boss@outside.test\";", "keep;")));
}

fn forwards_on(addr: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::live_for_address(&conn, &format!("ma-{addr}")).expect("live").map(|f| f.dest)
}

fn sieve_user_ooo(text: &str) -> crate::mail::sieve_user::UserSieve {
    crate::mail::sieve_user::UserSieve {
        ooo: Some(crate::mail::sieve_user::OooBlock { days: 7, text: text.to_string(), until: None, from: MB.to_string() }),
        forward: None,
    }
}
