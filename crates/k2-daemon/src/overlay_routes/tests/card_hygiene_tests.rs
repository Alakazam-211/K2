//! Thread card hygiene (prd-agent-settings-cards-v1 rev 3.1 §6.9, §6.10;
//! vs-live C13). Unswitched: these hold whatever the permission-cards
//! switches say (named exceptions to "off = today").
//!
//! - No caller voids a settings (action) card through the route: not the
//!   owner, a Member, an agent, a Garden widget or an App guest (C13).
//! - An agent session voids only a card it posted, and never a secret card.
//! - A sidecar can't answer or void the canonical Thread's cards.
//! - An agent's answer to an ordinary choice card (only with
//!   K2_CARD_PERSON_ONLY=off since 0.46.4) is stamped with its name.
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
    // 0.46.4 (AC24): the canonical agent no longer answers either: only a
    // person answers a card...
    let canonical = passport_params(&project_id, &handle);
    let answer = serde_json::json!({ "addr": handle, "id": id, "answer": "Go" });
    let r = post_json("/cli/thread/answer", &canonical, answer.clone());
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(json_body(&r)["error"]["code"], "card_person_only", "{}", r.body);
    assert_eq!(choice_of(&conv, &id).status, "pending", "a refusal writes nothing");
    // ...unless the owner turned the rule off (K2_CARD_PERSON_ONLY=off):
    // then today's canonical-only rule is all that applies.
    let r = with_card_person_only(false, || post_json("/cli/thread/answer", &canonical, answer.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
}

#[test]
fn an_agents_answer_is_stamped_with_its_name_and_a_persons_is_unchanged() {
    let handle = format!("ovlhya{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    let conv = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &conv);
    let id = ask_as(&HashMap::new(), &handle, "k2");
    // An agent answers only with the 0.46.4 rule switched off.
    let r = with_card_person_only(false, || {
        post_json(
            "/cli/thread/answer",
            &passport_params(&project_id, &handle),
            serde_json::json!({ "addr": handle, "id": id, "answer": "Go" }),
        )
    });
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

/// prd-app-caps-0464-v1 AC24/AC25/AC27 (0.46.4): only a person answers an
/// ordinary choice card. Refused (403 `card_person_only`, the card stays
/// pending): the asking agent's own passport (stamped and in the request
/// slot), a K2 terminal cell, an App's own pass, a platform pass and a
/// marked agent guest login. Allowed: a person's guest login, a Connect
/// login and the owner token. `K2_CARD_PERSON_ONLY=off` is the old path.
#[test]
fn only_a_person_answers_a_choice_card() {
    let _home = crate::test_support::TempHome::new();
    let handle = format!("ovlhyp{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    let conv = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &conv);
    let pass = |session: bool, principal: Option<&str>| SkinPass {
        id: format!("t-{}", uuid::Uuid::new_v4()),
        principal_id: principal.map(str::to_string),
        username: "guest".into(),
        caps: vec![k2_core::skin::CAP_THREAD_READ.into(), k2_core::skin::CAP_THREAD_POST.into()],
        rooms: vec![project_id.clone()],
        session,
        room_policy: [(project_id.clone(), vec![k2_core::skin::CAP_THREAD_READ.to_string(), k2_core::skin::CAP_THREAD_POST.to_string()])]
            .into_iter()
            .collect(),
        app: if session { "host".into() } else { String::new() },
        room_paths: Default::default(),
    };
    let person = k2_core::skin::add_principal(&format!("pat{}", &uuid::Uuid::new_v4().simple().to_string()[..6])).expect("person");
    let bot_name = format!("bot{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let bot = k2_core::skin::add_principal(&bot_name).expect("bot");
    k2_core::skin::set_principal_agent(&bot_name, Some(&project_id)).expect("agent mark");

    let answer = |id: &str| serde_json::json!({ "addr": handle, "id": id, "answer": "Go" });
    fn boxed<'a>(f: impl Fn(&str) -> CliResponse + 'a) -> Box<dyn Fn(&str) -> CliResponse + 'a> {
        Box::new(f)
    }
    let refused: Vec<(&str, Box<dyn Fn(&str) -> CliResponse + '_>)> = vec![
        ("its own agent's passport", boxed(|id: &str| post_json("/cli/thread/answer", &passport_params(&project_id, &handle), answer(id)))),
        (
            "its agent in the request slot",
            boxed(|id: &str| {
                crate::caller_workspace::with_request_principal(
                    Some(crate::session_token::HookPrincipal { workspace_uuid: project_id.clone(), agent_address: handle.clone() }),
                    || post_json("/cli/thread/answer", &HashMap::new(), answer(id)),
                )
            }),
        ),
        ("a K2 terminal cell", boxed(|id: &str| post_json("/cli/thread/answer", &params_of(&[("cell_session_id", "cell-p")]), answer(id)))),
        ("an App's own pass", boxed(|id: &str| with_request_skin(Some(pass(false, None)), || post_json("/cli/thread/answer", &HashMap::new(), answer(id))))),
        (
            "a marked agent guest login",
            boxed(|id: &str| with_request_skin(Some(pass(true, Some(&bot.id))), || post_json("/cli/thread/answer", &HashMap::new(), answer(id)))),
        ),
    ];
    for (who, call) in &refused {
        let id = ask_as(&HashMap::new(), &handle, "k2");
        let r = call(&id);
        assert_error_code(&r, "403 Forbidden", "card_person_only");
        assert!(r.body.contains("k2 thread void"), "{who}: the hint says how to close it: {}", r.body);
        assert_eq!(choice_of(&conv, &id).status, "pending", "{who}: a refusal writes nothing");
        // The old path (K2_CARD_PERSON_ONLY=off): today's rules, so it answers.
        let r = with_card_person_only(false, || call(&id));
        assert_eq!(r.status, "200 OK", "{who} with the switch off: {}", r.body);
    }
    // A platform pass (owner-minted, no login): refused as well.
    let id = ask_as(&HashMap::new(), &handle, "k2");
    let mut platform = pass(false, None);
    platform.app = String::new();
    let r = with_request_skin(Some(platform), || post_json("/cli/thread/answer", &HashMap::new(), answer(&id)));
    assert_error_code(&r, "403 Forbidden", "card_person_only");

    // People answer: a person's own guest login, a Connect login, the owner.
    let r = with_request_skin(Some(pass(true, Some(&person.id))), || post_json("/cli/thread/answer", &HashMap::new(), answer(&id)));
    assert_eq!(r.status, "200 OK", "a person's guest login: {}", r.body);
    let id = ask_as(&HashMap::new(), &handle, "k2");
    let r = dispatch_post_as("/cli/thread/answer", &HashMap::new(), answer(&id).to_string().as_bytes(), "hyg-member");
    assert_eq!(r.status, "200 OK", "a Connect login: {}", r.body);
    let id = ask_as(&HashMap::new(), &handle, "k2");
    let r = post_json("/cli/thread/answer", &HashMap::new(), answer(&id));
    assert_eq!(r.status, "200 OK", "the owner token: {}", r.body);
    // The agent still closes its own card (void is another route).
    let id = ask_as(&passport_params(&project_id, &handle), &handle, &handle);
    let r = void(&passport_params(&project_id, &handle), &handle, &id);
    assert_eq!(r.status, "200 OK", "k2 thread void by its agent: {}", r.body);
}

#[test]
fn the_switch_reads_off_0_false_and_no() {
    for v in ["off", "OFF", " 0 ", "false", "no"] {
        assert!(!card_person_only_from(Some(v)), "{v:?}");
    }
    for v in [None, Some("on"), Some("1"), Some(""), Some("yes")] {
        assert!(card_person_only_from(v), "{v:?}");
    }
}
