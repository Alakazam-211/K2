//! Thread card hygiene (prd-agent-settings-cards-v1 rev 3.1 §6.9, §6.10;
//! vs-live C13). Unswitched: these hold whatever the permission-cards
//! switches say (named exceptions to "off = today").
//!
//! - No caller voids a settings (action) card through the route: not the
//!   owner, a Member, an agent, a Garden widget or an App guest (C13).
//! - An agent session voids only a card it posted, and never a secret card.
//! - A sidecar can't answer or void the canonical Thread's cards.
//! - An agent's answer to an ordinary choice card is stamped with its name.
//! - A choice ask has at most 8 options of 80 characters and a 1,000-
//!   character prompt.

use super::*;

fn ask_as(params: &HashMap<String, String>, handle: &str, from: &str) -> String {
    let r = post_json(
        "/cli/thread/ask",
        params,
        serde_json::json!({ "addr": handle, "prompt": "Ship it?", "options": "Go,Stop", "from": from }),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json_body(&r)["id"].as_str().expect("id").to_string()
}

fn void(params: &HashMap<String, String>, handle: &str, id: &str) -> CliResponse {
    post_json("/cli/thread/void", params, serde_json::json!({ "addr": handle, "id": id }))
}

fn choice_of(conv: &str, id: &str) -> k2_core::overlay::doc::ChoiceBody {
    overlay::get_thread_doc(conv, id).expect("doc").expect("card").choice.expect("choice")
}

#[test]
fn no_caller_voids_an_action_card_through_the_route() {
    let handle = format!("ovlhyv{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    let conv = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &conv);
    let widget = format!("hv{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let id = match present_keep(&project_id, "builder changed it. Keep it?", &KEEP_OPTIONS, keep_action(&widget, &"b".repeat(64)), true)
        .expect("present")
    {
        Presented::Card { id, .. } => id,
        other => panic!("expected a card, got {other:?}"),
    };
    let body = serde_json::json!({ "addr": handle, "id": id }).to_string();
    let mut tries: Vec<(&str, CliResponse)> = vec![
        ("the owner token", void(&HashMap::new(), &handle, &id)),
        ("a Member login", dispatch_post_as("/cli/thread/void", &HashMap::new(), body.as_bytes(), "hyg-member")),
        ("an Owner login", dispatch_post_as("/cli/thread/void", &HashMap::new(), body.as_bytes(), "hyg-owner")),
        ("its agent", void(&passport_params(&project_id, &handle), &handle, &id)),
        (
            "a Garden widget",
            post_json(
                "/cli/thread/void",
                &HashMap::new(),
                serde_json::json!({ "addr": handle, "id": id, "origin": { "widget": "w", "garden": "g" } }),
            ),
        ),
    ];
    let guest = with_request_skin(Some(skin_pass(&[project_id.clone()])), || void(&HashMap::new(), &handle, &id));
    tries.push(("an App guest with thread:post", guest));
    for (who, r) in tries {
        assert_error_code(&r, "403 Forbidden", "action_card_person_only");
        assert!(json_body(&r)["error"]["hint"].as_str().expect("hint").contains("deny it instead"), "{who}: {}", r.body);
    }
    assert_eq!(choice_of(&conv, &id).status, "pending", "every refused void leaves the card waiting");
    // The daemon's own close still works (expiry, supersede, switch-off).
    overlay::void_card(&conv, &project_id, &id, None).expect("the daemon voids directly");
    assert_eq!(choice_of(&conv, &id).status, "voided");
}

#[test]
fn an_agent_voids_only_its_own_cards_and_never_a_secret_card() {
    let handle = format!("ovlhyo{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    let conv = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &conv);
    let agent = passport_params(&project_id, &handle);

    // A card someone else posted (the owner token, as "rosson"): the agent
    // can't dismiss it. (An owner-token ask with no `from`, or `k2`, is
    // stamped with the room handle, the agent's own name, so the card's
    // `from` is what tells them apart.)
    let persons = ask_as(&HashMap::new(), &handle, "rosson");
    assert_error_code(&void(&agent, &handle, &persons), "403 Forbidden", "card_not_yours");
    assert_eq!(choice_of(&conv, &persons).status, "pending");
    // Its own card: as before.
    let own = ask_as(&agent, &handle, &handle);
    let r = void(&agent, &handle, &own);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(choice_of(&conv, &own).status, "voided");
    // The person still dismisses any ordinary card.
    let r = void(&HashMap::new(), &handle, &persons);
    assert_eq!(r.status, "200 OK", "{}", r.body);

    // A secret card, even one the agent asked for: the person's to dismiss.
    let posted = post_json(
        "/cli/thread/secret",
        &agent,
        serde_json::json!({ "addr": handle, "name": "HYG_TOKEN", "for": "api.example.com", "from": handle }),
    );
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let secret = json_body(&posted)["id"].as_str().expect("id").to_string();
    assert_error_code(&void(&agent, &handle, &secret), "403 Forbidden", "card_not_yours");
    let doc = overlay::get_thread_doc(&conv, &secret).expect("doc").expect("card");
    assert_eq!(doc.secret.expect("secret").status, "pending", "the refused void left it waiting");
}

#[test]
fn a_sidecar_neither_answers_nor_voids_the_canonical_threads_cards() {
    let handle = format!("ovlhys{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    let conv = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &conv);
    let side_conv = uuid::Uuid::new_v4().to_string();
    sidecar(&project_id, &side_conv, "reviewer");
    let side = passport_params(&project_id, &format!("{handle}/reviewer"));
    let id = ask_as(&HashMap::new(), &handle, "k2");
    let r = post_json("/cli/thread/answer", &side, serde_json::json!({ "addr": handle, "id": id, "answer": "Go" }));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("canonical-only"), "{}", r.body);
    let r = void(&side, &handle, &id);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("canonical-only"), "{}", r.body);
    assert_eq!(choice_of(&conv, &id).status, "pending");
    // The canonical agent still answers on its own Thread.
    let r = post_json(
        "/cli/thread/answer",
        &passport_params(&project_id, &handle),
        serde_json::json!({ "addr": handle, "id": id, "answer": "Go" }),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
}

#[test]
fn an_agents_answer_is_stamped_with_its_name_and_a_persons_is_unchanged() {
    let handle = format!("ovlhya{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    let conv = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &conv);
    let id = ask_as(&HashMap::new(), &handle, "k2");
    let r = post_json(
        "/cli/thread/answer",
        &passport_params(&project_id, &handle),
        serde_json::json!({ "addr": handle, "id": id, "answer": "Go" }),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let by = choice_of(&conv, &id).answered_by.expect("an agent's answer names it");
    assert_eq!((by.kind.as_str(), by.login.as_str(), by.name.as_deref()), ("agent", handle.as_str(), None));
    let wire = serde_json::to_value(overlay::get_thread_doc(&conv, &id).expect("doc").expect("card")).expect("json");
    assert_eq!(wire["choice"]["answeredBy"], serde_json::json!({ "kind": "agent", "login": handle }), "{wire}");

    // A person's answer: today's owner stamp.
    let id2 = ask_as(&HashMap::new(), &handle, "k2");
    let r = post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id2, "answer": "Stop" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(
        serde_json::to_value(choice_of(&conv, &id2).answered_by).expect("json"),
        owner_answered_by_json(),
        "the owner token's stamp is unchanged"
    );
}

#[test]
fn a_choice_ask_is_capped_at_eight_options_of_eighty_and_a_thousand_char_prompt() {
    let handle = format!("ovlhyc{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    let agent = passport_params(&project_id, &handle);
    let ask = |prompt: String, options: serde_json::Value| {
        post_json("/cli/thread/ask", &agent, serde_json::json!({ "addr": handle, "prompt": prompt, "options": options.to_string(), "from": handle }))
    };
    let eight: Vec<String> = (0..8).map(|i| format!("{i}{}", "o".repeat(79))).collect();
    let r = ask("p".repeat(1000), serde_json::json!(eight));
    assert_eq!(r.status, "200 OK", "8 options of 80 and a 1,000-character prompt pass: {}", r.body);
    let nine: Vec<String> = (0..9).map(|i| format!("o{i}")).collect();
    for (what, r) in [
        ("9 options", ask("Pick".into(), serde_json::json!(nine))),
        ("an 81-character option", ask("Pick".into(), serde_json::json!(["a", "o".repeat(81)]))),
        ("a 1,001-character prompt", ask("p".repeat(1001), serde_json::json!(["a", "b"]))),
    ] {
        assert_eq!(r.status, "400 Bad Request", "{what}: {}", r.body);
        assert_eq!(json_body(&r)["error"]["code"], "usage", "{what}: {}", r.body);
    }
}
