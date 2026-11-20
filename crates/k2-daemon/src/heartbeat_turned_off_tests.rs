//! A heartbeat turned off, or paused on the LLM usage limit, says so: the
//! real failure policy (`heartbeat_launch::record_fire_failure_in`) on real
//! rows, with the Thread, the bus and the wallet faked and the clock
//! passed in. The real Thread write is `overlay_routes`
//! `heartbeat_turned_off_notice_tests`.

use std::collections::HashMap;

use chrono::{FixedOffset, Utc};
use parking_lot::Mutex;
use serde_json::{json, Value};

use super::*;
use crate::heartbeat_launch::{record_fire_failure_in, MAX_CONSECUTIVE_FAILURES};

const AGENT: &str = "sales";

#[derive(Default)]
struct Fake {
    posts: Mutex<Vec<(String, String, SystemNotice)>>,
    events: Mutex<Vec<TurnedOff>>,
    reading: Mutex<Option<i64>>,
    reading_asked: Mutex<usize>,
}

impl Fake {
    fn with_reading(reset: Option<i64>) -> Self {
        let f = Self::default();
        *f.reading.lock() = reset;
        f
    }
    fn posts(&self) -> Vec<(String, String, SystemNotice)> {
        self.posts.lock().clone()
    }
    fn events(&self) -> Vec<TurnedOff> {
        self.events.lock().clone()
    }
}

impl Effects for Fake {
    fn addr(&self, _project_id: &str) -> String {
        AGENT.to_string()
    }
    fn post(&self, addr: &str, text: &str, notice: SystemNotice) -> Result<String, String> {
        let mut p = self.posts.lock();
        p.push((addr.to_string(), text.to_string(), notice));
        Ok(format!("doc-{}", p.len()))
    }
    fn emit(&self, t: &TurnedOff) {
        self.events.lock().push(t.clone());
    }
    fn offset(&self, _at: i64) -> FixedOffset {
        FixedOffset::west_opt(7 * 3600).expect("offset")
    }
    fn reading_reset(&self, _project_id: &str, _addr: &str, now: i64) -> Option<i64> {
        *self.reading_asked.lock() += 1;
        self.reading.lock().filter(|r| *r > now)
    }
}

struct Ws {
    id: String,
    path: String,
    handle: String,
}

fn workspace(home: &crate::test_support::TempHome, name: &str) -> Ws {
    let handle = format!("hbto{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let dir = home.path().join(&handle);
    std::fs::create_dir_all(dir.join(format!(".k2/heartbeats/{name}"))).expect("workspace dir");
    let rel = format!(".k2/heartbeats/{name}/WAKEUP.md");
    std::fs::write(dir.join(&rel), "Check the queue.\n").expect("wakeup");
    let path = dir.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path])
        .expect("insert project");
    AgentHeartbeat::insert(&conn, &uuid::Uuid::new_v4().to_string(), &id, name, "daily", r#"{"time":"09:00"}"#, &rel, true)
        .expect("insert heartbeat");
    Ws { id, path, handle }
}

fn setup() -> (crate::test_support::TempHome, Ws) {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = workspace(&home, "morning");
    forget_out(&ws.id);
    (home, ws)
}

/// The row as smart_launch loads it before each fire.
fn row(ws: &Ws) -> AgentHeartbeat {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, &ws.id, "morning").expect("query").expect("row")
}

fn fail(fx: &Fake, ws: &Ws, reason: &str, now: i64) -> Value {
    let hb = row(ws);
    record_fire_failure_in(fx, &ws.id, AGENT, &hb, reason, now)
}

fn decisions(ws: &Ws) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT decision FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = 'morning' ORDER BY rowid")
        .expect("prepare");
    let out = stmt
        .query_map(rusqlite::params![ws.id], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    out
}

fn listed(ws: &Ws) -> Value {
    let body = crate::heartbeat_routes::dispatch_get("/cli/heartbeat/list", &ws.path, &HashMap::new()).expect("list");
    let rows: Vec<Value> = serde_json::from_str(&body).expect("json rows");
    rows.into_iter().find(|r| r["name"] == "morning").expect("listed")
}

