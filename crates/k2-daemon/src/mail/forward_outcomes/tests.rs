//! Outside-forward outcomes, caps, auto-pause and the box brake against the
//! fake mail server (no Stalwart, no mail). The events are the shapes
//! Stalwart 0.16.25's webhook posts (span keys merged into `data`).

use std::cell::RefCell;

use k2_core::mail_forward as store;
use serde_json::{json, Value as J};

use super::*;
use crate::email_notify::webhook;
use crate::mail::defaults;
use crate::mail::forward_sync::tests::{install, seed_mailbox, uninstall, verified_row, FakeSieve};

struct Guard(#[allow(dead_code)] k2_core::db::TestDbGuard);
impl Drop for Guard {
    fn drop(&mut self) {
        uninstall();
        webhook::set_test_key(None);
        crate::email_notify::set_test_switch(None);
        let _ = take_notices();
    }
}

fn setup() -> (Guard, std::rc::Rc<FakeSieve>) {
    let g = Guard(k2_core::db::scoped_for_test());
    let io = install(false);
    let _ = take_notices();
    (g, io)
}

fn now() -> i64 {
    forward_sync::ctx().now
}

fn account(addr: &str) -> String {
    format!("acc-{}", addr.split('@').next().expect("local"))
}

/// An active outside forward, created at `at` (its window starts there),
/// with its guarded block written.
fn active(addr: &str, dest: &str, at: i64) -> Forward {
    let aid = seed_mailbox(addr, "direct");
    let v = verified_row(addr, dest);
    let f = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::create(
            &conn,
            &store::NewForward {
                address_id: &aid,
                address: addr,
                dest,
                dest_kind: store::KIND_EXTERNAL,
                keep: true,
                set_by: "owner",
                state: store::ST_ACTIVE,
                verified_address_id: Some(&v),
            },
            at,
        )
        .expect("create")
    };
    forward_sync::sync_mailbox(&aid, forward_sync::Why::Change).expect("sync");
    f
}

fn row(id: &str) -> Forward {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get(&conn, id).expect("get").expect("row")
}

fn events(list: J) -> Vec<HookEvent> {
    webhook::parse_events(json!({ "events": list }).to_string().as_bytes()).expect("parsed")
}

fn queued(qid: u64, from: &str, to: &str) -> J {
    json!({"id": format!("q{qid}"), "createdAt": "2026-10-30T00:00:00Z", "type": AUTOGEN_QUEUED,
           "data": {"queueId": qid, "from": from, "to": [to], "size": 900, "total": 1}})
}

/// A `delivery.dsn-perm-fail` as Stalwart writes it (no `code`; the
/// failure in `details`, enhanced status included).
fn perm_fail(qid: u64, from: &str, to: &str, code: u16, esc: &str, msg: &str) -> J {
    json!({"id": format!("p{qid}"), "type": DSN_PERM_FAIL, "data": {
        "to": to, "hostname": "mx.outside.test",
        "details": format!("Unexpected response for RCPT TO:<{to}>: Code: {code}, Enhanced code: {esc}, Message: {msg}"),
        "total": 1, "queueId": qid, "queueName": "default", "from": from}})
}

fn delivered(qid: u64, to: &str) -> J {
    json!({"id": format!("d{qid}"), "type": DELIVERED, "data": {"queueId": qid, "to": to, "code": 250, "details": "OK queued"}})
}

/// Record `n` queued outcomes for `f` directly (queue ids from `base`), at `at`.
fn seed_queued(f: &Forward, base: u64, n: u64, at: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for i in 0..n {
        let q = (base + i).to_string();
        store::link_queue(&conn, &q, &f.id, at).expect("link");
        assert!(store::record_outcome(&conn, &f.id, Some(&q), store::O_QUEUED, None, at).expect("queued"));
    }
}

// ── classifying ─────────────────────────────────────────────────────

