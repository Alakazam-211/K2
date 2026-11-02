//! The forward routes end to end against fakes: the mail engine (notice
//! mails), a fake Sieve (forward_sync) and the verify and stop pages. No
//! mail server, no real mail.

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
    // `ws-fw` owns the mailboxes here. Its agent may send, unless a test says
    // otherwise (review finding 9).
    agent_sending("on");
    k2_core::skin::set_front_door("direct", Some("https://box.brand.test"), None, None).expect("door");
    let io = install(false);
    seed_mailbox(MB, "direct");
    T { f, io }
}

fn agent_sending(mode: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, mail_agent_send) VALUES ('ws-fw', 'it-agent-folder', '/tmp/it-agent-folder', ?1) \
         ON CONFLICT(id) DO UPDATE SET mail_agent_send = ?1",
        rusqlite::params![mode],
    )
    .expect("workspace");
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
        ADD_PATH => handle_forward_add(b.as_bytes()),
        RM_PATH => handle_forward_rm(b.as_bytes()),
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

fn sent(t: &T) -> usize {
    t.f.engine.sent.lock().unwrap().len()
}

fn forwards() -> Vec<J> {
    show()["forwards"].as_array().expect("forwards").clone()
}

fn one<'a>(rows: &'a [J], to: &str) -> &'a J {
    rows.iter().find(|f| f["to"] == to).unwrap_or_else(|| panic!("no row for {to} in {rows:?}"))
}

/// Opt-out: active at once, with one notice saying so, and no confirm step.
#[test]
fn an_outside_forward_goes_on_at_once_with_one_notice_and_no_confirm() {
    let t = t("fwout");
    let sys = system_sender_on();
    let _ = crate::mail::forward_outcomes::take_notices();
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "Me@Outside.test" }));
    assert_eq!((st, v["state"].clone(), v["keep"].clone()), (200, json!("active"), json!(true)), "{v}");
    assert!(v["message"].as_str().unwrap_or("").contains("with a link to stop it"), "{v}");
    let fwd = t.io.forward(ACC).expect("forward written");
    assert_eq!((fwd.dest.as_str(), fwd.keep, fwd.guard.map(|g| g.host)), (OUT, true, Some(HOST.to_string())));
    // One mail: the notice.
    assert_eq!(sent(&t), 1);
    let mail = t.f.engine.sent.lock().unwrap().last().cloned().expect("notice");
    assert_eq!((mail.from.as_str(), mail.rcpt.clone()), (sys.address.as_str(), vec![OUT.to_string()]));
    let text = mail.email.to_string();
    assert!(text.contains(&format!("copies of the mail that arrives at {MB}")), "{text}");
    assert!(text.contains("The owner of"), "who set it: {text}");
    assert!(text.contains("k2es_"), "a stop link: {text}");
    assert!(!text.contains("k2ev_"), "no confirm link: {text}");
    let s = show();
    assert_eq!((s["forward"]["state"].clone(), s["to"].clone(), s["forward"]["notice"].clone()), (json!("active"), json!(OUT), json!("sent")), "{s}");
    let events: Vec<&str> = s["events"].as_array().expect("events").iter().rev().filter_map(|e| e["event"].as_str()).collect();
    assert_eq!(events, vec!["set", "activated", "notice_sent"], "{s}");
    assert_eq!(s["events"][0]["actor"], "owner", "audited with who: {s}");
    // The owner heard, naming the mailbox, the address and who set it.
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains(MB) && n[0].1.contains(OUT) && n[0].1.contains("the owner or an Admin"), "{}", n[0].1);
    // Setting it again changes nothing and mails nobody.
    let (st, _) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, sent(&t), forwards().len()), (200, 1, 1));
}

/// The notice goes out before the Sieve is written, and a Sieve that can't be
/// written doesn't lose it.
#[test]
fn the_notice_goes_out_before_the_redirect_exists() {
    let t = t("fworder");
    system_sender_on();
    *t.io.fail.borrow_mut() = true;
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(sent(&t), 1, "the notice is out");
    assert_eq!(t.io.src(ACC), None, "the redirect isn't");
    assert!(v["note"].as_str().unwrap_or("").contains("within the hour"), "{v}");
}

/// A failed notice doesn't block the forward; it shows, and K2 tries again.
#[test]
fn a_failed_notice_shows_and_is_retried_hourly_without_blocking() {
    let t = t("fwfail");
    system_sender_on();
    *t.f.engine.fail_submit.lock().unwrap() = Some("the mail server is down".to_string());
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    assert!(v["message"].as_str().unwrap_or("").contains("couldn't email"), "{v}");
    assert!(t.io.forward(ACC).is_some(), "a failure that may pass doesn't block");
    assert_eq!(v["destinations"][0]["notice"], "failed", "{v}");
    let n = crate::mail::forward_outcomes::take_notices();
    assert!(n[0].1.contains("K2 couldn't email") && !n[0].1.contains("K2 emailed"), "the owner isn't told it went: {n:?}");
    let list = body(&handle_forwards_list(&HashMap::new()));
    assert_eq!(list["forwards"][0]["notice"], "failed", "forward list shows it: {list}");
    assert!(list["forwards"][0]["noticeWords"].as_str().unwrap_or("").contains("stop link"), "{list}");
    *t.f.engine.fail_submit.lock().unwrap() = None;
    let at = email_notify::now();
    assert_eq!(forward_verify::minute_pass(at).retried, 0, "not within the hour");
    assert_eq!(forward_verify::minute_pass(at + store::NOTICE_RETRY_SECS + 60).retried, 1);
    assert_eq!(show()["forward"]["notice"], "sent");
    assert!(t.f.engine.sent.lock().unwrap().last().expect("mail").email.to_string().contains("k2es_"));
    assert_eq!(forward_verify::minute_pass(at + 2 * store::NOTICE_RETRY_SECS).retried, 0, "told once");
}

/// Review finding 2: an address K2 has stopped mailing can't get a stop link, so
/// the add is refused, not started.
#[test]
fn a_suppressed_address_is_refused() {
    let t = t("fwsupp");
    system_sender_on();
    let n = sent(&t);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::email_outcomes::suppress(&conn, OUT, k2_core::email_outcomes::SCOPE_BOX, "", k2_core::email_outcomes::R_HARD, "dsn", email_notify::now()).expect("suppress");
    }
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("suppressed")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("bounced or complained"), "{v}");
    assert_eq!((sent(&t), forwards().len()), (n, 0), "nothing went, nothing changed");
    assert_eq!(t.io.src(ACC), None);
}

/// Review finding 2: no front door, no stop link, no forward.
#[test]
fn no_front_door_refuses_an_outside_forward() {
    let t = t("fwdoor");
    system_sender_on();
    k2_core::skin::set_front_door("direct", None, None, None).expect("no door");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("no_front_door")), "{v}");
    assert_eq!(t.io.src(ACC), None, "no mail flows");
    assert_eq!(forwards().len(), 0);
    // A same-box forward needs no link.
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, _) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!(st, 200);
}

/// Review finding 2: a failure that may pass doesn't block, but it backs off
/// without piling up rows, and after a day the forward pauses until the email
/// goes, then goes back on by itself.
#[test]
fn a_notice_that_keeps_failing_pauses_the_forward_after_a_day() {
    let t = t("fwnottold");
    system_sender_on();
    *t.f.engine.fail_submit.lock().unwrap() = Some("the mail server is down".to_string());
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    let stops = || {
        let db = k2_core::db::shared();
        let n: i64 = db.lock().query_row("SELECT COUNT(*) FROM email_verified_stops", [], |r| r.get(0)).expect("count");
        n
    };
    assert_eq!(stops(), 0, "the failed try left no link row");
    let at = email_notify::now();
    let _ = crate::mail::forward_outcomes::take_notices();
    // Tries back off: passes between them do nothing.
    assert_eq!(forward_verify::minute_pass(at + 60), forward_verify::Pass::default());
    for hours in [1, 3, 8, 14, 20] {
        forward_verify::minute_pass(at + hours * 3600);
        assert_eq!(show()["forward"]["state"], "active", "{hours}h");
    }
    assert_eq!(stops(), 0, "still none");
    // Past a day, the next try pauses it, and the owner hears once.
    forward_verify::minute_pass(at + 26 * 3600);
    let s = show();
    assert_eq!((s["forward"]["state"].clone(), s["forward"]["pauseReason"].clone()), (json!("paused"), json!("not_told")), "{s}");
    assert_eq!(t.io.forward(ACC), None, "nothing leaves with no stop link");
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains(OUT) && n[0].1.contains("K2 paused it"), "{}", n[0].1);
    // Nobody resumes it by hand; it's held.
    let (st, v) = post(RESUME_PATH, json!({ "address": MB }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("held")), "{v}");
    // The email goes: back on, and the owner hears.
    *t.f.engine.fail_submit.lock().unwrap() = None;
    let pass = forward_verify::minute_pass(at + 40 * 3600);
    assert_eq!(pass.retried, 1, "{pass:?}");
    let s = show();
    assert_eq!((s["forward"]["state"].clone(), s["forward"]["notice"].clone()), (json!("active"), json!("sent")), "{s}");
    assert!(t.io.forward(ACC).is_some());
    assert!(crate::mail::forward_outcomes::take_notices()[0].0.contains("back on"));
    assert_eq!(stops(), 1, "one link, the one that went");
}

