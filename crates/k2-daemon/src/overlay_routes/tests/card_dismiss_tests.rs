//! Dismissed and withdrawn cards (prd-popup-close-and-card-dismiss-v1 B1–B4,
//! 0.46.11). Rosson: "cards should have some kind of 'dismiss' system
//! because sometimes the question is no longer relevant."
//!
//! - A person dismisses a waiting card with an optional one-line reason:
//!   the daemon stores `status: voided` + `closedAs: dismissed` +
//!   `closeReason`, every read (headless too) shows it, and the agent that
//!   asked hears it on the same lane as an answer.
//! - The asking agent withdraws its own card: `closedAs: withdrawn`, no
//!   turn to itself.
//! - A stranger (another workspace's agent, an agent that didn't ask) is
//!   refused and nothing changes.
//! - A settings card is dismissed only through its answer gate.

use super::*;

fn ask_from(params: &HashMap<String, String>, handle: &str, from: &str) -> String {
    let r = post_json(
        "/cli/thread/ask",
        params,
        serde_json::json!({ "addr": handle, "prompt": "Ship the 0.46.10 build?", "options": "Ship,Wait", "from": from }),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json_body(&r)["id"].as_str().expect("id").to_string()
}

fn close(params: &HashMap<String, String>, handle: &str, id: &str, reason: Option<&str>) -> CliResponse {
    let mut body = serde_json::json!({ "addr": handle, "id": id });
    if let Some(r) = reason {
        body["reason"] = serde_json::json!(r);
    }
    post_json("/cli/thread/void", params, body)
}

/// The card's `choice` (or `secret`) as the Thread read serves it: what
/// every client, the phone and a headless `k2 thread --json` see.
fn served(handle: &str, id: &str) -> serde_json::Value {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET /cli/thread"));
    let item = snap["items"].as_array().expect("items").iter().find(|i| i["id"] == id).cloned().unwrap_or_else(|| panic!("card {id} not in the Thread read: {snap}"));
    if item["doc"]["kind"] == "secret" {
        item["doc"]["secret"].clone()
    } else {
        item["doc"]["choice"].clone()
    }
}

fn ws(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    (handle, project_id)
}

#[test]
fn a_persons_dismiss_is_stored_served_and_told_to_the_agent() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id) = ws("ovldm");
    let owner = workspace_msg::resolve_owner_from();
    let agent = passport_params(&project_id, &handle);
    let id = ask_from(&agent, &handle, &handle);

    // A reason is one line of at most 200 characters; a bad one changes nothing.
    let long = "x".repeat(201);
    let r = close(&HashMap::new(), &handle, &id, Some(&long));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    let r = close(&HashMap::new(), &handle, &id, Some("two\nlines"));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert_eq!(served(&handle, &id)["status"], "pending", "a refused dismiss leaves the card waiting");

    let r = close(&HashMap::new(), &handle, &id, Some("Shipped it yesterday"));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_body(&r);
    assert_eq!((&v["status"], &v["closedAs"], &v["reason"]), (&serde_json::json!("voided"), &serde_json::json!("dismissed"), &serde_json::json!("Shipped it yesterday")), "{v}");

    let c = served(&handle, &id);
    assert_eq!(c["status"], "voided", "{c}");
    assert_eq!(c["closedAs"], "dismissed", "the daemon stores the terminal state: {c}");
    assert_eq!(c["closeReason"], "Shipped it yesterday", "{c}");
    assert_eq!(c["answeredBy"], owner_answered_by_json(), "who dismissed it: {c}");
    assert!(c["answeredAt"].as_i64().is_some(), "{c}");

    let line = format!("[from {owner}] [thread:{handle}] dismissed card {id} without answering: \"Shipped it yesterday\"");
    let injects = recorded_injects();
    assert!(injects.iter().any(|l| l == &line), "the agent hears the dismiss like an answer: want {line:?} in {injects:?}");

    // Final: a second dismiss, an answer or a withdraw after it is refused.
    assert_error_code(&close(&HashMap::new(), &handle, &id, None), "409 Conflict", "card_not_pending");
    let a = post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id, "answer": "Ship" }));
    assert_eq!(a.status, "409 Conflict", "{}", a.body);
    assert_eq!(served(&handle, &id)["closedAs"], "dismissed");
}