#[test]
fn the_dsn_details_carry_the_enhanced_status_and_classify_user_vs_policy() {
    let r = |d: &str| refusal(d).0;
    let st = parse_status("Unexpected response for RCPT TO:<x@outside.test>: Code: 550, Enhanced code: 5.1.1, Message: The account does not exist");
    assert_eq!(st, Status { code: Some(550), enhanced: Some((5, 1, 1)) });
    assert_eq!(st.label().as_deref(), Some("550 5.1.1"));
    assert_eq!(r("Unexpected response for RCPT TO:<x@o.test>: Code: 550, Enhanced code: 5.1.1, Message: no"), store::O_REFUSED_USER);
    assert_eq!(r("Unexpected response for RCPT TO:<x@o.test>: Code: 550, Enhanced code: 5.2.1, Message: disabled"), store::O_REFUSED_USER);
    assert_eq!(r("Unexpected response for DATA: Code: 550, Enhanced code: 5.7.1, Message: rejected"), store::O_REFUSED_POLICY);
    assert_eq!(r("Unexpected response for DATA: Code: 554, Enhanced code: 5.7.26, Message: DMARC"), store::O_REFUSED_POLICY);
    // A full mailbox, a bad message: refused, but neither gone nor policy.
    assert_eq!(r("Unexpected response for DATA: Code: 552, Enhanced code: 5.2.2, Message: over quota"), store::O_FAILED);
    // No enhanced code (0.0.0): the text decides; policy words win.
    assert_eq!(parse_status("Code: 550, Enhanced code: 0.0.0, Message: x").enhanced, None);
    assert_eq!(r("Unexpected response for RCPT TO:<x@o.test>: Code: 550, Enhanced code: 0.0.0, Message: User unknown"), store::O_REFUSED_USER);
    assert_eq!(r("Unexpected response for DATA: Code: 550, Enhanced code: 0.0.0, Message: Message rejected as spam"), store::O_REFUSED_POLICY);
    assert_eq!(
        r("Unexpected response for RCPT TO:<x@o.test>: Code: 550, Enhanced code: 0.0.0, Message: user unknown or blocked by policy"),
        store::O_REFUSED_POLICY,
        "a policy refusal never reads as a gone mailbox"
    );
    assert_eq!(r("Unexpected response for RCPT TO:<x@o.test>: Code: 550, Enhanced code: 0.0.0, Message: mailbox unavailable"), store::O_FAILED);
    // Retries ran out on a temporary failure, or the host never answered.
    assert_eq!(r("Unexpected response for RCPT TO:<x@o.test>: Code: 451, Enhanced code: 4.7.1, Message: greylisted"), store::O_FAILED);
    assert_eq!(r("Connection failed: connection refused"), store::O_FAILED);
    assert_eq!(r("Rate limited"), store::O_FAILED);
    // A raw reply line (another server version's wording).
    assert_eq!(parse_status("550 5.1.1 <x@o.test>: Recipient address rejected"), Status { code: Some(550), enhanced: Some((5, 1, 1)) });
    assert_eq!(r("550-5.7.1 Our system has detected"), store::O_REFUSED_POLICY);
    // Which events count: rcpt-to-rejected never (the same refusal comes
    // back on dsn-perm-fail, with its enhanced code).
    let ev = events(json!([
        perm_fail(1, "a@b.test", "x@o.test", 550, "5.7.1", "spam"),
        delivered(1, "x@o.test"),
        {"type": "delivery.rcpt-to-rejected", "data": {"queueId": 1, "to": "x@o.test", "code": 550, "details": "No such user"}},
        {"type": FAILED, "data": {"queueId": 1, "to": "x@o.test", "reason": {"type": "x"}}},
    ]));
    assert_eq!(classify(&ev[0]), Some((store::O_REFUSED_POLICY, Some("550 5.7.1".to_string()))));
    assert_eq!(classify(&ev[1]), Some((store::O_DELIVERED, Some("250".to_string()))));
    assert_eq!(classify(&ev[2]), None);
    assert_eq!(classify(&ev[3]), Some((store::O_FAILED, None)));
}

#[test]
fn the_trip_rules_match_the_prd() {
    let c = |queued, refused_user, refused_policy, failed| Counts { queued, delivered: 0, refused_user, refused_policy, failed };
    assert_eq!(forward_trip(&c(3, 3, 0, 0), 3), Some(store::P_DEST_GONE));
    assert_eq!(forward_trip(&c(3, 2, 0, 0), 2), None);
    assert_eq!(forward_trip(&c(10, 0, 3, 0), 0), Some(store::P_REFUSED), "30% of 10");
    assert_eq!(forward_trip(&c(10, 0, 2, 0), 0), None, "exactly 20% is not more than 20%");
    assert_eq!(forward_trip(&c(9, 0, 9, 0), 0), None, "under 10 forwards");
    assert_eq!(forward_trip(&c(10, 0, 0, 9), 0), None, "failures aren't policy refusals");
    assert_eq!(forward_trip(&c(500, 0, 0, 0), 0), Some(store::P_DAILY_CAP));
    assert_eq!(forward_trip(&c(499, 0, 0, 0), 0), None);
    assert!(brake_trip(&c(50, 2, 2, 2)), "12% of 50");
    assert!(!brake_trip(&c(50, 5, 0, 0)), "exactly 10%");
    assert!(!brake_trip(&c(49, 10, 10, 10)), "under 50");
}