/// Review finding 2: a row waiting its turn doesn't go on until its email has
/// gone, and the owner hears after a day of failing.
#[test]
fn a_waiting_row_stays_off_until_its_email_goes() {
    let t = t("fwduefail");
    system_sender_on();
    let (id, _) = seed_pending(MB, OUT, 3600, "owner");
    *t.f.engine.fail_submit.lock().unwrap() = Some("down".to_string());
    let at = email_notify::now();
    let pass = forward_verify::minute_pass(at);
    assert_eq!(pass.turned_on, 0, "{pass:?}");
    let row = db(|c| store::get(c, &id)).ok().flatten().expect("row");
    assert_eq!((row.state.as_str(), row.notice_state.as_deref()), (store::ST_PENDING_VERIFY, Some(store::N_DUE)));
    assert_eq!(t.io.forward(ACC), None);
    assert!(forwards()[0]["noticeWords"].as_str().unwrap_or("").contains("stays off until that email goes"), "forward list says so");
    let _ = crate::mail::forward_outcomes::take_notices();
    forward_verify::minute_pass(at + 26 * 3600);
    forward_verify::minute_pass(at + 40 * 3600);
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.iter().filter(|(title, _, _)| title.contains("can't be told")).count(), 1, "once: {n:?}");
    *t.f.engine.fail_submit.lock().unwrap() = None;
    assert_eq!(forward_verify::minute_pass(at + 60 * 3600).turned_on, 1);
    assert!(t.io.forward(ACC).is_some());
}

/// Review finding 3: re-posting a stop link changes nothing and tells nobody, and
/// the page refuses after enough tries.
#[test]
fn re_posting_a_stop_link_tells_nobody_and_is_rate_limited() {
    let t = t("fwstopflood");
    system_sender_on();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let i = k2_core::email_verify::issue(&conn, k2_core::email_verify::PURPOSE_PERSON_NOTICE, "owner", "boss@elsewhere.test", "owner", 1).expect("issue");
        k2_core::email_verify::confirm(&conn, &i.token, 2).expect("confirm");
    }
    post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    let tok = {
        let mails = t.f.engine.sent.lock().unwrap().clone();
        let text = mails.iter().find(|m| m.rcpt == vec![OUT.to_string()]).expect("the notice").email.to_string();
        let at = text.find("k2es_").expect("stop link");
        text[at..].chars().take(5 + 43).collect::<String>()
    };
    let _ = crate::mail::forward_outcomes::take_notices();
    let before = sent(&t);
    for _ in 0..5 {
        assert!(vpages::page_post(STOP_PATH, "", format!("t={tok}").as_bytes()).status.starts_with("200"));
    }
    assert_eq!(crate::mail::forward_outcomes::take_notices().len(), 1, "one press changed something");
    assert_eq!(sent(&t) - before, 1, "one owner email");
    for _ in 0..5 {
        vpages::page_post(STOP_PATH, "", format!("t={tok}").as_bytes());
    }
    assert!(vpages::page_post(STOP_PATH, "", format!("t={tok}").as_bytes()).status.starts_with("429"));
}

/// Review finding 4: a pile of keep flips can't spend the slots new addresses
/// use, and a summary names every new address, however many.
#[test]
fn new_addresses_are_always_named_and_keep_flips_don_t_spend_their_slots() {
    let t = t("fwbury");
    system_sender_on();
    seed_server();
    set_cap(store::NEW_OUTSIDE_PER_DAY_MAX);
    let _ = (&t, crate::mail::forward_outcomes::take_notices());
    post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    let _ = crate::mail::forward_outcomes::take_notices();
    // Cheap changes first.
    for i in 0..30 {
        let (st, _) = post(ADD_PATH, json!({ "address": MB, "to": OUT, "keep": i % 2 == 0 }));
        assert_eq!(st, 200);
    }
    let _ = crate::mail::forward_outcomes::take_notices();
    // The real address still gets its own note, at once.
    let (st, _) = post(ADD_PATH, json!({ "address": MB, "to": "real@outside.test" }));
    assert_eq!(st, 200);
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains("real@outside.test"), "{}", n[0].1);
    // Past the adds' own slots, every one is still named in the summary.
    let total = store::OWNER_NOTES_PER_HOUR + 30;
    add_many(0, total);
    let _ = crate::mail::forward_outcomes::take_notices();
    forward_verify::flush_owner_summary(email_notify::now() + store::OWNER_SUMMARY_SECS);
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains(&format!("d{}@outside.test", total - 1)), "the last one is named: {}", n[0].1);
    assert!(n[0].1.contains("New outside addresses"), "{}", n[0].1);
}

/// Review finding 5: an agent can't undo a person's pause, and the owner hears
/// about outside pauses and resumes.
#[test]
fn an_agent_can_t_undo_someone_else_s_pause_and_the_owner_hears() {
    let t = t("fwpausewho");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    let _ = crate::mail::forward_outcomes::take_notices();
    let (st, _) = post(PAUSE_PATH, json!({ "address": MB }));
    assert_eq!(st, 200);
    assert!(crate::mail::forward_outcomes::take_notices()[0].1.contains(&format!("Paused: {OUT}")));
    let (st, v) = as_agent("ws-fw", || post(RESUME_PATH, json!({ "address": MB })));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    assert_eq!(t.io.forward(ACC), None, "still paused");
    let (st, _) = post(RESUME_PATH, json!({ "address": MB }));
    assert_eq!(st, 200);
    let n = crate::mail::forward_outcomes::take_notices();
    assert!(n[0].1.contains("Resumed"), "{n:?}");
    // Its own pause it can undo; the owner hears both, naming it.
    let (st, _) = as_agent("ws-fw", || post(PAUSE_PATH, json!({ "address": MB })));
    assert_eq!(st, 200);
    assert!(crate::mail::forward_outcomes::take_notices()[0].1.contains("the AI agent"));
    let (st, v) = as_agent("ws-fw", || post(RESUME_PATH, json!({ "address": MB })));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    assert!(crate::mail::forward_outcomes::take_notices()[0].1.contains("the AI agent"));
}

/// Review finding 6: `rm` and `add` don't wash out an automatic pause.
#[test]
fn rm_then_add_doesn_t_clear_an_automatic_pause_or_the_daily_count() {
    let t = t("fwlaunder");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    let id = show()["forward"]["id"].as_str().expect("id").to_string();
    let now = email_notify::now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE mail_forward_events SET at = at - 1000 WHERE forward_id = ?1", rusqlite::params![id]).expect("age");
        for i in 0..store::DAILY_CAP {
            store::record_outcome(&conn, &id, Some(&format!("q{i}")), store::O_QUEUED, None, now - 100).expect("outcome");
        }
    }
    assert_eq!(crate::mail::forward_outcomes::evaluate(&id, now), Some(store::P_DAILY_CAP));
    assert_eq!(t.io.forward(ACC), None);
    let (st, _) = as_agent("ws-fw", || post(RM_PATH, json!({ "address": MB, "to": OUT })));
    assert_eq!(st, 200, "an agent may remove it");
    let (st, v) = as_agent("ws-fw", || post(ADD_PATH, json!({ "address": MB, "to": OUT })));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    assert_eq!(t.io.forward(ACC), None, "still nothing flowing");
    // A person can, and the count stays honest: the mailbox is still over.
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!(st, 200, "{v}");
    let new_id = show()["forward"]["id"].as_str().expect("id").to_string();
    assert_ne!(new_id, id);
    assert_eq!(crate::mail::forward_outcomes::evaluate(&new_id, now + 1), Some(store::P_DAILY_CAP), "the count didn't start over");
}