#[test]
fn the_asking_agent_withdraws_its_own_card_and_is_not_woken() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id) = ws("ovlwd");
    let agent = passport_params(&project_id, &handle);
    let id = ask_from(&agent, &handle, &handle);
    let r = close(&agent, &handle, &id, Some("Found the answer in the docs"));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json_body(&r)["closedAs"], "withdrawn", "{}", r.body);
    let c = served(&handle, &id);
    assert_eq!((&c["status"], &c["closedAs"], &c["closeReason"]), (&serde_json::json!("voided"), &serde_json::json!("withdrawn"), &serde_json::json!("Found the answer in the docs")), "{c}");
    let injects = recorded_injects();
    assert!(!injects.iter().any(|l| l.contains(&id)), "a withdraw sends the agent nothing: {injects:?}");

    // `why` is the CLI's word for the reason.
    let id2 = ask_from(&agent, &handle, &handle);
    let r = post_json("/cli/thread/void", &agent, serde_json::json!({ "addr": handle, "id": id2, "why": "Superseded" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(served(&handle, &id2)["closeReason"], "Superseded");
}

#[test]
fn a_stranger_cannot_dismiss_or_withdraw_someone_elses_card() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id) = ws("ovlst");
    let (other_handle, other_id) = ws("ovlsx");
    // The person's own card on this Thread (stamped "rosson").
    let persons = ask_from(&HashMap::new(), &handle, "rosson");
    let agents = ask_from(&passport_params(&project_id, &handle), &handle, &handle);

    // This workspace's agent didn't ask the person's card.
    let r = close(&passport_params(&project_id, &handle), &handle, &persons, Some("not mine"));
    assert_error_code(&r, "403 Forbidden", "card_not_yours");
    // Another workspace's agent reaches neither card.
    for id in [&persons, &agents] {
        let r = close(&passport_params(&other_id, &other_handle), &handle, id, None);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    }
    for id in [&persons, &agents] {
        let c = served(&handle, id);
        assert_eq!(c["status"], "pending", "a refused close changes nothing: {c}");
        assert!(c["closedAs"].is_null(), "{c}");
    }
}

#[test]
fn an_agent_withdraws_a_secret_card_its_session_asked_but_never_k2s() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id) = ws("ovlss");
    let sid = uuid::Uuid::new_v4().to_string();
    let mut agent = passport_params(&project_id, &handle);
    agent.insert("cell_session_id".into(), sid.clone());
    let asked = post_json("/cli/thread/secret", &agent, serde_json::json!({ "addr": handle, "name": "DISMISS_KEY", "for": "api.example.com", "from": handle }));
    assert_eq!(asked.status, "200 OK", "{}", asked.body);
    let asked = json_body(&asked)["id"].as_str().expect("id").to_string();
    // K2's own secret card on the same Thread (a clone's, a reconnect):
    // the agent's handle on it, but no session asked.
    let k2s = post_json("/cli/thread/secret", &HashMap::new(), serde_json::json!({ "addr": handle, "name": "K2S_KEY", "for": "api.example.com", "from": handle }));
    assert_eq!(k2s.status, "200 OK", "{}", k2s.body);
    let k2s = json_body(&k2s)["id"].as_str().expect("id").to_string();

    let bare = passport_params(&project_id, &handle);
    assert_error_code(&close(&bare, &handle, &k2s, None), "403 Forbidden", "card_not_yours");
    assert_eq!(served(&handle, &k2s)["status"], "pending");
    let r = close(&bare, &handle, &asked, Some("Not needed after all"));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let s = served(&handle, &asked);
    assert_eq!((&s["status"], &s["closedAs"]), (&serde_json::json!("voided"), &serde_json::json!("withdrawn")), "{s}");
    let vault = crate::vault::get().expect("vault");
    assert_eq!(vault.request_get(&asked).expect("req").status, "void", "its vault request closes too");
    // The person still dismisses K2's card.
    let r = close(&HashMap::new(), &handle, &k2s, None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(served(&handle, &k2s)["closedAs"], "dismissed");
}

#[test]
fn only_its_person_dismisses_a_settings_card_and_no_action_runs() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id) = ws("ovlsc");
    let conv = pinned_chat_of(&project_id).expect("pinned chat");
    let widget = format!("dm{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let id = match present_keep(&project_id, "builder changed it. Keep it?", &KEEP_OPTIONS, keep_action(&widget, &"d".repeat(64)), true).expect("present") {
        Presented::Card { id, .. } => id,
        other => panic!("expected a card, got {other:?}"),
    };
    let body = serde_json::json!({ "addr": handle, "id": id }).to_string();
    let mut tries: Vec<(&str, CliResponse)> = vec![
        ("a Member login", dispatch_post_as("/cli/thread/void", &HashMap::new(), body.as_bytes(), "dm-member")),
        ("its agent", close(&passport_params(&project_id, &handle), &handle, &id, None)),
        (
            "a Garden widget",
            post_json("/cli/thread/void", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id, "origin": { "widget": "w", "garden": "g" } })),
        ),
    ];
    tries.push(("an App guest with thread:post", with_request_skin(Some(skin_pass(&[project_id.clone()])), || close(&HashMap::new(), &handle, &id, None))));
    for (who, r) in tries {
        assert_eq!(r.status, "403 Forbidden", "{who}: {}", r.body);
        assert_eq!(json_body(&r)["error"]["code"], "action_card_person_only", "{who}: {}", r.body);
    }
    let c = overlay::get_thread_doc(&conv, &id).expect("doc").expect("card").choice.expect("choice");
    assert_eq!(c.status, "pending", "every refused close leaves the card waiting");

    let r = close(&HashMap::new(), &handle, &id, Some("Leave it as is"));
    assert_eq!(r.status, "200 OK", "the owner dismisses it: {}", r.body);
    let v = serde_json::to_value(overlay::get_thread_doc(&conv, &id).expect("doc").expect("card")).expect("json");
    assert_eq!(v["choice"]["status"], "voided", "{v}");
    assert_eq!(v["choice"]["closedAs"], "dismissed", "{v}");
    assert!(v["choice"]["action"]["result"].is_null(), "a dismiss runs no action: {v}");
}