#[test]
fn auto_disable_posts_exactly_one_notice_and_the_event() {
    let (_home, ws) = setup();
    let fx = Fake::default();
    let now = Utc::now().timestamp();
    for i in 1..=MAX_CONSECUTIVE_FAILURES {
        fail(&fx, &ws, "resume spawn failed: exit status 1; delivery: inline", now + i);
        let expect = usize::from(i == MAX_CONSECUTIVE_FAILURES);
        assert_eq!(fx.posts().len(), expect, "after failure {i}: {:?}", fx.posts());
    }
    let hb = row(&ws);
    assert!(!hb.enabled, "turned off after {MAX_CONSECUTIVE_FAILURES}");
    assert_eq!(hb.disabled_reason.as_deref(), Some("failures"));
    assert_eq!(decisions(&ws).iter().filter(|d| *d == "auto_disabled_failing").count(), 1);

    let posts = fx.posts();
    let (addr, text, notice) = &posts[0];
    assert_eq!(addr, AGENT);
    assert_eq!(text, "sales's heartbeat 'morning' was turned off after 5 failed runs (resume spawn failed: exit status 1).");
    assert_eq!(
        serde_json::to_value(notice).expect("json"),
        json!({
            "kind": "action",
            "code": "heartbeat.auto_disabled",
            "actions": [{
                "id": "heartbeat_enable",
                "label": "Turn it back on",
                "route": "/cli/heartbeat/enable",
                "body": {"project": "sales", "name": "morning", "enabled": true}
            }]
        })
    );
    let events = fx.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0],
        TurnedOff {
            workspace_id: ws.id.clone(),
            address: AGENT.into(),
            name: "morning".into(),
            state: STATE_DISABLED,
            reason: "resume spawn failed: exit status 1; delivery: inline".into(),
            failures: 5,
            paused_until: None,
            notice_id: Some("doc-1".into()),
        }
    );

    // A forced manual fire of the off row that fails again: no second notice.
    fail(&fx, &ws, "resume spawn failed: exit status 1", now + 10);
    assert_eq!(fx.posts().len(), 1, "one notice per disable");
    assert_eq!(fx.events().len(), 1, "one event per disable");
    assert_eq!(*fx.reading_asked.lock(), 0, "a plain failure never asks the wallet");
}

#[test]
fn the_notice_button_turns_it_back_on_through_the_enable_route() {
    let (_home, ws) = setup();
    let fx = Fake::default();
    let now = Utc::now().timestamp();
    for i in 1..=MAX_CONSECUTIVE_FAILURES {
        fail(&fx, &ws, "resume spawn failed: exit status 1", now + i);
    }
    assert!(!row(&ws).enabled);
    // What the client posts for the button, with the real handle.
    let notice = up::disabled_notice(&ws.handle, "morning");
    let action = &notice.actions[0];
    assert_eq!(action.route.as_deref(), Some("/cli/heartbeat/enable"));
    let mut params: HashMap<String, String> = HashMap::new();
    crate::heartbeat_app_routes::fold_json_body(&mut params, action.body.as_ref().expect("body").to_string().as_bytes());
    let r = crate::heartbeat_routes::with_request_actor(Some("owner-token".into()), || {
        crate::misc_routes::dispatch(up::ENABLE_ROUTE, &params).expect("enable is handled")
    });
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let hb = row(&ws);
    assert!(hb.enabled, "back on");
    assert_eq!(hb.disabled_reason, None);
    assert_eq!(hb.consecutive_failures, 0);
}

#[test]
fn the_usage_limit_pauses_instead_of_disabling_and_resumes_at_the_reset() {
    let (_home, ws) = setup();
    let now = Utc::now().timestamp();
    let reset = now + 3 * 3600;
    let fx = Fake::with_reading(Some(reset));
    let weekly = "restarted: weekly limit (its last turn was a weekly-limit error); hit an error at once (weekly_limit)";
    for i in 0..(MAX_CONSECUTIVE_FAILURES + 3) {
        fail(&fx, &ws, weekly, now + i);
    }
    let hb = row(&ws);
    assert!(hb.enabled, "the usage limit never turns it off");
    assert_eq!(hb.consecutive_failures, 0, "a usage-limit failure doesn't count");
    assert_eq!(hb.disabled_reason.as_deref(), Some(up::PAUSE_REASON));
    let until = reset + up::RESET_SLACK_SECS;
    assert_eq!(hb.next_retry_at.as_deref(), Some(up::fmt(until).as_str()), "paused until just past the reset");
    assert!(!decisions(&ws).iter().any(|d| d == "auto_disabled_failing"), "{:?}", decisions(&ws));

    let posts = fx.posts();
    assert_eq!(posts.len(), 1, "one notice per pause: {posts:?}");
    let when = ou::when_words(reset, now, fx.offset(reset));
    assert_eq!(posts[0].1, format!("sales's heartbeat 'morning' is paused until {when} (usage limit); it resumes by itself."));
    assert_eq!(serde_json::to_value(&posts[0].2).expect("json"), json!({"kind": "info", "code": "heartbeat.paused_usage_limit"}));
    let events = fx.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].state, STATE_PAUSED);
    assert_eq!(events[0].paused_until, Some(until));

    // The listing a manager reads.
    let l = listed(&ws);
    assert_eq!(l["enabled"], true, "{l}");
    assert_eq!(l["disabledReason"], up::PAUSE_REASON, "{l}");
    assert_eq!(l["pausedUntil"], up::fmt(until), "{l}");
    assert!(l.get("lastFired").is_some(), "{l}");

    // Before the reset nothing is re-checked; after it, nothing says the
    // login is out: it resumes.
    assert!(sweep(&fx, &ws.id, now + 3600).is_empty(), "not due yet");
    *fx.reading.lock() = None;
    assert_eq!(sweep(&fx, &ws.id, until + 1), vec![Rechecked::Resumed { name: "morning".into() }]);
    let hb = row(&ws);
    assert!(hb.enabled);
    assert_eq!(hb.next_retry_at, None, "it fires on its schedule again");
    assert_eq!(listed(&ws)["pausedUntil"], Value::Null);

    // The next fire goes through: the pause mark is gone.
    crate::heartbeat_launch::stamp_fired_and_release(&ws.id, "morning");
    let hb = row(&ws);
    assert_eq!(hb.disabled_reason, None, "a fire that went through ends the pause");
    assert!(hb.last_fired.is_some());
    assert_eq!(fx.posts().len(), 1, "resuming posts nothing");
}