/// Review finding 7: an address that stopped and was re-asked under 0.46.7 doesn't
/// go live on update, and gets no notice.
#[test]
fn a_stopped_then_re_asked_row_doesn_t_go_live_on_update() {
    let t = t("fwreask");
    system_sender_on();
    let (_, i) = seed_pending(MB, OUT, 3600, "owner");
    assert!(vpages::page_post(STOP_PATH, "", format!("t={}", i.stop).as_bytes()).status.starts_with("200"));
    let (fid2, _) = seed_pending(MB, OUT, 1800, "agent:x");
    let pass = forward_verify::minute_pass(email_notify::now());
    assert_eq!(pass.turned_on, 0, "{pass:?}");
    let row = db(|c| store::get(c, &fid2)).ok().flatten().expect("row");
    assert_eq!(row.state, store::ST_REMOVED, "{row:?}");
    assert_eq!(t.io.forward(ACC), None);
    assert_eq!(sent(&t), 0);
}

/// Review finding 8: when the mail server can't be told, the page doesn't claim
/// it's done.
#[test]
fn the_stop_page_is_honest_when_the_write_fails() {
    let t = t("fwhonest");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": ["a@outside.test", "b@outside.test", "c@outside.test"] }));
    let tok = last_token(&t, "k2es_");
    // The limit call failing doesn't matter to a removal.
    *t.io.limit_fail.borrow_mut() = true;
    let p = vpages::page_post(STOP_PATH, "", format!("t={tok}").as_bytes());
    assert!(p.html.contains("<h1>Stopped</h1>"), "{}", p.html);
    assert!(!t.io.src(ACC).expect("script").contains("c@outside.test"), "written");
    *t.io.limit_fail.borrow_mut() = false;
    // A write that fails outright: it says what's true.
    let (_, stop) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::email_verify::open_for_notice(&conn, MB, "a@outside.test", "owner", email_notify::now()).expect("open")
    };
    *t.io.fail.borrow_mut() = true;
    let p = vpages::page_post(STOP_PATH, "", format!("t={stop}").as_bytes());
    assert!(p.html.contains("K2 has your stop") && !p.html.contains("<h1>Stopped</h1>"), "{}", p.html);
    *t.io.fail.borrow_mut() = false;
    let ctx = crate::mail::forward_sync::ctx();
    crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert!(!t.io.src(ACC).expect("script").contains("a@outside.test"), "the reconcile finishes it");
}

/// Review finding 9: an agent's own mailbox doesn't go outside past its sending
/// rules. Someone else's it manages is another matter, and a person can.
#[test]
fn an_agent_s_own_mailbox_needs_agent_sending_on_to_forward_outside() {
    let t = t("fwownsend");
    system_sender_on();
    for mode in ["off", "approval"] {
        agent_sending(mode);
        let (st, v) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": OUT })));
        assert_eq!((st, v["error"]["code"].clone()), (403, json!("agent_send_required")), "{mode}: {v}");
    }
    assert_eq!((sent(&t), forwards().len()), (0, 0));
    // A same-box forward isn't sending out.
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, v) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" })));
    assert_eq!(st, 200, "{v}");
    // A person can set it.
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!(st, 200, "{v}");
    post(UNSET_PATH, json!({ "address": MB }));
    agent_sending("on");
    let (st, v) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": "other@outside.test" })));
    assert_eq!(st, 200, "{v}");
}

/// Review finding 11: the gaps in what the owner and the person hear.
#[test]
fn the_owner_hears_no_copy_and_the_linked_person_hears_an_agent() {
    let t = t("fwpersonhears");
    system_sender_on();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_person_principal(&conn, &format!("ma-{MB}"), Some("pat")).expect("link");
    }
    let _ = crate::mail::forward_outcomes::take_notices();
    let (st, v) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": OUT, "keep": false })));
    assert_eq!((st, v["keep"].clone()), (200, json!(false)), "{v}");
    assert!(!t.io.forward(ACC).expect("fwd").keep);
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains("No copy stays"), "{}", n[0].1);
    assert_eq!(n[0].2, vec!["pat".to_string()], "the mailbox's person hears");
    // A person doing it themselves isn't told about themselves by this.
    let (st, _) = post(ADD_PATH, json!({ "address": MB, "to": "b@outside.test" }));
    assert_eq!(st, 200);
    assert!(crate::mail::forward_outcomes::take_notices()[0].2.is_empty());
}

#[test]
fn a_change_of_the_daily_cap_tells_the_owner() {
    let _t = t("fwcapnote");
    system_sender_on();
    seed_server();
    let _ = crate::mail::forward_outcomes::take_notices();
    let (st, v) = set_cap(1000);
    assert_eq!(st, 200, "{v}");
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains("1000") && n[0].1.contains(&format!("it was {}", store::NEW_OUTSIDE_PER_DAY)), "{}", n[0].1);
}

/// Review finding 12: `forward list` shows it.
#[test]
fn forward_list_says_when_delivery_reports_are_broken() {
    let _t = t("fwhook");
    seed_server();
    assert_eq!(body(&handle_forwards_list(&HashMap::new()))["outcomes"]["ok"], true);
    crate::mail::forward_outcomes::webhook_health(Some("no management client"));
    let v = body(&handle_forwards_list(&HashMap::new()));
    assert_eq!((v["outcomes"]["ok"].clone(), v["outcomes"]["error"].clone()), (json!(false), json!("no management client")), "{v}");
}

/// Review finding 14: nobody gets the box's whole cap in notices.
#[test]
fn one_address_gets_at_most_a_few_notices_a_day() {
    let t = t("fwcool");
    system_sender_on();
    seed_server();
    set_cap(store::NEW_OUTSIDE_PER_DAY_MAX);
    let n = sent(&t);
    for _ in 0..store::NOTICES_PER_DEST_PER_DAY {
        let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": OUT }));
        assert_eq!(st, 200, "{v}");
        let (st, _) = post(RM_PATH, json!({ "address": MB, "to": OUT }));
        assert_eq!(st, 200);
    }
    assert_eq!(sent(&t) - n, store::NOTICES_PER_DEST_PER_DAY as usize);
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (429, json!("notice_limit")), "{v}");
    assert_eq!(sent(&t) - n, store::NOTICES_PER_DEST_PER_DAY as usize, "not mailed again");
}

/// Review finding 15: a same-box loop is refused.
#[test]
fn a_same_box_loop_is_refused() {
    let _t = t("fwloop");
    seed_mailbox("b@notify.brand.test", "direct");
    let (st, _) = post(FORWARD_PATH, json!({ "address": MB, "to": "b@notify.brand.test" }));
    assert_eq!(st, 200);
    let (st, v) = post(FORWARD_PATH, json!({ "address": "b@notify.brand.test", "to": MB }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("forward_loop")), "{v}");
}

/// FW-A16: no system sender → the notice comes
/// A GET never stops (mail scanners); only the POST does, one destination,
/// and the owner hears. Stopped stays stopped.
#[test]
fn the_stop_button_is_a_post_and_stopped_means_stopped() {
    let t = t("fwstop");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    let stop = last_token(&t, "k2es_");
    let _ = crate::mail::forward_outcomes::take_notices();
    let p = vpages::page_get(STOP_PATH, &format!("t={stop}"));
    assert!(p.status.starts_with("200") && p.html.contains("<form method=\"post\"") && p.html.contains("<button"), "{p:?}");
    assert_eq!(p.html.matches("<button").count(), 1, "one button");
    assert!(t.io.forward(ACC).is_some(), "a GET stops nothing");
    assert_eq!(show()["forward"]["state"], "active");
    assert!(crate::mail::forward_outcomes::take_notices().is_empty());
    let p = vpages::page_post(STOP_PATH, "", format!("t={stop}").as_bytes());
    assert!(p.html.contains("Stopped"), "{p:?}");
    assert_eq!(t.io.forward(ACC), None, "stopped = keep-only");
    let s = show();
    assert_eq!((s["forward"]["state"].clone(), s["forward"]["pauseReason"].clone()), (json!("paused"), json!("revoked")), "{s}");
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].0.contains(OUT) && n[0].0.contains(MB) && n[0].1.contains("--restart-stopped"), "{n:?}");
    // Nobody resumes it, not even the owner.
    let (st, v) = post(RESUME_PATH, json!({ "address": MB }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("destination_stopped")), "{v}");
    // set and add don't restart it, for anyone, and mail nobody.
    let n = sent(&t);
    for path in [FORWARD_PATH, ADD_PATH] {
        let (st, v) = post(path, json!({ "address": MB, "to": OUT }));
        assert_eq!((st, v["error"]["code"].clone()), (409, json!("destination_stopped")), "{path}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap_or("").contains("--restart-stopped"), "{v}");
    }
    assert_eq!(sent(&t), n);
    assert_eq!(t.io.forward(ACC), None);
    // Removing the row doesn't forget the stop.
    post(UNSET_PATH, json!({ "address": MB }));
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("destination_stopped")), "a removed row's stop still holds: {v}");
    // An agent can't override it, flag or no flag.
    let (st, v) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": OUT, "restartStopped": true })));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    assert_eq!(t.io.forward(ACC), None);
    // The owner, explicitly: it comes back with a new notice, audited.
    let _ = crate::mail::forward_outcomes::take_notices();
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT, "restartStopped": true }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    assert!(t.io.forward(ACC).is_some());
    assert_eq!(sent(&t), n + 1);
    assert!(show()["events"].as_array().expect("events").iter().any(|e| e["event"] == "restarted"));
    let n = crate::mail::forward_outcomes::take_notices();
    assert!(n[0].1.contains("after the address pressed Stop"), "{n:?}");
    // And the very first link still stops it.
    vpages::page_post(STOP_PATH, "", format!("t={stop}").as_bytes());
    assert_eq!(t.io.forward(ACC), None, "a stop link works for good");
}

