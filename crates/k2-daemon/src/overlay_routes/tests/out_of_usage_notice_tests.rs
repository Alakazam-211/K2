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