// ── the webhook join and the pauses ─────────────────────────────────

#[test]
fn three_gone_refusals_in_a_row_pause_the_forward_once_and_tell_the_owner_and_person() {
    let (_g, io) = setup();
    let (addr, dest) = ("sales@brand.test", "me@outside.test");
    let t = now() - 1000;
    let f = active(addr, dest, t);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_person_principal(&conn, &f.address_id, Some("pat")).expect("link");
    }
    assert!(io.forward(&account(addr)).is_some(), "forwarding");
    // Two refusals, a delivery (breaks the run), two refusals: no pause.
    let batch = json!([
        queued(11, addr, dest), queued(12, addr, dest), queued(13, addr, dest), queued(14, addr, dest), queued(15, addr, dest),
        // Another mailbox's redirect and a bounce are not this forward's.
        queued(90, "other@brand.test", dest), queued(91, "", dest),
        perm_fail(11, addr, dest, 550, "5.1.1", "no such user"),
        perm_fail(12, addr, dest, 550, "5.1.1", "no such user"),
        delivered(13, dest),
        perm_fail(14, addr, dest, 550, "5.1.1", "no such user"),
        perm_fail(15, addr, dest, 550, "5.2.1", "disabled"),
    ]);
    let a = apply(&events(batch.clone()), t + 10);
    assert_eq!((a.linked, a.recorded), (5, 10), "{a:?}");
    assert!(a.paused.is_empty(), "{a:?}");
    assert_eq!(row(&f.id).state, store::ST_ACTIVE);
    // The mail server sends the batch again: nothing new.
    let a = apply(&events(batch), t + 11);
    assert_eq!((a.linked, a.recorded, a.paused.len()), (0, 0, 0));
    // The third in a row pauses it: keep-only at once.
    let a = apply(&events(json!([queued(16, addr, dest), perm_fail(16, addr, dest, 550, "5.1.1", "gone")])), t + 20);
    assert_eq!(a.paused, vec![(f.id.clone(), store::P_DEST_GONE)]);
    let r = row(&f.id);
    assert_eq!((r.state.as_str(), r.pause_reason.as_deref()), (store::ST_PAUSED, Some(store::P_DEST_GONE)));
    assert_eq!(io.forward(&account(addr)), None, "paused = keep-only");
    let n = take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].0, format!("Forwarding from {addr} is paused"));
    assert!(n[0].1.contains(&format!("said {dest} doesn't exist or is disabled, 3 times in a row")), "{}", n[0].1);
    assert!(n[0].1.contains(&format!("k2 hostmail forward resume {addr}")), "{}", n[0].1);
    assert_eq!(n[0].2, vec!["pat".to_string()], "the mailbox's person hears too");
    // The audit row carries counts, not addresses.
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ev = store::events_for_address(&conn, &f.address_id, 5).expect("events");
    assert_eq!((ev[0].event.as_str(), ev[0].actor.as_str()), (store::E_PAUSED, forward_sync::ACTOR_K2));
    assert_eq!(ev[0].detail["inARow"], 3);
    assert!(!ev[0].detail.to_string().contains('@'), "{}", ev[0].detail);
    // The code is kept, the reply text (which names the address) isn't.
    let code: String = conn
        .query_row("SELECT code FROM mail_forward_outcomes WHERE queue_id = '16' AND kind = 'refused_user'", [], |r| r.get(0))
        .expect("outcome");
    assert_eq!(code, "550 5.1.1");
    drop(conn);
    // More refusals after the pause: no second pause, no second notice.
    let a = apply(&events(json!([perm_fail(16, addr, dest, 550, "5.1.1", "gone")])), t + 30);
    assert!(a.paused.is_empty());
    assert!(take_notices().is_empty());
}