/// The owner's notice also goes by email when the owner has a confirmed
/// notice address.
#[test]
fn the_owner_also_gets_email_when_they_have_a_notice_address() {
    let t = t("fwowner");
    system_sender_on();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let i = k2_core::email_verify::issue(&conn, k2_core::email_verify::PURPOSE_PERSON_NOTICE, "owner", "boss@elsewhere.test", "owner", 1).expect("issue");
        k2_core::email_verify::confirm(&conn, &i.token, 2).expect("confirm");
    }
    let (st, _) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": OUT })));
    assert_eq!(st, 200);
    let mails = t.f.engine.sent.lock().unwrap().clone();
    assert_eq!(mails.len(), 2, "the address's notice and the owner's");
    assert_eq!(mails[0].rcpt, vec![OUT.to_string()]);
    assert_eq!(mails[1].rcpt, vec!["boss@elsewhere.test".to_string()]);
    let text = mails[1].email.to_string();
    assert!(text.contains(MB) && text.contains(OUT) && text.contains("the AI agent"), "names mailbox, address, who: {text}");
    let n = crate::mail::forward_outcomes::take_notices();
    assert!(n.iter().any(|(_, b, _)| b.contains("the AI agent")), "and the Inbox: {n:?}");
}

/// Several destinations: each its own state and stop link.
#[test]
fn several_destinations_stand_alone() {
    let t = t("fwmany");
    system_sender_on();
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": ["a@outside.test", "b@outside.test", "A@Outside.Test"] }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["destinations"].as_array().map(Vec::len), Some(2), "named twice, one row: {v}");
    assert_eq!(sent(&t), 2, "one notice each");
    let stop_b = last_token(&t, "k2es_");
    let fwd = t.io.forward(ACC).expect("written");
    assert_eq!((fwd.dest.as_str(), fwd.more.clone()), ("a@outside.test", vec!["b@outside.test".to_string()]));
    assert_eq!(*t.io.limit_calls.borrow(), 1, "the mail server's limit was raised first");
    // The stop link stops its own destination only.
    vpages::page_post(STOP_PATH, "", format!("t={stop_b}").as_bytes());
    let rows = forwards();
    assert_eq!((one(&rows, "a@outside.test")["state"].clone(), one(&rows, "b@outside.test")["pauseReason"].clone()), (json!("active"), json!("revoked")), "{rows:?}");
    let fwd = t.io.forward(ACC).expect("a still forwards");
    assert_eq!((fwd.dest.as_str(), fwd.more.len()), ("a@outside.test", 0));
    // set replaces the list; a stopped one in it refuses the whole call.
    let n = sent(&t);
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": ["c@outside.test", "b@outside.test"] }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("destination_stopped")), "{v}");
    assert_eq!(sent(&t), n, "nobody was mailed");
    assert_eq!(forwards().len(), 2, "nothing changed");
}

#[test]
fn add_and_rm_touch_only_what_they_name() {
    let t = t("fwaddrm");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": "a@outside.test" }));
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "b@outside.test" }));
    assert_eq!((st, v["state"].clone(), v["to"].clone()), (200, json!("active"), json!("b@outside.test")), "{v}");
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!((st, v["kind"].clone()), (200, json!("internal")), "{v}");
    assert_eq!(sent(&t), 2, "a same-box destination gets no notice");
    let fwd = t.io.forward(ACC).expect("written");
    assert_eq!(fwd.same_box, vec!["team@notify.brand.test".to_string()]);
    assert_eq!(fwd.more, vec!["b@outside.test".to_string()]);
    let id_a = one(&forwards(), "a@outside.test")["id"].clone();
    // Adding one already there: same row, no second notice.
    let (st, _) = post(ADD_PATH, json!({ "address": MB, "to": "a@outside.test" }));
    assert_eq!((st, sent(&t), one(&forwards(), "a@outside.test")["id"].clone()), (200, 2, id_a.clone()));
    let _ = crate::mail::forward_outcomes::take_notices();
    let (st, v) = post(RM_PATH, json!({ "address": MB, "to": "b@outside.test" }));
    assert_eq!((st, v["removed"].clone(), v["remaining"].as_array().map(Vec::len)), (200, json!(["b@outside.test"]), Some(2)), "{v}");
    assert!(crate::mail::forward_outcomes::take_notices()[0].1.contains("No longer forwarding to: b@outside.test"));
    let rows = forwards();
    assert_eq!(one(&rows, "a@outside.test")["id"], id_a, "the others are untouched");
    assert!(!t.io.src(ACC).expect("script").contains("b@outside.test"));
    // rm of one that isn't there, or with no `to`, changes nothing.
    let (st, v) = post(RM_PATH, json!({ "address": MB, "to": ["a@outside.test", "nope@outside.test"] }));
    assert_eq!((st, v["error"]["code"].clone()), (404, json!("not_found")), "{v}");
    assert_eq!(forwards().len(), 2);
    let (st, _) = post(RM_PATH, json!({ "address": MB }));
    assert_eq!(st, 400);
    // A single-destination set still replaces everything.
    let (st, _) = post(FORWARD_PATH, json!({ "address": MB, "to": "a@outside.test" }));
    assert_eq!(st, 200);
    assert_eq!(forwards().len(), 1);
    assert_eq!(forwards()[0]["id"], id_a, "the one it names keeps its row");
    // unset takes them all.
    post(ADD_PATH, json!({ "address": MB, "to": "b@outside.test" }));
    let (st, v) = post(UNSET_PATH, json!({ "address": MB }));
    assert_eq!((st, v["count"].clone()), (200, json!(2)), "{v}");
    assert_eq!(t.io.forward(ACC), None);
}

#[test]
fn pause_and_resume_take_one_destination_or_all() {
    let t = t("fwpause");
    system_sender_on();
    post(FORWARD_PATH, json!({ "address": MB, "to": ["a@outside.test", "b@outside.test"] }));
    let (st, v) = post(PAUSE_PATH, json!({ "address": MB, "to": "b@outside.test" }));
    assert_eq!((st, v["state"].clone(), v["pauseReason"].clone()), (200, json!("paused"), json!("manual")), "{v}");
    assert_eq!(t.io.forward(ACC).expect("a").more.len(), 0);
    let (st, v) = post(RESUME_PATH, json!({ "address": MB, "to": "b@outside.test" }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    let (st, v) = as_agent("ws-fw", || post(PAUSE_PATH, json!({ "address": MB })));
    assert_eq!((st, v["forwards"].as_array().map(Vec::len)), (200, Some(2)), "{v}");
    assert_eq!(t.io.forward(ACC), None);
    // Resume with no --to: what this caller may, and it says what it left.
    let id_b = one(&forwards(), "b@outside.test")["id"].as_str().expect("id").to_string();
    crate::mail::forward_sync::pause(&id_b, store::P_DEST_GONE, "k2", None).expect("reason");
    let (st, v) = as_agent("ws-fw", || post(RESUME_PATH, json!({ "address": MB })));
    assert_eq!((st, v["forwards"][0]["to"].clone(), v["skipped"][0]["to"].clone()), (200, json!("a@outside.test"), json!("b@outside.test")), "{v}");
    assert_eq!(v["skipped"][0]["code"], "owner_only", "{v}");
    let (st, _) = post(PAUSE_PATH, json!({ "address": MB, "to": "nope@outside.test" }));
    assert_eq!(st, 404);
}

#[test]
fn the_destination_cap_refuses_clearly() {
    let t = t("fwcap");
    system_sender_on();
    let many: Vec<String> = (0..store::MAX_DESTINATIONS + 1).map(|i| format!("d{i}@outside.test")).collect();
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": many }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("too_many_destinations")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains(&format!("at most {}", store::MAX_DESTINATIONS)), "{v}");
    assert_eq!((sent(&t), forwards().len()), (0, 0), "nobody mailed, nothing changed");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": many[..store::MAX_DESTINATIONS].to_vec() }));
    assert_eq!(st, 200, "{v}");
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "one-more@outside.test" }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("too_many_destinations")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("forward rm"), "{v}");
    assert_eq!(forwards().len(), store::MAX_DESTINATIONS);
    // set may swap the whole list at the cap.
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "only@outside.test" }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(forwards().len(), 1);
}

