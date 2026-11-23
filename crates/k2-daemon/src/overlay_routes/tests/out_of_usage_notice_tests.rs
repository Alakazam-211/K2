//! Out of usage (0.46.17): the notice lands on the agent's real Thread as
//! K2's own system row, with its buttons, and a second signal for the same
//! event writes nothing.

use super::*;
use crate::llm_out_notice::{self as lon, LiveWorld, Login, Place, World};
use chrono::FixedOffset;
use k2_core::llm_accounts::out_of_usage::{self as ou, Next};
use k2_core::llm_accounts::Tool;

struct Room {
    addr: String,
    next: Next,
}

impl World for Room {
    fn place(&self, _pane: &str, _hint: Option<&str>) -> Option<Place> {
        Some(Place { addr: self.addr.clone(), workspace_id: "ws".into() })
    }
    fn login(&self, _p: &Place, _t: Option<Tool>, _a: Option<&str>, _now: i64) -> Option<Login> {
        Some(Login {
            tool: Tool::Claude,
            id: Some("acc_1".into()),
            label: "Work Max".into(),
            api_key: false,
            reading_reset: None,
            next: self.next.clone(),
        })
    }
    fn post(&self, addr: &str, text: &str, notice: k2_core::overlay::SystemNotice) -> Result<String, String> {
        LiveWorld.post(addr, text, notice)
    }
    fn offset(&self, _at: i64) -> FixedOffset {
        FixedOffset::west_opt(7 * 3600).expect("offset")
    }
}

fn notices(handle: &str) -> Vec<serde_json::Value> {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET thread"));
    snap["items"].as_array().expect("items").iter().filter(|i| i["doc"].get("systemNotice").is_some()).cloned().collect()
}