#[test]
fn a_resumed_forward_counts_from_its_resume() {
    let (_g, io) = setup();
    let (addr, dest) = ("ops@brand.test", "ops@outside.test");
    let t = now() - 5000;
    let f = active(addr, dest, t);
    let mut list = Vec::new();
    for q in 21..24 {
        list.push(queued(q, addr, dest));
        list.push(perm_fail(q, addr, dest, 550, "5.1.1", "gone"));
    }
    assert_eq!(apply(&events(J::Array(list)), t + 10).paused, vec![(f.id.clone(), store::P_DEST_GONE)]);
    let _ = take_notices();
    forward_sync::resume(&f.id, "owner").expect("resume");
    assert!(io.forward(&account(addr)).is_some());
    // One more refusal after the resume is one in a row, not four.
    let a = apply(&events(json!([queued(24, addr, dest), perm_fail(24, addr, dest, 550, "5.1.1", "gone")])), now());
    assert!(a.paused.is_empty(), "{a:?}");
    assert_eq!(row(&f.id).state, store::ST_ACTIVE);
}

#[test]
fn policy_refusals_over_a_fifth_pause_it_but_only_from_ten_forwards() {
    let (_g, io) = setup();
    let (addr, dest) = ("news@brand.test", "list@outside.test");
    let t = now() - 1000;
    let f = active(addr, dest, t);
    let mut list: Vec<J> = (31..40).map(|q| queued(q, addr, dest)).collect();
    for q in 31..34 {
        list.push(perm_fail(q, addr, dest, 550, "5.7.1", "message looks like spam"));
    }
    let a = apply(&events(J::Array(list)), t + 10);
    assert!(a.paused.is_empty(), "9 forwards is under the minimum: {a:?}");
    let a = apply(&events(json!([queued(40, addr, dest)])), t + 20);
    assert_eq!(a.paused, vec![(f.id.clone(), store::P_REFUSED)]);
    assert_eq!(io.forward(&account(addr)), None);
    let n = take_notices();
    assert_eq!(n.len(), 1);
    assert!(n[0].1.contains("refused 3 of the 10 messages forwarded in the last 24 hours as spam or against its policy"), "{}", n[0].1);
    assert!(n[0].2.is_empty(), "no person linked: owner and Admins only");
}

#[test]
fn the_daily_cap_pauses_at_500_forwards() {
    let (_g, io) = setup();
    let (addr, dest) = ("busy@brand.test", "busy@outside.test");
    let t = now() - 1000;
    let f = active(addr, dest, t);
    seed_queued(&f, 1000, 499, t + 5);
    assert_eq!(evaluate(&f.id, t + 6), None, "499 is under the cap");
    let a = apply(&events(json!([queued(5000, addr, dest)])), t + 10);
    assert_eq!(a.paused, vec![(f.id.clone(), store::P_DAILY_CAP)]);
    assert_eq!(io.forward(&account(addr)), None);
    let n = take_notices();
    assert!(n[0].1.contains("forwarded 500 messages in the last 24 hours, the daily limit of 500"), "{}", n[0].1);
    // Older than 24 hours doesn't count.
    let (addr2, dest2) = ("old@brand.test", "old@outside.test");
    let t2 = now() - 3 * 86_400;
    let g = active(addr2, dest2, t2);
    seed_queued(&g, 20_000, 499, t2 + 5);
    assert!(apply(&events(json!([queued(30_000, addr2, dest2)])), now()).paused.is_empty());
}