/// The box's daily limit on new outside addresses: refused clearly, across
/// mailboxes; a same-box destination and an address already on don't count.
#[test]
fn new_outside_addresses_are_capped_per_box_per_day() {
    let t = t("fwday");
    system_sender_on();
    // Across mailboxes: five destinations each, up to the day's limit.
    let mut added = 0;
    for m in 0.. {
        let mailbox = format!("day{m}@notify.brand.test");
        seed_mailbox(&mailbox, "direct");
        for d in 0..store::MAX_DESTINATIONS {
            if added < store::NEW_OUTSIDE_PER_DAY {
                let (st, v) = post(ADD_PATH, json!({ "address": mailbox, "to": format!("m{m}d{d}@outside.test") }));
                assert_eq!(st, 200, "{added}: {v}");
                added += 1;
            }
        }
        if added >= store::NEW_OUTSIDE_PER_DAY {
            break;
        }
    }
    let n = sent(&t);
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "too-many@outside.test" }));
    assert_eq!((st, v["error"]["code"].clone()), (429, json!("new_outside_limit")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains(&format!("at most {} new outside addresses a day", store::NEW_OUTSIDE_PER_DAY)), "{v}");
    assert_eq!(sent(&t), n, "nobody was mailed");
    // A list that would cross it refuses whole.
    assert_eq!(forwards().iter().filter(|f| f["to"] == "too-many@outside.test").count(), 0);
    // A same-box destination still works, and so does keeping one already on.
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!(st, 200, "{v}");
    // Past the window it opens again.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE mail_forward_events SET at = at - 90000", []).expect("age");
    }
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "too-many@outside.test" }));
    assert_eq!(st, 200, "{v}");
}

fn seed_server() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO mail_server (id, status, pinned_version, hostname, updated_at) VALUES (1, 'running', '0.16.25', 'mail.brand.test', 1)",
        [],
    )
    .expect("server");
}

fn set_cap(n: i64) -> (u16, J) {
    let r = handle_forward_cap(json!({ "perDay": n }).to_string().as_bytes());
    (status(&r), body(&r))
}

/// `n` new outside addresses on fresh mailboxes (five each), from `from`.
fn add_many(from: i64, n: i64) -> Vec<(u16, J)> {
    (from..from + n)
        .map(|i| {
            let mailbox = format!("bulk{}@notify.brand.test", i / store::MAX_DESTINATIONS as i64);
            let known = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                crate::mail::addresses::load_address(&conn, &mailbox).is_some()
            };
            if !known {
                seed_mailbox(&mailbox, "direct");
            }
            post(ADD_PATH, json!({ "address": mailbox, "to": format!("d{i}@outside.test") }))
        })
        .collect()
}

/// The owner may set it: the default holds until they do, a raised cap lets
/// more through, and the ceiling refuses with a clear line.
#[test]
fn the_daily_cap_is_the_owner_s_to_raise_and_has_a_ceiling() {
    let _t = t("fwcapset");
    system_sender_on();
    seed_server();
    // The default.
    let got = add_many(0, store::NEW_OUTSIDE_PER_DAY);
    assert!(got.iter().all(|(st, _)| *st == 200), "{got:?}");
    let (st, v) = add_many(store::NEW_OUTSIDE_PER_DAY, 1).pop().expect("one");
    assert_eq!((st, v["error"]["code"].clone()), (429, json!("new_outside_limit")), "the default holds: {v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("--forward-new-per-day"), "{v}");
    // Above the ceiling: refused, nothing changed.
    let (st, v) = set_cap(store::NEW_OUTSIDE_PER_DAY_MAX + 1);
    assert_eq!((st, v["error"]["code"].clone()), (400, json!("usage")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains(&format!("at most {}", store::NEW_OUTSIDE_PER_DAY_MAX)), "{v}");
    let (st, _) = set_cap(-1);
    assert_eq!(st, 400);
    let (st, v) = add_many(store::NEW_OUTSIDE_PER_DAY, 1).pop().expect("one");
    assert_eq!(st, 429, "still the default: {v}");
    // Raised.
    let (st, v) = set_cap(25);
    assert_eq!((st, v["perDay"].clone(), v["default"].clone()), (200, json!(25), json!(store::NEW_OUTSIDE_PER_DAY)), "{v}");
    let got = add_many(store::NEW_OUTSIDE_PER_DAY + 1, 5);
    assert!(got.iter().all(|(st, _)| *st == 200), "{got:?}");
    let (st, v) = add_many(store::NEW_OUTSIDE_PER_DAY + 6, 1).pop().expect("one");
    assert_eq!(st, 429, "the new number holds: {v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("at most 25"), "{v}");
    // config shows it, and the per-mailbox limit hasn't moved.
    let cfg: J = serde_json::from_str(&crate::mail::routes_server::handle_config_get(&HashMap::new()).body).expect("json");
    assert_eq!(cfg["forwarding"]["newOutsidePerDay"], 25, "{cfg}");
    assert_eq!(cfg["forwarding"]["maxDestinationsPerMailbox"], 5, "{cfg}");
    let many: Vec<String> = (0..6).map(|i| format!("x{i}@outside.test")).collect();
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": many }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("too_many_destinations")), "{v}");
}

/// An agent can't loosen its own limit, with mail-manage or without: the
/// handler refuses it, the route's floor is the owner's (not an Admin's), and
/// the refusal isn't answered by asking for mail.manage.
#[test]
fn an_agent_can_t_raise_the_daily_cap() {
    let _t = t("fwcapagent");
    seed_server();
    let r = as_agent("ws-fw", || handle_forward_cap(json!({ "perDay": 1000 }).to_string().as_bytes()));
    assert_eq!(status(&r), 403, "{}", r.body);
    assert_eq!(body(&r)["error"]["code"], "owner_only");
    let cap = db(|c| store::new_outside_cap(c)).ok().expect("cap");
    assert_eq!(cap, store::NEW_OUTSIDE_PER_DAY, "unchanged");
    // A mail-manage passport at the extra gate: refused, whatever its toggle.
    let principal = crate::session_token::HookPrincipal { workspace_uuid: "ws-fw".into(), agent_address: "agent".into() };
    assert!(crate::mail_routes::mail_manage_authorized(CAP_PATH, false, Some(&principal)).is_err());
    assert!(!crate::mail_routes::is_mail_manage_surface(CAP_PATH), "mail-manage doesn't open it");
    // The owner's floor: an Admin login doesn't pass.
    let row = crate::routes::route_policy::lookup(CAP_PATH).expect("in the table");
    assert!(row.post.is_some_and(|m| !m.admits(k2_core::connect_users::Role::Admin) && m.admits(k2_core::connect_users::Role::Owner)), "{row:?}");
    // GET is refused.
    assert_eq!(crate::mail_routes::dispatch(CAP_PATH, &HashMap::new()).expect("claimed").status, "405 Method Not Allowed");
}

/// The guards hold at a raised cap: every address gets its notice with a stop
/// link, the owner hears, stop works, and refusals still pause.
#[test]
fn the_guards_fire_at_a_raised_cap() {
    let t = t("fwcapguard");
    system_sender_on();
    seed_server();
    let (st, _) = set_cap(store::NEW_OUTSIDE_PER_DAY_MAX);
    assert_eq!(st, 200);
    let _ = crate::mail::forward_outcomes::take_notices();
    let total = store::NEW_OUTSIDE_PER_DAY + 5;
    let got = add_many(0, total);
    assert!(got.iter().all(|(st, v)| *st == 200 && v["state"] == "active"), "{got:?}");
    let mails = t.f.engine.sent.lock().unwrap().clone();
    assert_eq!(mails.len() as i64, total, "one notice each");
    assert!(mails.iter().all(|m| m.email.to_string().contains("k2es_")), "each with a stop link");
    // The owner hears the first few one by one; the rest wait for the summary.
    assert_eq!(crate::mail::forward_outcomes::take_notices().len() as i64, store::OWNER_NOTES_PER_HOUR);
    // The last address presses Stop: stops have their own slots, so the owner
    // hears now, and a change doesn't use them up.
    let stop = last_token(&t, "k2es_");
    vpages::page_post(STOP_PATH, "", format!("t={stop}").as_bytes());
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].0.contains(&format!("d{}@outside.test", total - 1)), "{n:?}");
    forward_verify::flush_owner_summary(email_notify::now() + store::OWNER_SUMMARY_SECS);
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "one summary: {n:?}");
    let want = total - store::OWNER_NOTES_PER_HOUR;
    assert!(n[0].0.contains(&want.to_string()) && n[0].1.contains(&format!("{want} more changes")), "the count: {n:?}");
    assert!(n[0].1.contains(&format!("d{}@outside.test", total - 1)), "every new address by name: {}", n[0].1);
    assert!(n[0].1.contains("k2 hostmail forward list"), "how to list them: {}", n[0].1);
    // Every change still has its audit line, rolled up or not.
    let audit = k2_core::auth_audit::read_all().expect("audit");
    let lines = audit.iter().filter(|e| e.to_string().contains("mail-forward-changed") && e.to_string().contains("bulk")).count() as i64;
    assert!(lines >= total, "{lines} audit lines for {total} changes");
    // Three no-such-user refusals pause that destination.
    let (mailbox, dest) = ("bulk0@notify.brand.test", "d0@outside.test");
    let at = email_notify::now();
    let batch: Vec<J> = (1..=3u64)
        .flat_map(|q| {
            [
                json!({"id": format!("q{q}"), "type": "queue.autogenerated-queued", "data": {"queueId": q, "from": mailbox, "to": [dest]}}),
                json!({"id": format!("p{q}"), "type": "delivery.dsn-perm-fail", "data": {"queueId": q, "to": dest, "details": "Code: 550, Enhanced code: 5.1.1, Message: no such user"}}),
            ]
        })
        .collect();
    let events = crate::email_notify::webhook::parse_events(json!({ "events": batch }).to_string().as_bytes()).expect("events");
    let applied = crate::mail::forward_outcomes::apply(&events, at);
    assert_eq!(applied.paused.len(), 1, "{applied:?}");
    let live = db(|c| store::live_for_pair(c, mailbox, dest)).ok().flatten().expect("row");
    assert_eq!(live.pause_reason.as_deref(), Some(store::P_DEST_GONE));
    assert_eq!(crate::mail::forward_outcomes::take_notices().len(), 1, "the owner hears");
}