#[test]
fn out_of_usage_is_one_system_row_on_the_agents_thread() {
    let _home_lock = crate::test_support::lock_home();
    let handle = format!("ouse{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    let world = Room { addr: handle.clone(), next: Next::Available { id: "acc_2".into(), label: "Home Pro".into() } };
    let episodes = parking_lot::Mutex::new(ou::Episodes::default());
    let now = chrono::Utc::now().timestamp();
    let reset = now + 3 * 3600;
    let sig = lon::Signal {
        pane: "pane-x".into(),
        project_hint: None,
        tool: Some(Tool::Claude),
        account: None,
        hit: ou::claude_statusline(&serde_json::json!({"five_hour": {"used_percentage": 100, "resets_at": reset}})).expect("hit"),
    };
    let posted = lon::handle(&world, &episodes, &sig, now).expect("posted");
    assert!(lon::handle(&world, &episodes, &sig, now + 5).is_none(), "same event: no second row");
    let rows = notices(&handle);
    assert_eq!(rows.len(), 1, "exactly one notice: {rows:?}");
    let doc = &rows[0]["doc"];
    assert_eq!(rows[0]["id"], posted.doc_id.as_str());
    assert_eq!(doc["author"], serde_json::json!({"kind": "system", "login": "k2"}), "{doc}");
    assert_eq!(doc["from"], "k2");
    let when = ou::when_words(reset, now, FixedOffset::west_opt(7 * 3600).expect("offset"));
    assert_eq!(
        doc["body"],
        format!("{handle} is out of usage on Work Max until {when}. Home Pro has room: switch to it, or add a login.")
    );
    assert_eq!(
        doc["systemNotice"],
        serde_json::json!({
            "kind": "action",
            "code": "llm.out_of_usage.switch_available",
            "actions": [
                {"id": "llm_switch", "label": "Switch to Home Pro", "route": "/cli/llm/accounts/switch", "body": {"id": "acc_2"}},
                {"id": "llm_add_login", "label": "Add a login", "body": {"tool": "claude"}}
            ]
        }),
        "{doc}"
    );
}

fn stop_failure_signal() -> lon::Signal {
    lon::Signal {
        pane: "pane-x".into(),
        project_hint: None,
        tool: Some(Tool::Claude),
        account: None,
        hit: ou::claude_stop_failure(Some("rate_limit"), None).expect("hit"),
    }
}

fn worked(started: i64, ended: i64) -> lon::Worked {
    lon::Worked { pane: "pane-x".into(), project_hint: None, started_ms: started * 1000, ended_ms: ended * 1000 }
}

/// 0.46.18, headless (no client: the daemon's own Thread write, read back
/// through `/cli/thread`): repeated detections are one row, the agent
/// working again is one more, and the next limit is a new episode.
#[test]
fn one_row_per_episode_then_one_working_again_row() {
    let _home_lock = crate::test_support::lock_home();
    let handle = format!("ousr{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    let world = Room { addr: handle.clone(), next: Next::Nothing };
    let episodes = parking_lot::Mutex::new(ou::Episodes::default());
    let now = chrono::Utc::now().timestamp();
    let sig = stop_failure_signal();
    lon::handle(&world, &episodes, &sig, now).expect("the first detection posts");
    for i in 1..=5 {
        assert!(lon::handle(&world, &episodes, &sig, now + i * 30).is_none(), "detection {i} of the same episode");
    }
    let rows = notices(&handle);
    assert_eq!(rows.len(), 1, "one row for six detections: {rows:?}");
    // No reset known: no reset words.
    assert_eq!(rows[0]["doc"]["body"], format!("{handle} is out of usage on Work Max. Add or switch a login."));
    // The turn that ran out, and a retry that ran out again, aren't working again.
    assert!(lon::handle_worked(&world, &episodes, &worked(now - 5, now), None).is_none(), "the turn that hit it");
    assert!(
        lon::handle_worked(&world, &episodes, &worked(now + 60, now + 61), Some((now + 61) * 1000)).is_none(),
        "a retry that showed the limit again"
    );
    assert_eq!(notices(&handle).len(), 1);
    // A good turn after the limit: one line.
    let good = worked(now + 3600, now + 3700);
    let id = lon::handle_worked(&world, &episodes, &good, Some((now + 61) * 1000)).expect("working again");
    assert!(lon::handle_worked(&world, &episodes, &good, None).is_none(), "once per episode");
    let rows = notices(&handle);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let doc = &rows[1]["doc"];
    assert_eq!(rows[1]["id"], id.as_str());
    assert_eq!(doc["body"], format!("{handle} is working again."));
    assert_eq!(doc["author"], serde_json::json!({"kind": "system", "login": "k2"}), "{doc}");
    assert_eq!(doc["systemNotice"], serde_json::json!({"kind": "info", "code": "llm.out_of_usage.resumed"}), "{doc}");
    // The next limit is news again.
    lon::handle(&world, &episodes, &sig, now + 3800).expect("a new episode posts");
    assert_eq!(notices(&handle).len(), 3);
}

/// 0.46.18: the out notice reaches the person's phone under the agent's
/// name, with the notice's words; the working-again line never pushes; the
/// person's bell off means no push.
#[test]
fn the_out_notice_pushes_under_the_agents_name_and_respects_the_bell() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    k2_core::push_devices::upsert_device("ouse-dev", "apns", "ouse-tok", "owner").expect("phone");
    let (ws, conv) = push_ws("ou");
    let world = Room { addr: ws.clone(), next: Next::Nothing };
    let episodes = parking_lot::Mutex::new(ou::Episodes::default());
    let now = chrono::Utc::now().timestamp();
    let mark = crate::push_routes::test_push_capture::mark();
    let posted = lon::handle(&world, &episodes, &stop_failure_signal(), now).expect("posted");
    crate::overlay_routes::push_system_notice(&posted.addr, &posted.doc_id).expect("pushed");
    let pushes = thread_pushes(mark, &conv);
    assert_eq!(pushes.len(), 1, "one push: {pushes:?}");
    let ev = k2_core::push::PushEvent::ThreadPost(pushes[0].clone());
    assert_eq!(ev.title(), ws, "under the agent's name, not K2's");
    assert_eq!(crate::thread_push::last_preview(&conv).as_deref(), Some(posted.text.as_str()));
    // Working again: a Thread line, no push.
    let mark = crate::push_routes::test_push_capture::mark();
    lon::handle_worked(&world, &episodes, &worked(now + 10, now + 20), None).expect("working again");
    assert!(thread_pushes(mark, &conv).is_empty(), "the working-again line is quiet");
    // A notice that isn't K2's is refused.
    let r = post_json("/cli/thread/post", &agent_of(&ws), serde_json::json!({ "addr": ws, "text": "hi" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let reply_id = json_body(&r)["id"].as_str().expect("id").to_string();
    let err = crate::overlay_routes::push_system_notice(&ws, &reply_id).expect_err("not a K2 notice");
    assert!(err.contains("not a K2 notice"), "{err}");
    // The bell off: no push for the next agent's notice.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::notify_prefs::set_server(&conn, k2_core::notify_prefs::OWNER, false, "owner").expect("bell off");
    }
    let (ws2, conv2) = push_ws("ob");
    let world2 = Room { addr: ws2.clone(), next: Next::Nothing };
    let mark = crate::push_routes::test_push_capture::mark();
    let posted = lon::handle(&world2, &episodes, &stop_failure_signal(), now).expect("posted");
    crate::overlay_routes::push_system_notice(&posted.addr, &posted.doc_id).expect("handed to the coalescer");
    assert!(thread_pushes(mark, &conv2).is_empty(), "the bell is off: no push");
}