#[test]
fn the_box_brake_pauses_every_outside_forward_with_one_notice_until_the_owner_resumes() {
    let (_g, io) = setup();
    let t = now() - 1000;
    let a = active("a@brand.test", "a@outside.test", t);
    let b = active("b@brand.test", "b@outside.test", t);
    // A same-box forward is never braked.
    let internal_aid = seed_mailbox("team@brand.test", "direct");
    seed_mailbox("lead@brand.test", "direct");
    let internal = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::create(
            &conn,
            &store::NewForward {
                address_id: &internal_aid,
                address: "team@brand.test",
                dest: "lead@brand.test",
                dest_kind: store::KIND_INTERNAL,
                keep: true,
                set_by: "owner",
                state: store::ST_ACTIVE,
                verified_address_id: None,
            },
            t,
        )
        .expect("internal")
    };
    // 50 forwards from A; 5 refused for policy, then a 6th: 12% box-wide
    // (A alone is under its own 20%).
    seed_queued(&a, 100, 50, t + 5);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for q in 100..105 {
            store::record_outcome(&conn, &a.id, Some(&q.to_string()), store::O_REFUSED_POLICY, Some("550 5.7.1"), t + 5).expect("refused");
        }
    }
    let out = apply(&events(json!([perm_fail(105, "a@brand.test", "a@outside.test", 550, "5.7.1", "policy")])), t + 10);
    let mut paused = out.paused.clone();
    paused.sort();
    let mut want = vec![(a.id.clone(), store::P_BOX_BRAKE), (b.id.clone(), store::P_BOX_BRAKE)];
    want.sort();
    assert_eq!(paused, want, "{out:?}");
    assert_eq!(io.forward("acc-a"), None);
    assert_eq!(io.forward("acc-b"), None);
    assert_eq!(row(&internal.id).state, store::ST_ACTIVE);
    let n = take_notices();
    assert_eq!(n.len(), 1, "one notice for the brake: {n:?}");
    assert_eq!(n[0].0, "Forwarding to outside addresses is paused on this server");
    assert!(n[0].1.contains("refused 6 of the 50 messages") && n[0].1.contains("paused all 2 outside forwards"), "{}", n[0].1);
    assert!(n[0].1.contains("Only the owner or an Admin can resume them"), "{}", n[0].1);
    // A forward confirmed while the brake holds is paused with the rest,
    // without another notice.
    let c_aid = seed_mailbox("c@brand.test", "direct");
    let v = verified_row("c@brand.test", "c@outside.test");
    let c = forward_sync::change_and_sync(&c_aid, |conn, now| {
        store::create(
            conn,
            &store::NewForward {
                address_id: &c_aid,
                address: "c@brand.test",
                dest: "c@outside.test",
                dest_kind: store::KIND_EXTERNAL,
                keep: true,
                set_by: "owner",
                state: store::ST_ACTIVE,
                verified_address_id: Some(&v),
            },
            now,
        )
        .map_err(store::TransitionError::Db)
    })
    .expect("set");
    let c = row(&c.id);
    assert_eq!((c.state.as_str(), c.pause_reason.as_deref()), (store::ST_PAUSED, Some(store::P_BOX_BRAKE)));
    assert!(take_notices().is_empty(), "the brake already told the owner");
    // The owner resumes B: the brake's window starts again, B stays on.
    forward_sync::resume(&b.id, "owner").expect("resume");
    assert_eq!(row(&b.id).state, store::ST_ACTIVE);
    assert!(io.forward("acc-b").is_some());
    let out = apply(&events(json!([queued(200, "b@brand.test", "b@outside.test")])), now());
    assert!(out.paused.is_empty(), "{out:?}");
    assert_eq!(row(&a.id).state, store::ST_PAUSED, "the others wait for their own resume");
}

// ── the webhook stays on ────────────────────────────────────────────

fn hook(key: &str, body: &[u8]) -> crate::cli_response::CliResponse {
    let head = format!("POST /cli/email/events HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Signature: {}\r\n\r\n", webhook::sign(key, body));
    webhook::handle(true, &head, body)
}

fn status(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split_whitespace().next().expect("status").parse().expect("number")
}

#[test]
fn the_webhook_stays_on_while_an_outside_forward_is_live_with_email_notify_off() {
    let (_g, io) = setup();
    webhook::set_test_key(Some(Some("hook-key".into())));
    crate::email_notify::set_test_switch(Some(false));
    assert!(!webhook_wanted());
    assert!(!webhook::wanted());
    let body = json!({ "events": [] }).to_string().into_bytes();
    assert_eq!(status(&hook("hook-key", &body)), 404, "switch off, no outside forward: unknown route");
    let (addr, dest) = ("solo@brand.test", "solo@outside.test");
    let f = active(addr, dest, now() - 100);
    assert!(webhook_wanted() && webhook::wanted());
    let body = json!({ "events": [queued(7, addr, dest), delivered(7, dest)] }).to_string().into_bytes();
    let r = hook("hook-key", &body);
    assert_eq!(status(&r), 200, "{}", r.body);
    let v: J = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["forwards"], 2, "{v}");
    assert_eq!(status(&hook("other-key", &body)), 401, "still signed");
    let _ = io;
    forward_sync::transition_and_sync(&f.id, store::ST_REMOVED, &store::Change { actor: "owner", ..Default::default() }).expect("removed");
    assert!(!webhook::wanted());
    assert_eq!(status(&hook("hook-key", &body)), 404);
}

