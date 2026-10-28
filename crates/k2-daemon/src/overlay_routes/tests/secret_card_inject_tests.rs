//! Secret cards that say how a secret is sent (prd-secret-card-injection-v1,
//! vs-live SC-A1–SC-A18; Rosson's 0.46.6 rulings: no presets, older clients
//! refuse with "Update K2 to answer this card", the existing-name fix).
//!
//! The route half: `thread/secret` takes `inject: {dst, fmt}` and seals it
//! on the vault request; the card doc shows it (`secret.inject`, `envName`,
//! `replaces`); `thread/answer` needs the echo; the agent's set line says how
//! it is sent and what it replaced. Values never reach a response or a line.

use super::*;
use k2_vault::SecretStore as _;

const PW: &str = "k2test-route-card-pw-4e9a";

fn ws(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    (handle, project_id)
}

fn post_card(handle: &str, name: &str, extra: serde_json::Value) -> CliResponse {
    let mut body = serde_json::json!({ "addr": handle, "name": name, "for": "robot-ws.your-server.de", "from": "k2" });
    for (k, v) in extra.as_object().expect("object") {
        body[k] = v.clone();
    }
    post_json("/cli/thread/secret", &HashMap::new(), body)
}

fn answer_as(handle: &str, id: &str, value: &str, extra: serde_json::Value, author: &str) -> CliResponse {
    let mut body = serde_json::json!({ "addr": handle, "id": id, "secret": value, "disclosed": "1" });
    for (k, v) in extra.as_object().expect("object") {
        body[k] = v.clone();
    }
    dispatch_post_as("/cli/thread/answer", &HashMap::new(), body.to_string().as_bytes(), author)
}

fn answer(handle: &str, id: &str, value: &str, extra: serde_json::Value) -> CliResponse {
    answer_as(handle, id, value, extra, crate::session_author::OWNER_TOKEN)
}

fn card_doc(handle: &str, id: &str) -> serde_json::Value {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET"));
    snap["items"].as_array().expect("items").iter().find(|i| i["id"] == id).expect("card")["doc"]["secret"].clone()
}

fn sel(project_id: &str, name: &str) -> k2_vault::Selector {
    k2_vault::Selector::user(k2_vault::Scope::workspace(project_id.to_string()), name)
}

#[test]
fn a_basic_card_shows_seals_and_needs_the_echo() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlci");
    let posted = post_card(&handle, "HETZNER_ROBOT", serde_json::json!({ "kind": "password", "inject": { "dst": "basic" } }));
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let body = json_body(&posted);
    assert_eq!(body["inject"], serde_json::json!({ "dst": "basic", "fmt": "{}" }), "{body}");
    assert_eq!(body["envName"], "HETZNER_ROBOT", "{body}");
    assert_eq!(body["replaces"], false, "{body}");
    assert_eq!(body["sends"], "K2 will send HETZNER_ROBOT as a Basic login (user:password) to robot-ws.your-server.de", "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    // The vault request holds the sealed copy; the card shows it.
    let req = vault.request_get(&id).expect("request");
    assert_eq!(req.inject.as_ref().map(|i| i.dst.render()), Some("basic".to_string()));
    // A password with an injection is sent by K2 (SC-A5).
    assert_eq!(req.proposed_delivery, k2_vault::Delivery::Fetch);
    let doc = card_doc(&handle, &id);
    assert_eq!(doc["inject"], serde_json::json!({ "dst": "basic", "fmt": "{}" }), "{doc}");
    assert_eq!(doc["delivery"], "fetch", "{doc}");
    assert_eq!(doc["envName"], "HETZNER_ROBOT", "{doc}");
    assert!(doc.get("replaces").is_none(), "skipped when false: {doc}");

    // An older client (no echo) is refused, fail closed, in words.
    let old = answer(&handle, &id, &format!("u:{PW}"), serde_json::json!({}));
    assert_error_code(&old, "409 Conflict", "card_inject_unseen");
    assert!(old.body.contains("Update K2 to answer this card"), "{}", old.body);
    assert!(!old.body.contains(PW));
    assert_eq!(vault.request_get(&id).expect("request").status, "pending");

    // Basic with a bare {} needs user:password; the error never quotes it.
    let bare = answer(&handle, &id, PW, serde_json::json!({ "inject": { "dst": "basic", "fmt": "{}" } }));
    assert_error_code(&bare, "400 Bad Request", "basic_needs_user");
    assert!(!bare.body.contains(PW));

    let set = answer(&handle, &id, &format!("u:{PW}"), serde_json::json!({ "inject": { "dst": "basic", "fmt": "{}" } }));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert!(!set.body.contains(PW));
    assert_eq!(json_body(&set)["replaced"], false);
    let m = vault.meta(&sel(&project_id, "HETZNER_ROBOT"), &k2_vault::Actor::Owner).expect("meta");
    assert_eq!(m.inject.as_ref().map(|i| (i.dst.render(), i.fmt.clone())), Some(("basic".into(), "{}".into())));
    let injects = recorded_injects();
    assert!(
        injects.iter().any(|l| l.contains(&format!(
            "[thread:{handle}] secret HETZNER_ROBOT set → k2v://ws/{handle}/HETZNER_ROBOT (fetch, sent as a Basic login (user:password); robot-ws.your-server.de)"
        ))),
        "set line names how it is sent: {injects:?}"
    );
    assert!(injects.iter().all(|l| !l.contains(PW)), "{injects:?}");
}