/// Past the hour's burst a new outside forward waits its turn: not on, no
/// mail, nothing written; the pass emails it first, then turns it on, at the
/// pace. It counts against the cap, and asking again doesn't queue it twice.
#[test]
fn past_the_burst_new_forwards_wait_their_turn_with_the_notice_first() {
    let t = t("fwpace");
    system_sender_on();
    seed_server();
    set_cap(store::NEW_OUTSIDE_PER_DAY_MAX);
    let burst = store::NOTICES_BURST_PER_HOUR;
    let got = add_many(0, burst);
    assert!(got.iter().all(|(st, v)| *st == 200 && v["state"] == "active"), "{got:?}");
    assert_eq!(sent(&t) as i64, burst);
    let _ = crate::mail::forward_outcomes::take_notices();
    // The next ones wait.
    let got = add_many(burst, 3);
    assert!(got.iter().all(|(st, v)| *st == 200 && v["state"] == "pending_verify"), "{got:?}");
    assert!(got[0].1["message"].as_str().unwrap_or("").contains("isn't on yet"), "{:?}", got[0].1);
    assert_eq!(sent(&t) as i64, burst, "no mail yet");
    let waiting_mailbox = got[0].1["address"].as_str().expect("address").to_string();
    assert!(crate::mail::forward_outcomes::take_notices().is_empty(), "these are past the hour's first few");
    // Nothing goes there yet.
    assert!(!t.io.src(&format!("acc-{}", waiting_mailbox.split('@').next().expect("local"))).unwrap_or_default().contains(&format!("d{burst}@outside.test")));
    // They count against the cap, and asking again doesn't queue a second row.
    let used = db(|c| store::new_outside_since(c, 0)).ok().expect("count");
    assert_eq!(used, burst + 3);
    let (st, v) = post(ADD_PATH, json!({ "address": waiting_mailbox, "to": format!("d{burst}@outside.test") }));
    assert_eq!((st, v["state"].clone()), (200, json!("pending_verify")), "{v}");
    assert_eq!(db(|c| store::new_outside_since(c, 0)).ok().expect("count"), burst + 3);
    // listed in words.
    let only_waiting: HashMap<String, String> = [("state".to_string(), "pending_verify".to_string())].into();
    let list = body(&handle_forwards_list(&only_waiting));
    assert!(list["forwards"][0]["noticeWords"].as_str().unwrap_or("").contains("pacing"), "{list}");
    // The pass, at the pace: the notice first, then on.
    let now = email_notify::now();
    let pass = forward_verify::minute_pass(now);
    assert_eq!(pass.turned_on, store::NOTICES_PER_PASS, "{pass:?}");
    assert_eq!(sent(&t) as i64, burst + store::NOTICES_PER_PASS as i64);
    let pass = forward_verify::minute_pass(now + 60);
    assert_eq!(pass.turned_on, 1, "{pass:?}");
    assert_eq!(db(|c| store::due_waiting(c)).ok().expect("waiting"), 0);
    let row = db(|c| store::live_for_pair(c, &waiting_mailbox, &format!("d{burst}@outside.test"))).ok().flatten().expect("row");
    assert_eq!((row.state.as_str(), row.notice_state.as_deref()), (store::ST_ACTIVE, Some(store::N_SENT)));
    let events = db(|c| store::events_for_address(c, &row.address_id, 50)).ok().expect("events");
    assert!(!events.iter().any(|e| e.event == store::E_LEGACY_ON), "not a row from before opt-out");
    assert_eq!(db(|c| store::new_outside_since(c, 0)).ok().expect("count"), burst + 3, "still counted");
    // While the hour's notices stay spent, the next one waits too.
    let got = add_many(burst + 3, 1);
    assert_eq!(got[0].1["state"], "pending_verify", "the hour's burst is still spent");
}

/// The hour's first few owner notices say a queued address isn't told yet.
#[test]
fn the_owner_is_told_a_queued_address_isn_t_on_yet() {
    let t = t("fwpacenote");
    system_sender_on();
    seed_server();
    set_cap(store::NEW_OUTSIDE_PER_DAY_MAX);
    let _ = crate::mail::forward_outcomes::take_notices();
    // The hour's emails are spent, but not the owner's notes.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for i in 0..store::NOTICES_BURST_PER_HOUR {
            store::record_event(&conn, &format!("old{i}"), store::E_NOTICE_SENT, "k2", &json!({}), email_notify::now()).expect("event");
        }
    }
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["state"].clone()), (200, json!("pending_verify")), "{v}");
    assert_eq!(sent(&t), 0);
    let n = crate::mail::forward_outcomes::take_notices();
    assert!(n[0].1.contains("K2 is pacing") && !n[0].1.contains("K2 emailed"), "{n:?}");
}

/// A row from before this that can't arrive isn't changed: the owner is told
/// once, `forward list` names it, and it runs on until its bounces pause it.
#[test]
fn a_row_that_can_t_arrive_is_listed_not_changed() {
    let t = t("fwcant");
    system_sender_on();
    // Set when the domain wasn't this box's; K2 hosts it now.
    post(FORWARD_PATH, json!({ "address": MB, "to": "bob@later.test" }));
    assert_eq!(forwards()[0]["cantArrive"], J::Null);
    let _ = crate::mail::forward_outcomes::take_notices();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO mail_domains (id, domain, send_mode, status, created_at) VALUES ('later', 'later.test', 'direct', 'verified', 1)", []).expect("domain");
    }
    let ctx = crate::mail::forward_sync::ctx();
    let r = crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert_eq!(r.failed, 0, "{r:?}");
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains(&format!("{MB} to bob@later.test")) && n[0].1.contains("k2 hostmail forward list"), "{}", n[0].1);
    // Not changed: still active, still written.
    let row = forwards()[0].clone();
    assert_eq!(row["state"], "active", "{row}");
    assert!(t.io.src(ACC).expect("script").contains("bob@later.test"));
    assert!(row["cantArrive"].as_str().unwrap_or("").contains("can't arrive"), "{row}");
    let list = body(&handle_forwards_list(&HashMap::new()));
    assert!(list["forwards"][0]["cantArrive"].as_str().unwrap_or("").contains("forward rm"), "{list}");
    // Told once.
    crate::mail::forward_sync::reconcile_with(t.io.as_ref(), false, &ctx);
    assert!(crate::mail::forward_outcomes::take_notices().is_empty());
    // A new add of such an address is refused.
    let (st, _) = post(ADD_PATH, json!({ "address": MB, "to": "other@later.test" }));
    assert_eq!(st, 400);
}