// ── the throttles ───────────────────────────────────────────────────

#[derive(Default)]
struct FakeThrottles {
    objs: RefCell<Vec<J>>,
    calls: RefCell<Vec<String>>,
}

impl defaults::ThrottleApi for FakeThrottles {
    fn throttles(&self) -> Result<Vec<J>, String> {
        Ok(self.objs.borrow().clone())
    }
    fn throttle_create(&self, mut body: J) -> Result<String, String> {
        let id = format!("t{}", self.objs.borrow().len() + 1);
        self.calls.borrow_mut().push(format!("create {id}"));
        body["id"] = json!(id);
        // The registry gives the expression back with an empty match list.
        body["match"]["match"] = json!({});
        self.objs.borrow_mut().push(body);
        Ok(id)
    }
    fn throttle_update(&self, id: &str, mut body: J) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("update {id}"));
        body["id"] = json!(id);
        let mut o = self.objs.borrow_mut();
        let slot = o.iter_mut().find(|x| x["id"] == json!(id)).expect("exists");
        *slot = body;
        Ok(())
    }
    fn throttle_destroy(&self, id: &str) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("destroy {id}"));
        self.objs.borrow_mut().retain(|x| x["id"] != json!(id));
        Ok(())
    }
    fn reload_settings(&self) -> Result<(), String> {
        self.calls.borrow_mut().push("reload".into());
        Ok(())
    }
}

#[test]
fn the_two_throttles_match_the_probe_file_and_are_written_once() {
    // The loopback probe (X9) posts this file verbatim.
    let file: J = serde_json::from_str(include_str!("../../../../../tests/mail/forward-throttles.json")).expect("json");
    assert_eq!(J::Array(defaults::forward_throttles()), file, "tests/mail/forward-throttles.json is what K2 writes");
    for t in defaults::forward_throttles() {
        assert_eq!(t["match"]["else"], "source == 'autogenerated'");
        assert!(defaults::unknown_functions(&t, &defaults::known_functions((0, 16, 25))).is_empty(), "{t}");
        assert!(t["description"].as_str().expect("description").starts_with(defaults::FORWARD_THROTTLE_PREFIX));
    }
    assert_eq!(file[0]["key"], json!({"sender": true}));
    assert_eq!(file[0]["rate"], json!({"count": store::HOURLY_CAP, "period": 3_600_000}));
    assert_eq!(file[1]["key"], json!({"localIp": true}));
    assert_eq!(file[1]["rate"], json!({"count": store::BOX_HOURLY_CAP, "period": 3_600_000}));

    let api = FakeThrottles::default();
    // Someone else's throttle is never touched.
    api.objs.borrow_mut().push(json!({"id": "mine", "description": "relay limit", "enable": true, "key": {"rcptDomain": true},
                                      "match": {"else": "true", "match": {}}, "rate": {"count": 5, "period": 1000}}));
    let lines = defaults::reconcile_forward_throttles(&api);
    assert_eq!(*api.calls.borrow(), vec!["create t2", "create t3", "reload"], "{lines:?}");
    api.calls.borrow_mut().clear();
    assert!(defaults::reconcile_forward_throttles(&api).is_empty(), "idempotent");
    assert!(api.calls.borrow().is_empty());
    // Someone changed one of K2's; an older K2 left a duplicate.
    api.objs.borrow_mut()[1]["rate"]["count"] = json!(5000);
    let mut dup = api.objs.borrow()[2].clone();
    dup["id"] = json!("t9");
    api.objs.borrow_mut().push(dup);
    defaults::reconcile_forward_throttles(&api);
    assert_eq!(*api.calls.borrow(), vec!["update t2", "destroy t9", "reload"]);
    let objs = api.objs.borrow();
    assert_eq!(objs.len(), 3);
    assert_eq!(objs[0]["id"], "mine");
    assert_eq!(objs[0]["rate"]["count"], 5, "untouched");
    assert_eq!(objs[1]["rate"]["count"], store::HOURLY_CAP);
}