#[test]
fn each_destination_posts_and_saves_as_shown() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlcik");
    for (i, (inject, words)) in [
        (serde_json::json!({ "dst": "basic", "fmt": "ws123:{}" }), "as a Basic login for user ws123"),
        (serde_json::json!({ "dst": "header:x-api-key" }), "in the X-Api-Key header"),
        (serde_json::json!({ "fmt": "Token {}" }), "as Authorization: Token ‹secret›"),
        (serde_json::json!({ "dst": "query:appid" }), "in the URL as ?appid=‹secret›"),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("KIND_{i}");
        let posted = post_card(&handle, &name, serde_json::json!({ "inject": inject }));
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let body = json_body(&posted);
        assert!(body["sends"].as_str().expect("sends").contains(words), "{body}");
        let id = body["id"].as_str().expect("id").to_string();
        // Save echoes what the card shows (the doc's normalised copy).
        let shown = card_doc(&handle, &id)["inject"].clone();
        let set = answer(&handle, &id, PW, serde_json::json!({ "inject": shown.clone() }));
        assert_eq!(set.status, "200 OK", "{name}: {}", set.body);
        let m = vault.meta(&sel(&project_id, &name), &k2_vault::Actor::Owner).expect("meta");
        let saved = m.inject.expect("inject");
        assert_eq!(serde_json::json!({ "dst": saved.dst.render(), "fmt": saved.fmt }), shown, "{name}");
        assert!(saved.allowed_dst.is_empty() && saved.allowed_fmt.is_empty(), "{name}: sealed, no choices");
    }
}

#[test]
fn body_env_and_bad_injections_are_refused_at_post() {
    let _home = crate::test_support::TempHome::new();
    crate::vault::ensure_test_vault();
    let (handle, _) = ws("ovlcib");
    // The agent writes the body, so it could echo the value back to itself.
    let body = post_card(&handle, "B", serde_json::json!({ "inject": { "dst": "body" } }));
    assert_error_code(&body, "400 Bad Request", "inject_body_not_on_cards");
    let env = post_card(&handle, "E", serde_json::json!({ "delivery": "env", "inject": { "dst": "basic" } }));
    assert_error_code(&env, "400 Bad Request", "inject_needs_fetch");
    let host = post_card(&handle, "H", serde_json::json!({ "inject": { "dst": "header:Host" } }));
    assert_error_code(&host, "400 Bad Request", "bad_inject");
    let slots = post_card(&handle, "F", serde_json::json!({ "inject": { "dst": "basic", "fmt": "{}:{}" } }));
    assert_error_code(&slots, "400 Bad Request", "bad_inject");
    let extra = post_card(&handle, "X", serde_json::json!({ "inject": { "dst": "basic", "allowedDst": ["body"] } }));
    assert_error_code(&extra, "400 Bad Request", "bad_inject");
    // And body as a person's edit on the answer, too.
    let plain = post_card(&handle, "P", serde_json::json!({}));
    let id = json_body(&plain)["id"].as_str().expect("id").to_string();
    let edit = answer(&handle, &id, PW, serde_json::json!({ "inject": { "dst": "body" } }));
    assert_error_code(&edit, "400 Bad Request", "inject_body_not_on_cards");
}