/// FW-A16: no system sender → the notice comes from the mailbox itself,
/// by delegation, with no stored copy.
#[test]
fn with_no_system_sender_the_notice_comes_from_the_mailbox() {
    let t = t("fwself");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT, "keep": false }));
    assert_eq!((st, v["sentVia"].clone(), v["keep"].clone()), (200, json!("mailbox"), json!(false)), "{v}");
    let mail = t.f.engine.sent.lock().unwrap().last().cloned().expect("notice");
    assert_eq!((mail.account.as_str(), mail.from.as_str(), mail.destroy), (ACC, MB, true));
    assert_eq!(mail.rcpt, vec![OUT.to_string()]);
}

/// Rosson 2026-11-02: only a domain's MX pointing elsewhere makes one of this
/// box's own addresses outside.
#[test]
fn a_hosted_domain_whose_mx_points_elsewhere_counts_as_outside() {
    let t = t("fwmx");
    system_sender_on();
    seed_mailbox("team@notify.brand.test", "direct");
    // No MX answer, or the MX is this box: same-box, no notice, no guard.
    let (_, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!(v["kind"], "internal", "{v}");
    set_test_mx("notify.brand.test", &[&format!("{HOST}.")]);
    let (_, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!(v["kind"], "internal", "the box's own MX, trailing dot and all: {v}");
    assert_eq!(sent(&t), 0);
    // One of several MX hosts being this box still counts as here.
    set_test_mx("notify.brand.test", &["mx.elsewhere.test", HOST]);
    let (_, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!(v["kind"], "internal", "{v}");
    // The MX points elsewhere: outside, with a notice, the guard and a note.
    set_test_mx("notify.brand.test", &["mx.elsewhere.test"]);
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!((st, v["kind"].clone(), v["state"].clone()), (200, json!("external"), json!("active")), "{v}");
    assert_eq!(sent(&t), 1, "told");
    assert!(t.io.forward(ACC).expect("written").guard.is_some());
    assert!(v["note"].as_str().unwrap_or("").contains("goes to the mailbox on this server, not to the other provider"), "{v}");
    // An address with no mailbox here: refused, MX or no MX, because the mail
    // server delivers its own domains itself and it would bounce (probe X12).
    let n = sent(&t);
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "ghost@notify.brand.test" }));
    assert_eq!((st, v["error"]["code"].clone()), (400, json!("usage")), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap_or("").to_string();
    assert!(hint.contains("this server hosts notify.brand.test") && hint.contains("never reach another provider"), "{hint}");
    assert_eq!((sent(&t), forwards().len()), (n, 1), "nobody mailed, nothing changed");
    // Air-gap: no lookup, and K2 won't guess.
    crate::push_routes::set_test_airgap(true);
    let (st, v) = post(UNSET_PATH, json!({ "address": MB }));
    assert_eq!(st, 200, "{v}");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    crate::push_routes::set_test_airgap(false);
    assert_eq!((st, v["kind"].clone()), (200, json!("internal")), "{v}");
}

fn seed_pending(mailbox: &str, dest: &str, age_secs: i64, by: &str) -> (String, k2_core::email_verify::Issued) {
    let at = email_notify::now() - age_secs;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let aid = format!("ma-{mailbox}");
    let i = k2_core::email_verify::issue(&conn, k2_core::email_verify::PURPOSE_FORWARD, mailbox, dest, by, at).expect("issue");
    let f = store::add(
        &conn,
        &store::NewForward {
            address_id: &aid,
            address: mailbox,
            dest,
            dest_kind: store::KIND_EXTERNAL,
            keep: true,
            set_by: by,
            state: store::ST_PENDING_VERIFY,
            verified_address_id: Some(&i.row.id),
        },
        at,
    )
    .expect("row");
    (f.id, i)
}

/// On update: a row set within the window goes on with its notice, an old one
/// stays off and is listed, the owner gets one notice, and neither counts
/// against the daily cap.
#[test]
fn rows_from_before_opt_out_go_on_when_recent_and_stay_off_when_old() {
    let t = t("fwlegacy");
    system_sender_on();
    let (fresh, _) = seed_pending(MB, "fresh@outside.test", 6 * 86_400, "owner");
    let (old, _) = seed_pending(MB, "old@outside.test", store::LEGACY_ON_WINDOW_SECS + 60, "owner");
    assert_eq!(t.io.forward(ACC), None);
    let _ = crate::mail::forward_outcomes::take_notices();
    let now = email_notify::now();
    let pass = forward_verify::minute_pass(now);
    assert_eq!((pass.turned_on, pass.left_off), (1, 1), "{pass:?}");
    let rows = forwards();
    let (f, o) = (one(&rows, "fresh@outside.test"), one(&rows, "old@outside.test"));
    assert_eq!((f["state"].clone(), f["notice"].clone()), (json!("active"), json!("sent")), "{f}");
    assert_eq!((o["state"].clone(), o["notice"].clone()), (json!("pending_verify"), json!("stale")), "{o}");
    assert!(o["noticeWords"].as_str().unwrap_or("").contains("set it again"), "{o}");
    // Only the recent one is written and told.
    let src = t.io.src(ACC).expect("script");
    assert!(src.contains("fresh@outside.test") && !src.contains("old@outside.test"), "{src}");
    assert_eq!(sent(&t), 1);
    assert_eq!(t.f.engine.sent.lock().unwrap()[0].rcpt, vec!["fresh@outside.test".to_string()]);
    // One notice for the owner, naming both.
    let n = crate::mail::forward_outcomes::take_notices();
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.contains("fresh@outside.test") && n[0].1.contains("old@outside.test") && n[0].1.contains("because they're old"), "{}", n[0].1);
    // A later pass changes nothing, says nothing, and the old one stays off.
    assert_eq!(forward_verify::minute_pass(now + 60), forward_verify::Pass::default());
    assert!(crate::mail::forward_outcomes::take_notices().is_empty());
    assert_eq!(one(&forwards(), "old@outside.test")["state"], "pending_verify");
    assert_eq!(sent(&t), 1);
    // Neither counted against the cap.
    let db_count = db(|c| store::new_outside_since(c, 0)).ok().expect("count");
    assert_eq!(db_count, 0);
    // The event says what happened to the row.
    let (ids, _) = (fresh, old);
    assert!(show()["events"].as_array().expect("events").iter().any(|e| e["event"] == "legacy_on" && e["forwardId"] == json!(ids)));
    // Setting the old one again turns it on, as a new add.
    let (st, v) = post(ADD_PATH, json!({ "address": MB, "to": "old@outside.test" }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    assert_eq!(sent(&t), 2);
}

/// Many rows: paced, no stall, no burst, and past the daily cap.
#[test]
fn a_box_with_many_waiting_rows_turns_them_on_paced() {
    let t = t("fwpaced");
    system_sender_on();
    // Before this release a mailbox had one forward, so one row each.
    let total = store::NEW_OUTSIDE_PER_DAY as usize + 3;
    // Oldest first, spread over hours: confirm mails were limited per hour.
    for m in (0..total).rev() {
        let mailbox = format!("m{m}@notify.brand.test");
        seed_mailbox(&mailbox, "direct");
        seed_pending(&mailbox, &format!("w{m}@outside.test"), 3600 + 2 * 3600 * (m as i64 / 5), "owner");
    }
    let now = email_notify::now();
    let mut on = 0;
    let mut passes = 0;
    while on < total {
        let before = sent(&t);
        let pass = forward_verify::minute_pass(now + passes * 60);
        assert_eq!(pass.turned_on, (total - on).min(store::NOTICES_PER_PASS), "paced: {pass:?}");
        assert_eq!(sent(&t) - before, pass.turned_on, "one notice each");
        on += pass.turned_on;
        passes += 1;
        assert!(passes < 100, "stalled");
    }
    assert_eq!(on, total, "past the {}-a-day cap", store::NEW_OUTSIDE_PER_DAY);
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!(st, 200, "they didn't spend the day's cap: {v}");
}

/// A stopped address is never turned on; air-gap holds everything back.
#[test]
fn a_stopped_row_stays_off_and_air_gap_holds_the_pass() {
    let t = t("fwlegstop");
    system_sender_on();
    let (id, i) = seed_pending(MB, OUT, 3600, "owner");
    crate::push_routes::set_test_airgap(true);
    assert_eq!(forward_verify::minute_pass(email_notify::now()), forward_verify::Pass::default());
    crate::push_routes::set_test_airgap(false);
    assert_eq!(sent(&t), 0);
    // The old email's stop link, pressed before K2 got to it.
    vpages::page_post(STOP_PATH, "", format!("t={}", i.stop).as_bytes());
    assert_eq!(forward_verify::minute_pass(email_notify::now()).turned_on, 0);
    assert_eq!(sent(&t), 0, "no notice to someone who stopped");
    assert_eq!(t.io.forward(ACC), None);
    let row = db(|c| store::get(c, &id)).ok().flatten().expect("row");
    assert_eq!(row.state, store::ST_REMOVED);
}

/// A confirm link from before opt-out: says what's happening, confirms
/// nothing, and offers Stop. Only the POST with Stop stops.
#[test]
fn an_old_confirm_link_says_it_s_on_and_offers_stop() {
    let t = t("fwoldlink");
    system_sender_on();
    let (_, i) = seed_pending(MB, OUT, 3600, "owner");
    forward_verify::minute_pass(email_notify::now());
    let q = format!("t={}", i.token);
    let p = vpages::page_get(VERIFY_PATH, &q);
    assert!(p.status.starts_with("200") && p.html.contains("is sending") && p.html.contains("<button"), "{p:?}");
    assert!(p.html.contains("value=\"stop\""), "{p:?}");
    assert!(!p.html.contains("Confirm</button>"), "nothing to confirm: {p:?}");
    assert!(t.io.forward(ACC).is_some());
    // An old open page posting the confirm: same page, nothing changes.
    let p = vpages::page_post(VERIFY_PATH, "", q.as_bytes());
    assert!(p.html.contains("is sending"), "{p:?}");
    assert!(t.io.forward(ACC).is_some());
    // Stop from it.
    let _ = crate::mail::forward_outcomes::take_notices();
    let p = vpages::page_post(VERIFY_PATH, "", format!("{q}&do=stop").as_bytes());
    assert!(p.html.contains("Stopped"), "{p:?}");
    assert_eq!(t.io.forward(ACC), None);
    assert_eq!(crate::mail::forward_outcomes::take_notices().len(), 1, "the owner hears");
    let (st, _) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!(st, 409, "stopped means stopped");
    // A link K2 can't place says what to do.
    let p = vpages::page_get(VERIFY_PATH, &format!("t=k2ev_{}", "a".repeat(43)));
    assert!(p.status.starts_with("404") && p.html.contains("no longer asks for a confirm"), "{p:?}");
}

/// A stop link whose request had expired before opt-out still offers Stop, and
/// the press counts.
#[test]
fn an_expired_request_s_stop_link_still_records_the_stop() {
    let t = t("fwexpired");
    system_sender_on();
    let i = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let i = k2_core::email_verify::issue(&conn, k2_core::email_verify::PURPOSE_FORWARD, MB, OUT, "owner", email_notify::now() - 2 * k2_core::email_verify::LINK_SECS).expect("issue");
        k2_core::email_verify::expire_due(&conn, email_notify::now()).expect("expire");
        i
    };
    let q = format!("t={}", i.stop);
    let p = vpages::page_get(STOP_PATH, &q);
    assert!(p.html.contains("<button"), "offers Stop: {p:?}");
    vpages::page_post(STOP_PATH, "", q.as_bytes());
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("destination_stopped")), "{v}");
    assert_eq!(sent(&t), 0);
}