#[test]
fn an_unknown_reset_rechecks_every_hour() {
    let (_home, ws) = setup();
    let now = Utc::now().timestamp();
    let fx = Fake::with_reading(None);
    fail(&fx, &ws, "resume spawn failed: StopFailure rate_limit", now);
    let hb = row(&ws);
    assert!(hb.enabled);
    assert_eq!(hb.next_retry_at.as_deref(), Some(up::fmt(now + up::RECHECK_SECS).as_str()));
    assert_eq!(
        fx.posts()[0].1,
        "sales's heartbeat 'morning' is paused (usage limit); it checks again every hour and resumes by itself."
    );

    assert!(sweep(&fx, &ws.id, now + 1800).is_empty(), "half an hour: not yet");
    // At the hour the login is still out (a fresh signal, reset unknown):
    // another hour.
    let hour = now + up::RECHECK_SECS;
    note_out(&ws.id, OutKind::Usage, None, hour);
    assert_eq!(
        sweep(&fx, &ws.id, hour),
        vec![Rechecked::Extended { name: "morning".into(), until: hour + up::RECHECK_SECS }]
    );
    assert_eq!(row(&ws).next_retry_at.as_deref(), Some(up::fmt(hour + up::RECHECK_SECS).as_str()));
    // The next hour the signal is old and the reading says nothing: resume.
    let two = hour + up::RECHECK_SECS;
    assert_eq!(sweep(&fx, &ws.id, two), vec![Rechecked::Resumed { name: "morning".into() }]);
    assert_eq!(row(&ws).next_retry_at, None);

    // The fire after it hits the limit again: paused again, no second notice.
    fail(&fx, &ws, "resume spawn failed: StopFailure rate_limit", two + 5);
    assert_eq!(row(&ws).next_retry_at.as_deref(), Some(up::fmt(two + 5 + up::RECHECK_SECS).as_str()));
    assert_eq!(fx.posts().len(), 1, "a re-pause is not a new notice");
    assert_eq!(fx.events().len(), 1);
}

#[test]
fn the_out_of_usage_signal_alone_makes_a_failure_the_limit() {
    let (_home, ws) = setup();
    let now = Utc::now().timestamp();
    let reset = now + 7200;
    let fx = Fake::with_reading(None);
    note_out(&ws.id, OutKind::Usage, Some(reset), now - 5);
    fail(&fx, &ws, "inject into live session x: wake prompt not delivered (timeout)", now);
    let hb = row(&ws);
    assert!(hb.enabled);
    assert_eq!(hb.consecutive_failures, 0);
    assert_eq!(hb.next_retry_at.as_deref(), Some(up::fmt(reset + up::RESET_SLACK_SECS).as_str()), "the signal's reset");
    assert_eq!(*fx.reading_asked.lock(), 0, "the signal had the reset");
    assert_eq!(fx.posts().len(), 1);
}

#[test]
fn a_failure_that_isnt_the_limit_ends_the_pause_and_counts() {
    let (_home, ws) = setup();
    let now = Utc::now().timestamp();
    let fx = Fake::with_reading(None);
    fail(&fx, &ws, "resume spawn failed: billing_error", now);
    assert_eq!(row(&ws).disabled_reason.as_deref(), Some(up::PAUSE_REASON));
    assert_eq!(fx.posts()[0].1, "sales's heartbeat 'morning' is paused (out of credit); it checks again every hour and resumes by itself.");
    fail(&fx, &ws, "resume spawn failed: exit status 1", now + up::RECHECK_SECS + 1);
    let hb = row(&ws);
    assert_eq!(hb.disabled_reason, None, "no longer paused");
    assert_eq!(hb.consecutive_failures, 1, "an ordinary failure counts");
    assert!(hb.enabled);
}

#[test]
fn a_reminder_is_never_paused() {
    let (_home, ws) = setup();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE workspace_heartbeats SET kind = 'reminder' WHERE project_id = ?1", rusqlite::params![ws.id])
            .expect("make it a reminder");
    }
    let fx = Fake::with_reading(None);
    fail(&fx, &ws, "resume spawn failed: StopFailure rate_limit", Utc::now().timestamp());
    let hb = row(&ws);
    assert_eq!(hb.disabled_reason, None, "a reminder keeps its own rule");
    assert_eq!(hb.consecutive_failures, 1);
    assert!(fx.posts().is_empty());
}