#[test]
fn a_card_without_an_injection_answers_as_before() {
    let _home = crate::test_support::TempHome::new();
    crate::vault::ensure_test_vault();
    let (handle, _) = ws("ovlcin");
    let posted = post_card(&handle, "PLAIN_KEY", serde_json::json!({}));
    let body = json_body(&posted);
    assert_eq!(body["inject"], serde_json::Value::Null, "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    assert!(card_doc(&handle, &id).get("inject").is_none());
    let set = answer(&handle, &id, PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
}

#[test]
fn an_env_card_names_the_real_env_variable() {
    let _home = crate::test_support::TempHome::new();
    crate::vault::ensure_test_vault();
    let (handle, _) = ws("ovlcie");
    let posted = post_json(
        "/cli/thread/secret",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "name": "hetzner-robot", "delivery": "env", "kind": "password", "from": "k2" }),
    );
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let body = json_body(&posted);
    assert_eq!(body["envName"], "HETZNER_ROBOT", "{body}");
    assert_eq!(body["sends"], "hetzner-robot goes to you as $HETZNER_ROBOT (you can read it)", "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    let set = answer(&handle, &id, PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    // SC-A7: the use hint gives the name the spawn really uses.
    let injects = recorded_injects();
    assert!(
        injects.iter().any(|l| l.contains(&format!("[thread:{handle}] secret hetzner-robot set")) && l.contains("Use: $HETZNER_ROBOT in new sessions")),
        "{injects:?}"
    );
}

/// The existing secret K2 Web had: saved as a Bearer token.
fn bearer_robot(project_id: &str, name: &str, by: &k2_vault::Actor) {
    let vault = crate::vault::ensure_test_vault();
    let mut r = k2_vault::PutRequest::new(k2_vault::Namespace::User, k2_vault::Scope::workspace(project_id.to_string()), name, k2_vault::Kind::ApiKey);
    r.hosts = vec!["robot-ws.your-server.de".into()];
    r.inject = Some(k2_vault::inject::Inject::bearer());
    r.default_delivery = Some(k2_vault::Delivery::Fetch);
    vault.put(r, k2_vault::SecretBytes::from_string("old-bearer-value".into()), by).expect("put");
}

#[test]
fn sc_a1_owner_and_admin_cards_replace_an_existing_secret_with_their_terms() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    k2_core::connect_users::add_user("ovlciada", "password123").expect("ada");
    k2_core::connect_users::set_role("ovlciada", k2_core::connect_users::Role::Admin).expect("admin");
    for (who, author) in [("owner", crate::session_author::OWNER_TOKEN), ("admin", "ovlciada")] {
        let (handle, project_id) = ws("ovlcia");
        bearer_robot(&project_id, "HETZNER_ROBOT", &k2_vault::Actor::Owner);
        let posted = post_card(&handle, "HETZNER_ROBOT", serde_json::json!({ "inject": { "dst": "basic" } }));
        let body = json_body(&posted);
        assert_eq!(body["replaces"], true, "{who}: {body}");
        assert!(body["sends"].as_str().expect("sends").ends_with("; this replaces the HETZNER_ROBOT already saved"), "{body}");
        let id = body["id"].as_str().expect("id").to_string();
        assert_eq!(card_doc(&handle, &id)["replaces"], true);
        let set = answer_as(&handle, &id, &format!("u:{PW}"), serde_json::json!({ "inject": { "dst": "basic", "fmt": "{}" } }), author);
        assert_eq!(set.status, "200 OK", "{who}: {}", set.body);
        assert_eq!(json_body(&set)["replaced"], true, "{who}");
        let m = vault.meta(&sel(&project_id, "HETZNER_ROBOT"), &k2_vault::Actor::Owner).expect("meta");
        assert_eq!(m.inject.as_ref().map(|i| i.dst.render()), Some("basic".to_string()), "{who}: Basic saved, not dropped");
        assert_eq!(m.latest_version, 2, "{who}");
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l.contains(&format!("[thread:{handle}] secret HETZNER_ROBOT set"))
                && l.contains("It replaced the earlier HETZNER_ROBOT (that one was sent as Authorization: Bearer ‹secret›).")),
            "{who}: {injects:?}"
        );
    }
}

#[test]
fn sc_a1_a_member_who_would_widen_is_refused_and_nothing_changes() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    k2_core::connect_users::add_user("ovlcimia", "password123").expect("mia");
    k2_core::connect_users::set_role("ovlcimia", k2_core::connect_users::Role::Member).expect("member");
    let (handle, project_id) = ws("ovlcim");
    let mia = k2_vault::Actor::Login { username: "ovlcimia".into(), role: k2_vault::Role::Member };
    bearer_robot(&project_id, "HETZNER_ROBOT", &mia);
    let before = vault.meta(&sel(&project_id, "HETZNER_ROBOT"), &k2_vault::Actor::Owner).expect("meta");
    let posted = post_card(&handle, "HETZNER_ROBOT", serde_json::json!({ "inject": { "dst": "basic" } }));
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let refused = answer_as(&handle, &id, &format!("u:{PW}"), serde_json::json!({ "inject": { "dst": "basic", "fmt": "{}" } }), "ovlcimia");
    assert_error_code(&refused, "403 Forbidden", "vault_admin_required");
    assert!(refused.body.contains("an Owner or Admin must save this card"), "{}", refused.body);
    assert_eq!(vault.meta(&sel(&project_id, "HETZNER_ROBOT"), &k2_vault::Actor::Owner).expect("meta"), before, "nothing written");
    assert_eq!(card_doc(&handle, &id)["status"], "pending");
    // The owner then saves the same card.
    let set = answer(&handle, &id, &format!("u:{PW}"), serde_json::json!({ "inject": { "dst": "basic", "fmt": "{}" } }));
    assert_eq!(set.status, "200 OK", "{}", set.body);
}

#[test]
fn after_the_card_an_agent_cannot_change_how_it_is_sent() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlcis");
    let posted = post_card(&handle, "SEALED_ROBOT", serde_json::json!({ "inject": { "dst": "basic" } }));
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let set = answer(&handle, &id, &format!("u:{PW}"), serde_json::json!({ "inject": { "dst": "basic", "fmt": "{}" } }));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    let agent = k2_vault::Actor::Agent(k2_vault::AgentActor { workspace_id: project_id.clone(), session_id: Some("s".into()), shell: false, vault_manage: false });
    let p = k2_vault::PolicyPatch { inject: Some(k2_vault::inject::Inject::bearer()), ..Default::default() };
    let e = vault.set_policy(&sel(&project_id, "SEALED_ROBOT"), p, &agent).expect_err("agent");
    assert_eq!(e.code, "vault_owner_confirm_required");
}