/// A person notice address keeps its confirm: the POST confirms, with no Stop form.
#[test]
fn a_person_notice_address_still_confirms() {
    let _t = t("fwpersonconfirm");
    let i = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::email_verify::issue(&conn, k2_core::email_verify::PURPOSE_PERSON_NOTICE, "owner", OUT, "owner", email_notify::now()).expect("issue")
    };
    let q = format!("t={}", i.token);
    let p = vpages::page_get(VERIFY_PATH, &q);
    assert!(p.html.contains("Confirm</button>"), "{p:?}");
    let p = vpages::page_post(VERIFY_PATH, "", q.as_bytes());
    assert!(p.html.contains("Confirmed"), "{p:?}");
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
    assert!(t.f.engine.sent.lock().unwrap().is_empty(), "no notice mail for a same-box forward");
    assert!(crate::mail::forward_outcomes::take_notices().is_empty(), "nor an owner notice");
    let (st, v) = post(UNSET_PATH, json!({ "address": MB }));
    assert_eq!((st, v["removed"].clone()), (200, json!(true)), "{v}");
    assert_eq!(t.io.forward(ACC), None);
}

/// Rosson 2026-10-30: a mail-manage agent sets an outside forward end to
/// end (no owner card). It can't resume an automatic pause. The address's
/// notice says an agent set it, and the owner hears.
#[test]
fn a_mail_manage_agent_manages_outside_forwards_but_not_auto_pauses() {
    let t = t("fwagent");
    system_sender_on();
    // The mailbox is minted by `ws-fw` (seed_mailbox), so the agent of
    // that workspace may manage it (authorize_mailbox: minting workspace).
    let _ = crate::mail::forward_outcomes::take_notices();
    let (st, v) = as_agent("ws-fw", || post(FORWARD_PATH, json!({ "address": MB, "to": OUT })));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    let text = t.f.engine.sent.lock().unwrap().last().expect("mail").email.to_string();
    assert!(text.contains("An AI agent"), "the notice says an agent set it: {text}");
    assert!(!text.contains("it-agent-folder") && !text.contains("ws-fw"), "and not its workspace's name: {text}");
    assert!(t.io.forward(ACC).is_some());
    let n = crate::mail::forward_outcomes::take_notices();
    assert!(n[0].1.contains("the AI agent"), "{n:?}");
    let ev = show()["events"].clone();
    assert!(ev.as_array().expect("events").iter().any(|e| e["event"] == "set" && e["actor"].as_str().unwrap_or("").starts_with("agent:")), "{ev}");
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
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    let (st, v) = call("owner", false, SELF_PATH, json!({}));
    assert_eq!((st, v["forward"]["to"].clone()), (200, json!(OUT)), "{v}");
    // Someone else's mailbox: refused.
    seed_mailbox("other@notify.brand.test", "direct");
    let (st, v) = call("owner", true, SELF_PATH, json!({ "address": "other@notify.brand.test", "to": OUT }));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("forbidden")), "{v}");
    let (st, v) = call("owner", true, SELF_ADD_PATH, json!({ "to": "b@outside.test" }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "{v}");
    let (st, v) = call("owner", true, SELF_RM_PATH, json!({ "to": "b@outside.test" }));
    assert_eq!((st, v["removed"].clone()), (200, json!(["b@outside.test"])), "{v}");
    for p in [SELF_ADD_PATH, SELF_RM_PATH] {
        assert!(is_self_route(p));
        assert_eq!(call("owner", false, p, json!({})).0, 405, "{p}");
    }
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
    for p in [ADD_PATH, RM_PATH, UNSET_PATH, PAUSE_PATH, RESUME_PATH, PERSON_LINK_PATH] {
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
        assert_eq!(t.f.engine.sent.lock().unwrap().len(), n, "no notice mail on {v:?}");
    }
    seed_mailbox("team@notify.brand.test", "direct");
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": "team@notify.brand.test" }));
    assert_eq!((st, v["state"].clone()), (200, json!("active")), "same-box still works: {v}");
    assert!(t.io.forward(ACC).is_some());
    crate::mail::forward_sync::set_test_version(Some(Some("0.16.25".to_string())));
    let (st, v) = post(FORWARD_PATH, json!({ "address": MB, "to": OUT }));
    assert_eq!(st, 200, "the pinned version: {v}");
    // On, then the box's mail server reads as older: show says it is held
    // for the upgrade (not active), and resume refuses (it lifts itself).
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
