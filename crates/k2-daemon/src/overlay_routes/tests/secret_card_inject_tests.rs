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

/// SC-A8: a clone's leave-blank card keeps how the source sent it; a sending
/// a card can't carry (a signer) or a readable delivery falls back to none.
#[test]
fn clone_leave_blank_cards_carry_the_sending() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlcic");
    let conv = uuid::Uuid::new_v4().to_string();
    let basic = k2_vault::inject::card_inject(Some("basic"), Some("ws123:{}")).expect("inject").expect("some");
    let hosts = vec!["https://robot-ws.your-server.de:443".to_string()];
    let id = post_clone_secret_card(&conv, &project_id, &handle, "CLONED_ROBOT", "password", &hosts, "fetch", "carried", Some(&basic))
        .expect("card");
    assert_eq!(vault.request_get(&id).expect("request").inject, Some(basic.clone()));
    let env = post_clone_secret_card(&conv, &project_id, &handle, "CLONED_ENV", "password", &[], "env", "carried", Some(&basic))
        .expect("card");
    assert_eq!(vault.request_get(&env).expect("request").inject, None, "env: no sending on the card");
    let signed = k2_vault::inject::Inject { sign: Some(k2_vault::inject::Sign::Ovh), ..k2_vault::inject::Inject::bearer() };
    let ovh = post_clone_secret_card(&conv, &project_id, &handle, "CLONED_OVH", "generic", &hosts, "fetch", "carried", Some(&signed))
        .expect("card");
    assert_eq!(vault.request_get(&ovh).expect("request").inject, None, "a signer never rides a card");
}

const SESSION_PW: &str = "k2test-session-card-pw-71c3";

/// A live agent session with its secrets dir, under a throwaway runtime dir.
struct LiveSession {
    sid: String,
    dir: std::path::PathBuf,
    _run: k2_core::test_env::EnvVar,
    _t: k2_vault::fsutil::TestDir,
}

fn live_session(tag: &str, project_id: &str) -> LiveSession {
    let t = k2_vault::fsutil::TestDir::new(tag);
    let run = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, t.path().join("run"));
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = crate::vault::spawn::materialize(&mut HashMap::new(), project_id, &sid, false, &crate::vault::spawn::SpawnRefs::default())
        .dir
        .expect("a live session has a secrets dir");
    LiveSession { sid, dir, _run: run, _t: t }
}

fn post_as(sid: &str, handle: &str, name: &str, scope: &str, delivery: &str) -> CliResponse {
    post_json(
        "/cli/thread/secret",
        &params_of(&[("cell_session_id", sid)]),
        serde_json::json!({ "addr": handle, "name": name, "scope": scope, "delivery": delivery, "from": "k2" }),
    )
}

/// The field report: `k2 thread secret --scope session --delivery file`, Save,
/// card "saved", and the asking session saw nothing in `$K2_SECRETS_DIR` or
/// `k2 vault list`: the secret was bound to the Thread's conversation id, and
/// every check compares the passport's session id.
#[test]
fn a_session_file_card_reaches_the_session_that_asked() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlss");
    let s = live_session("sess-card", &project_id);
    let posted = post_as(&s.sid, &handle, "SESSION_FILE", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();

    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert!(!set.body.contains(SESSION_PW));
    let body = json_body(&set);
    assert_eq!(body["ref"], format!("k2v://session/{}/SESSION_FILE", s.sid), "{body}");
    assert_eq!(body["fileDelivered"], true, "{body}");
    assert_eq!(std::fs::read(s.dir.join("SESSION_FILE")).expect("the file is there at once"), SESSION_PW.as_bytes());

    // k2 vault list, and a bare NAME from that session.
    let me = crate::vault::actor::VaultCaller {
        actor: k2_vault::Actor::Agent(k2_vault::AgentActor { workspace_id: project_id.clone(), session_id: Some(s.sid.clone()), shell: false, vault_manage: false }),
        channel: crate::vault::actor::Channel::Plain,
        session_id: Some(s.sid.clone()),
    };
    let listed = vault.list(&k2_vault::ListFilter::default(), &me.actor).expect("list");
    assert!(listed.iter().any(|m| m.name == "SESSION_FILE"), "k2 vault list shows it");
    let file = crate::vault_routes::handle_on(
        &vault,
        crate::vault_use_routes::FILE,
        Ok(me.clone()),
        serde_json::json!({ "secret": "SESSION_FILE" }).to_string().as_bytes(),
        &HashMap::new(),
        "test",
        None,
    );
    assert_eq!(file.status, "200 OK", "k2 vault file --secret finds its own session's secret: {}", file.body);

    let injects = recorded_injects();
    let line = injects.iter().find(|l| l.contains("secret SESSION_FILE set")).unwrap_or_else(|| panic!("{injects:?}"));
    assert!(line.contains("Delivered: $K2_SECRETS_DIR/SESSION_FILE is there now."), "{line}");
    assert!(line.contains("It stays with this chat until it expires: after a restart or --resume"), "the agent hears it follows the chat: {line}");
    assert!(injects.iter().all(|l| !l.contains(SESSION_PW)), "{injects:?}");
    crate::vault::spawn::session_ended(&s.sid);
}

/// Asking is refused when nothing could receive it, so the agent hears at once.
#[test]
fn a_session_card_is_refused_when_nothing_could_receive_it() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlsr");
    let s = live_session("sess-refuse", &project_id);

    // Not from a session (an owner's terminal).
    let r = post_json("/cli/thread/secret", &HashMap::new(), serde_json::json!({ "addr": handle, "name": "A", "scope": "session", "delivery": "file", "from": "k2" }));
    assert_error_code(&r, "400 Bad Request", "session_scope_needs_session");
    // A session K2 doesn't know is running (a job's passport).
    assert_error_code(&post_as("job-r1", &handle, "A", "session", "file"), "400 Bad Request", "session_scope_needs_session");
    // Env can't reach a running session, and a new one is another session.
    let r = post_as(&s.sid, &handle, "A", "session", "env");
    assert_error_code(&r, "400 Bad Request", "session_env_unreachable");
    assert!(r.body.contains("--scope workspace"), "{}", r.body);
    crate::vault::spawn::session_ended(&s.sid);
}

/// The person is told before Save when the session that asked is gone: the
/// value isn't kept, the card stays open, and saving for the workspace works.
#[test]
fn saving_for_a_session_that_ended_is_refused_and_the_card_stays_open() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlse");
    let s = live_session("sess-ended", &project_id);
    let posted = post_as(&s.sid, &handle, "ENDED_FILE", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    crate::vault::spawn::session_ended(&s.sid);

    let r = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_error_code(&r, "409 Conflict", "card_session_gone");
    assert!(!r.body.contains(SESSION_PW));
    assert_eq!(card_doc(&handle, &id)["status"], "pending", "the card stays open");
    assert!(!vault.user_secret_exists(&k2_vault::Scope::session(s.sid.clone()), "ENDED_FILE"), "nothing kept");

    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({ "scope": "workspace" }));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert!(vault.user_secret_exists(&k2_vault::Scope::workspace(project_id), "ENDED_FILE"));
}

/// `k2 vault grant --to session:<id>` reaches only the id a running session's
/// passport holds. `k2 whoami` prints the chat's id, which is another id: a
/// share with that, or with a session that isn't running, would reach nothing,
/// so it is refused and no grant is made.
#[test]
fn a_share_with_a_session_needs_a_running_sessions_id() {
    use k2_vault::{Actor, Delivery, Kind, Namespace, PutRequest, Scope, SecretBytes};
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (_handle, project_id) = ws("ovlsh");
    let chat = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &chat);
    let mut put = PutRequest::new(Namespace::User, Scope::workspace(project_id.clone()), "SHARED_FILE", Kind::Generic);
    put.default_delivery = Some(Delivery::File);
    put.delivery_ceiling = Some(Delivery::Env);
    let id = vault.put(put, SecretBytes::from_slice(SESSION_PW.as_bytes()), &Actor::Owner).expect("put").meta.id;
    let selector = k2_vault::Selector::Id(id.clone());
    // An owner's own login: the raw owner token needs a confirm to share a file.
    let owner = crate::vault::actor::VaultCaller {
        actor: Actor::Login { username: "ada".into(), role: k2_vault::Role::Owner },
        channel: crate::vault::actor::Channel::Login,
        session_id: None,
    };
    let grant = |to: &str| {
        let body = serde_json::json!({ "id": id, "to": to, "as": "file" }).to_string();
        crate::vault_routes::handle_on(&vault, crate::vault_routes::GRANT, Ok(owner.clone()), body.as_bytes(), &HashMap::new(), "test", None)
    };

    let r = grant(&format!("session:{chat}"));
    assert!(r.status.starts_with("400"), "{}", r.body);
    assert!(r.body.contains("session_not_running") && r.body.contains("k2 whoami") && r.body.contains("k2 sessions live"), "{}", r.body);
    let r = grant("session:never-ran");
    assert!(r.status.starts_with("400"), "{}", r.body);
    assert!(r.body.contains("session_not_running") && !r.body.contains("k2 whoami"), "{}", r.body);
    assert!(vault.grants(&selector, &Actor::Owner).expect("grants").is_empty(), "a refusal makes no grant");

    // A running session's id works, and the file arrives.
    let s = live_session("sess-share", &uuid::Uuid::new_v4().to_string());
    let r = grant(&format!("session:{}", s.sid));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(std::fs::read(s.dir.join("SHARED_FILE")).expect("arrived"), SESSION_PW.as_bytes());
    // Removing a share from a session that has ended is never refused.
    let revoke = serde_json::json!({ "id": id, "from": format!("session:{}", s.sid) }).to_string();
    crate::vault::spawn::session_ended(&s.sid);
    let r = crate::vault_routes::handle_on(&vault, crate::vault_routes::REVOKE, Ok(owner.clone()), revoke.as_bytes(), &HashMap::new(), "test", None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
}

/// A PTY standing in for an agent's main chat: `claude` is the test shim, and
/// it is registered under the workspace id, as a running canonical chat is.
#[cfg(unix)]
struct RunningChat {
    key: String,
    session: std::sync::Arc<k2_core::terminal::DaemonPtySession>,
    _rt: tokio::runtime::Runtime,
    _shim: k2_core::test_env::AgentShim,
}

#[cfg(unix)]
impl RunningChat {
    fn start(project_id: &str) -> RunningChat {
        Self::start_as(project_id)
    }

    /// Registered under `key`: the workspace id, or a sidecar's `tab-…`.
    fn start_as(project_id: &str) -> RunningChat {
        use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession, LabelSource};
        let shim = k2_core::test_env::AgentShim::install();
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
        let enter = rt.enter();
        let label_source = LabelSource::Pty;
        let session = DaemonPtySession::spawn(DaemonPtyConfig {
            session_id: k2_core::session::SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(std::path::PathBuf::from("/tmp")),
            program: Some("claude".to_string()),
            args: vec![],
            durable_args: None,
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source,
            sandbox: k2_core::terminal::SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            survive: false,
            ptyd_meta: None,
        })
        .expect("spawn the fake agent");
        crate::v2_session_map::register(project_id.to_string(), session.clone());
        drop(enter);
        RunningChat { key: project_id.to_string(), session, _rt: rt, _shim: shim }
    }

    fn sid(&self) -> String {
        self.session.session_id.to_string()
    }
}

#[cfg(unix)]
impl Drop for RunningChat {
    fn drop(&mut self) {
        crate::v2_session_map::unregister(&self.key);
        self.session.kill();
    }
}

/// An App guest can only save for a session, and a card stays open for 7 days,
/// so the session that asked has often ended by Save: an idle agent sleeps, and
/// its next session has a new id. Whether the session that asked has gone or a
/// person posted the card with none:
/// - nothing running: nothing is kept, the card stays open, the guest is told
///   only what a guest can do, and the agent is woken and told;
/// - the chat running again: the same card saves, for that session, and that
///   session can use it.
#[cfg(unix)]
#[test]
fn a_guests_save_reaches_the_chats_running_session() {
    use k2_vault::{Requester, Selector, Use};
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlsg");
    let as_guest = |id: &str| {
        let mut pass = person_pass(&[project_id.clone()]);
        pass.username = "gail".into();
        with_request_skin(Some(pass), || {
            post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id, "secret": SESSION_PW }))
        })
    };
    let body = |name: &str| serde_json::json!({ "addr": handle, "name": name, "for": "api.example.com", "from": "k2" });
    let cards: Vec<(&str, String)> = [
        ("GUEST_ENDED", params_of(&[("cell_session_id", "gone-session")])),
        ("GUEST_NONE", HashMap::new()),
    ]
    .into_iter()
    .map(|(name, params)| {
        let posted = post_json("/cli/thread/secret", &params, body(name));
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        (name, json_body(&posted)["id"].as_str().expect("id").to_string())
    })
    .collect();

    for (name, id) in &cards {
        let r = as_guest(id);
        assert_error_code(&r, "409 Conflict", "card_session_gone");
        assert!(r.body.contains("Nothing was saved") && r.body.contains("try Save again"), "{}", r.body);
        assert!(!r.body.contains("workspace"), "a guest can't keep it for the workspace: {}", r.body);
        assert!(!r.body.contains(SESSION_PW));
        assert_eq!(vault.request_get(id).expect("request").status, "pending", "the card stays open");
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l.contains(&format!("tried to save {name}")) && l.contains("still open")),
            "the agent is woken and told: {injects:?}"
        );
        assert!(injects.iter().all(|l| !l.contains(SESSION_PW)), "{injects:?}");
    }

    let chat = RunningChat::start(&project_id);
    let who = Requester::Session { workspace_id: project_id.clone(), session_id: Some(chat.sid()) };
    let origin = k2_vault::hosts::Origin::parse_url("https://api.example.com/").expect("url").1;
    for (name, id) in &cards {
        let set = as_guest(id);
        assert_eq!(set.status, "200 OK", "{name}: {}", set.body);
        assert!(!set.body.contains(SESSION_PW));
        let selector = Selector::user(k2_vault::Scope::session(chat.sid()), *name);
        let r = vault.resolve(&selector, Use::Fetch(&origin), &who).expect("the running session can use it");
        assert_eq!(r.value.expose(), SESSION_PW.as_bytes(), "{name}");
    }
}

fn guest_named(project_id: &str, name: &str) -> k2_core::skin::SkinPass {
    let mut p = person_pass(&[project_id.to_string()]);
    p.username = name.to_string();
    p
}

fn guest_saves(handle: &str, project_id: &str, name: &str, id: &str) -> CliResponse {
    with_request_skin(Some(guest_named(project_id, name)), || {
        post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id, "secret": SESSION_PW }))
    })
}

/// A guest named like a person on this box is still stamped a guest, and
/// Saving again and again wakes the agent at most once a minute, with the same
/// refusal every time.
#[cfg(unix)]
#[test]
fn a_refused_save_wakes_once_and_stamps_a_guest_as_a_guest() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlwk");
    let name = format!("WAKE_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let posted = post_json(
        "/cli/thread/secret",
        &params_of(&[("cell_session_id", "gone-session")]),
        serde_json::json!({ "addr": handle, "name": name, "for": "api.example.com", "from": "k2" }),
    );
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let first = guest_saves(&handle, &project_id, "rosson", &id);
    assert_error_code(&first, "409 Conflict", "card_session_gone");
    for _ in 0..4 {
        let again = guest_saves(&handle, &project_id, "rosson", &id);
        assert_eq!((again.status, &again.body), (first.status, &first.body), "the same answer each time");
    }
    let lines: Vec<String> = recorded_injects().into_iter().filter(|l| l.contains(&format!("tried to save {name}"))).collect();
    assert_eq!(lines.len(), 1, "one wake a minute for one card: {lines:?}");
    assert!(lines[0].starts_with("[guest rosson"), "{}", lines[0]);
    assert!(!lines[0].contains("[from rosson]"), "{}", lines[0]);
}

/// A wake can re-pin the chat: it moves the Thread with its cards to a new key
/// (a provider that picks its own id), or starts a new conversation and leaves
/// the card in the old Thread. Either way the same card saves on retry, for the
/// running session.
#[cfg(unix)]
#[test]
fn the_same_card_saves_after_a_wake_repins_the_chat() {
    use k2_vault::{Requester, Selector, Use};
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let origin = k2_vault::hosts::Origin::parse_url("https://api.example.com/").expect("url").1;
    for moved in [true, false] {
        let (handle, project_id) = ws("ovlrp");
        let old = pinned_chat_of(&project_id).expect("pinned");
        let name = if moved { "REPIN_MOVED" } else { "REPIN_NEW" };
        let posted = post_json(
            "/cli/thread/secret",
            &params_of(&[("cell_session_id", "gone-session")]),
            serde_json::json!({ "addr": handle, "name": name, "for": "api.example.com", "from": "k2" }),
        );
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let id = json_body(&posted)["id"].as_str().expect("id").to_string();
        assert_error_code(&guest_saves(&handle, &project_id, "gail", &id), "409 Conflict", "card_session_gone");

        let new = uuid::Uuid::new_v4().to_string();
        if moved {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::overlay::move_conversation(&conn, &old, &new).expect("move").expect("moved");
        }
        pin(&project_id, &new);
        let chat = RunningChat::start(&project_id);
        let set = guest_saves(&handle, &project_id, "gail", &id);
        assert_eq!(set.status, "200 OK", "moved={moved}: {}", set.body);
        assert_eq!(vault.request_get(&id).expect("request").status, "set", "moved={moved}");
        let who = Requester::Session { workspace_id: project_id.clone(), session_id: Some(chat.sid()) };
        let sel = Selector::user(k2_vault::Scope::session(chat.sid()), name);
        assert_eq!(vault.resolve(&sel, Use::Fetch(&origin), &who).expect("usable").value.expose(), SESSION_PW.as_bytes(), "moved={moved}");
    }
}

/// Following a re-pin never reaches another chat: a sidecar's card can't be
/// answered at the workspace's main address.
#[test]
fn a_sidecars_card_is_not_answered_at_the_main_address() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlsc");
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
            rusqlite::params![project_id, format!("pane-{conv}"), format!("tab-pane-{conv}"), conv],
        )
        .expect("tab");
    }
    let posted = post_json("/cli/thread/secret", &HashMap::new(), serde_json::json!({ "addr": conv, "name": "SIDE_KEY", "for": "api.example.com", "from": "k2" }));
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    assert_ne!(json_body(&posted)["conversation_id"], pinned_chat_of(&project_id).expect("pinned").as_str());
    let r = answer(&handle, &id, SESSION_PW, serde_json::json!({ "scope": "workspace" }));
    assert_ne!(r.status, "200 OK", "{}", r.body);
    assert_eq!(vault.request_get(&id).expect("request").status, "pending");
}

/// A sidecar's Thread: its own running session, of this workspace, while its
/// process lives. A dead child in the map receives nothing.
#[cfg(unix)]
#[test]
fn a_sidecars_running_session_receives_while_it_lives() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (_handle, project_id) = ws("ovlsl");
    let conv = uuid::Uuid::new_v4().to_string();
    let chat = RunningChat::start_as(&format!("tab-{conv}"));
    let resolved = ResolvedOverlay {
        conversation_id: conv.clone(),
        project_id: project_id.clone(),
        addr: "x/side".into(),
        requested_addr: "x/side".into(),
        canonical_alias: false,
    };
    assert_eq!(chat_running_session(&resolved), None, "no passport of this workspace");
    let mint = |ws: &str| {
        crate::session_token::mint_session_token(
            &chat.session.session_id,
            &chat.sid(),
            crate::session_token::HookPrincipal { workspace_uuid: ws.into(), agent_address: "side".into() },
            crate::session_token::CredMode::default(),
            crate::session_token::Provider::default(),
        );
    };
    mint("another-workspace");
    assert_eq!(chat_running_session(&resolved), None, "another workspace's session");
    mint(&project_id);
    assert_eq!(chat_running_session(&resolved), Some(chat.sid()));
    chat.session.kill();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while chat.session.is_child_alive() {
        assert!(std::time::Instant::now() < deadline, "the fake agent didn't die");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(chat_running_session(&resolved), None, "a dead child receives nothing");
}

/// The agent hears when a same-named workspace secret keeps the bare name, and
/// a guest's own answer and Thread view don't carry the session's id.
#[cfg(unix)]
#[test]
fn a_guests_save_tells_the_agent_about_the_name_and_shows_the_guest_no_session_id() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlgn");
    {
        use k2_vault::{Actor, Delivery, Kind, Namespace, PutRequest, Scope, SecretBytes};
        let mut r = PutRequest::new(Namespace::User, Scope::workspace(project_id.clone()), "STRIPE_KEY", Kind::ApiKey);
        r.hosts = vec!["api.example.com".into()];
        r.default_delivery = Some(Delivery::Fetch);
        vault.put(r, SecretBytes::from_string("OWNER".into()), &Actor::Owner).expect("put");
    }
    let chat = RunningChat::start(&project_id);
    let posted = post_json("/cli/thread/secret", &HashMap::new(), serde_json::json!({ "addr": handle, "name": "STRIPE_KEY", "for": "api.example.com", "from": "k2" }));
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let set = guest_saves(&handle, &project_id, "gail", &id);
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert!(!set.body.contains(&chat.sid()), "{}", set.body);
    assert!(json_body(&set).get("ref").is_none(), "{}", set.body);
    let snap = with_request_skin(Some(guest_named(&project_id, "gail")), || dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET"));
    assert!(!snap.body.contains(&chat.sid()), "the guest's Thread view: {}", snap.body);
    let injects = recorded_injects();
    let line = injects.iter().find(|l| l.contains("secret STRIPE_KEY set")).unwrap_or_else(|| panic!("{injects:?}"));
    assert!(line.contains("workspace secret") && line.contains("--scope session"), "{line}");
}

/// A share with a session: someone who may not share at all gets that refusal,
/// and learns nothing about which ids are running sessions.
#[test]
fn a_share_refused_on_permission_says_nothing_about_the_session() {
    use k2_vault::{Actor, AgentActor, Delivery, Kind, Namespace, PutRequest, Scope, SecretBytes};
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (_handle, project_id) = ws("ovlsp");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(project_id.clone()), "PERSON_KEY", Kind::Generic);
    r.default_delivery = Some(Delivery::Fetch);
    r.hosts = vec!["api.example.com".into()];
    r.delivery_ceiling = Some(Delivery::Env);
    r.human_given_override = Some(true);
    let id = vault.put(r, SecretBytes::from_slice(SESSION_PW.as_bytes()), &Actor::Owner).expect("put").meta.id;
    let agent = crate::vault::actor::VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: project_id.clone(), session_id: Some("s-agent".into()), shell: false, vault_manage: false }),
        channel: crate::vault::actor::Channel::Plain,
        session_id: Some("s-agent".into()),
    };
    let body = serde_json::json!({ "id": id, "to": "session:never-ran", "as": "file" }).to_string();
    let r = crate::vault_routes::handle_on(&vault, crate::vault_routes::GRANT, Ok(agent), body.as_bytes(), &HashMap::new(), "test", None);
    assert!(r.status.starts_with("403"), "{}", r.body);
    assert!(!r.body.contains("session_not_running"), "{}", r.body);
}

/// A person, too: asked by a session that has ended, Save reaches the chat's
/// running session, and a file arrives there. With nothing running they can
/// also keep it for the workspace: the words say so.
#[cfg(unix)]
#[test]
fn a_persons_save_reaches_the_chats_running_session() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlsn");
    let t = k2_vault::fsutil::TestDir::new("sess-next");
    let _run = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, t.path().join("run"));
    let asker = uuid::Uuid::new_v4().to_string();
    let dir = crate::vault::spawn::materialize(&mut HashMap::new(), &project_id, &asker, false, &crate::vault::spawn::SpawnRefs::default()).dir.expect("dir");
    let posted = post_as(&asker, &handle, "NEXT_FILE", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    crate::vault::spawn::session_ended(&asker);
    assert!(!dir.exists());

    let r = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_error_code(&r, "409 Conflict", "card_session_gone");
    assert!(r.body.contains("the card's options"), "{}", r.body);

    let chat = RunningChat::start(&project_id);
    let next = crate::vault::spawn::materialize(&mut HashMap::new(), &project_id, &chat.sid(), false, &crate::vault::spawn::SpawnRefs::default()).dir.expect("dir");
    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    let body = json_body(&set);
    assert_eq!(body["ref"], format!("k2v://session/{}/NEXT_FILE", chat.sid()), "{body}");
    assert_eq!(body["fileDelivered"], true, "{body}");
    assert_eq!(std::fs::read(next.join("NEXT_FILE")).expect("arrived"), SESSION_PW.as_bytes());
    crate::vault::spawn::session_ended(&chat.sid());
}

/// A card never claims a delivery that didn't happen: a file of that name is
/// already in the folder, and K2 doesn't overwrite it.
#[test]
fn a_file_already_in_the_folder_is_not_claimed_as_delivered() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlsp");
    let s = live_session("sess-present", &project_id);
    std::fs::write(s.dir.join("STALE_FILE"), b"earlier").expect("earlier file");
    let posted = post_as(&s.sid, &handle, "STALE_FILE", "workspace", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();

    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert_eq!(json_body(&set)["fileDelivered"], false, "{}", set.body);
    assert_eq!(std::fs::read(s.dir.join("STALE_FILE")).expect("file"), b"earlier", "never overwritten");
    let injects = recorded_injects();
    let line = injects.iter().find(|l| l.contains("secret STALE_FILE set")).unwrap_or_else(|| panic!("{injects:?}"));
    assert!(line.contains("NOT delivered: $K2_SECRETS_DIR/STALE_FILE already exists"), "{line}");
    crate::vault::spawn::session_ended(&s.sid);
}

/// The vault's minute pass, as the daemon runs it.
fn vault_tick(vault: &k2_vault::LocalVault) {
    crate::vault::spawn::follow_and_top_up(vault);
}

/// A daemon restart without k2-ptyd ends the chat's session, and the chat comes
/// back with a new session id. A secret a card saved for the chat's session
/// follows it there until it expires: the file arrives, a bare name reaches
/// it, and a destroy or a removed share still takes it back for good.
#[cfg(unix)]
#[test]
fn a_session_card_follows_its_chat_to_the_session_after_a_restart() {
    use k2_vault::{Actor, Selector};
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlfr");
    let t = k2_vault::fsutil::TestDir::new("sess-follow");
    let _run = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, t.path().join("run"));
    let spawn = |sid: &str| {
        crate::vault::spawn::materialize(&mut HashMap::new(), &project_id, sid, false, &crate::vault::spawn::SpawnRefs::default()).dir.expect("dir")
    };
    let first = RunningChat::start(&project_id);
    let dir1 = spawn(&first.sid());
    let posted = post_as(&first.sid(), &handle, "FOLLOW_FILE", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert_eq!(std::fs::read(dir1.join("FOLLOW_FILE")).expect("delivered"), SESSION_PW.as_bytes());
    let secret = vault.request_get(&id).expect("request").secret_id.expect("saved");
    let sel = Selector::Id(secret.clone());
    let meta = vault.meta(&sel, &Actor::Owner).expect("meta");

    // The daemon restarts: the session ends, the chat runs again as another.
    let first_sid = first.sid();
    crate::vault::spawn::session_ended(&first_sid);
    drop(first);
    let second = RunningChat::start(&project_id);
    let dir2 = spawn(&second.sid());
    vault_tick(&vault);
    assert_eq!(std::fs::read(dir2.join("FOLLOW_FILE")).expect("it followed the chat"), SESSION_PW.as_bytes());
    let to_second: Vec<_> = vault.grants(&sel, &Actor::Owner).expect("grants").into_iter().filter(|g| g.consumer.id == second.sid()).collect();
    assert_eq!(to_second.len(), 1, "one share to the new session");
    assert_eq!(to_second[0].expires_at, meta.expires_at, "it ends when the secret does");
    vault_tick(&vault);
    assert_eq!(vault.grants(&sel, &Actor::Owner).expect("grants").len(), 1, "no second share on the next pass");

    // A bare name reaches it from the new session.
    let me = crate::vault::actor::VaultCaller {
        actor: Actor::Agent(k2_vault::AgentActor { workspace_id: project_id.clone(), session_id: Some(second.sid()), shell: false, vault_manage: false }),
        channel: crate::vault::actor::Channel::Plain,
        session_id: Some(second.sid()),
    };
    let file = crate::vault_routes::handle_on(
        &vault,
        crate::vault_use_routes::FILE,
        Ok(me.clone()),
        serde_json::json!({ "secret": "FOLLOW_FILE" }).to_string().as_bytes(),
        &HashMap::new(),
        "test",
        None,
    );
    assert_eq!(file.status, "200 OK", "{}", file.body);

    // A removed share stays removed: the next pass doesn't put it back.
    let owner = crate::vault::actor::VaultCaller { actor: Actor::Owner, channel: crate::vault::actor::Channel::Plain, session_id: None };
    let call = |path: &str, body: serde_json::Value| {
        let r = crate::vault_routes::handle_on(&vault, path, Ok(owner.clone()), body.to_string().as_bytes(), &HashMap::new(), "test", None);
        assert_eq!(r.status, "200 OK", "{path}: {}", r.body);
    };
    call(crate::vault_routes::REVOKE, serde_json::json!({ "grantId": to_second[0].id }));
    assert!(!dir2.join("FOLLOW_FILE").exists(), "revoke took the file back");
    vault_tick(&vault);
    assert!(vault.grants(&sel, &Actor::Owner).expect("grants").is_empty(), "a removed share stays removed");
    assert!(!dir2.join("FOLLOW_FILE").exists());

    // The chat runs again: it follows there, and a destroy takes it back for good.
    crate::vault::spawn::session_ended(&second.sid());
    drop(second);
    let third = RunningChat::start(&project_id);
    let dir3 = spawn(&third.sid());
    vault_tick(&vault);
    assert!(dir3.join("FOLLOW_FILE").exists(), "the next session of the chat gets it");
    call(crate::vault_routes::DESTROY, serde_json::json!({ "id": secret, "yes": true }));
    assert!(!dir3.join("FOLLOW_FILE").exists(), "destroy took it back");
    vault_tick(&vault);
    assert!(!dir3.join("FOLLOW_FILE").exists(), "a destroyed secret never follows again");
    crate::vault::spawn::session_ended(&third.sid());
}

/// It follows only its own chat: another workspace's running chat, and a
/// chat running again in the same session, get nothing new.
#[cfg(unix)]
#[test]
fn a_session_card_follows_only_its_own_chat() {
    use k2_vault::{Actor, Selector};
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlfo");
    let (_other_handle, other_project) = ws("ovlfx");
    let t = k2_vault::fsutil::TestDir::new("sess-follow-own");
    let _run = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, t.path().join("run"));
    let first = RunningChat::start(&project_id);
    crate::vault::spawn::materialize(&mut HashMap::new(), &project_id, &first.sid(), false, &crate::vault::spawn::SpawnRefs::default());
    let posted = post_as(&first.sid(), &handle, "OWN_FILE", "session", "file");
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    assert_eq!(answer(&handle, &id, SESSION_PW, serde_json::json!({})).status, "200 OK");
    let sel = Selector::Id(vault.request_get(&id).expect("request").secret_id.expect("saved"));
    vault_tick(&vault);
    assert!(vault.grants(&sel, &Actor::Owner).expect("grants").is_empty(), "its own session still runs: nothing to follow");
    crate::vault::spawn::session_ended(&first.sid());
    drop(first);
    let elsewhere = RunningChat::start(&other_project);
    vault_tick(&vault);
    assert!(vault.grants(&sel, &Actor::Owner).expect("grants").is_empty(), "another workspace's chat gets nothing");
    drop(elsewhere);
}

/// X3 (prd-vault-for-agents-v1 §5.1): `ws:<name>` on a card is this
/// workspace's scope when it names this workspace (handle, id, or a name
/// that resolves to it); any other name is refused in words that don't say
/// whether it exists; the other scope words pass through.
#[test]
fn card_scope_word_takes_this_workspace_only() {
    let resolve = |w: &str| (w == "ops-folder").then(|| "p-1".to_string());
    for raw in ["ws:ops", "ws:p-1", "workspace:ops", "ws: ops ", "ws:ops-folder"] {
        assert_eq!(super::super::card_scope_word(raw, "p-1", "ops", resolve).expect(raw), "workspace", "{raw}");
    }
    for raw in ["workspace", "server", "session"] {
        assert_eq!(super::super::card_scope_word(raw, "p-1", "ops", resolve).expect(raw), raw);
    }
    let e = super::super::card_scope_word("ws:sales", "p-1", "ops", resolve).expect_err("another workspace");
    assert_eq!((e.code, e.status), ("bad_scope", 400), "{e}");
    assert!(e.hint.contains("(ops)") && e.hint.contains("k2 vault send NAME --to sales"), "{}", e.hint);
}

/// X3 + X13 through the route: a card asks with `--scope ws:<this
/// workspace>` and `--kind ssh_key --delivery agent`; the request keeps
/// `workspace` and `agent`, the agent's line says ssh and git sign with it,
/// and Save stores an ssh key K2 serves on $SSH_AUTH_SOCK. Another
/// workspace's name, and agent for a key that isn't an ssh key, are refused
/// when the agent asks.
#[test]
fn a_card_takes_ws_scope_and_asks_for_an_ssh_key_by_agent_delivery() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlx3");
    let (other_handle, _other_project) = ws("ovlx3o");
    let ask = serde_json::json!({ "for": "", "kind": "ssh_key", "delivery": "agent", "scope": format!("ws:{handle}") });
    let posted = post_card(&handle, "DEPLOY_KEY", ask);
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let body = json_body(&posted);
    assert_eq!(
        body["sends"], "ssh and git sign with DEPLOY_KEY on $SSH_AUTH_SOCK in your new sessions (you never see the key)",
        "{body}"
    );
    let id = body["id"].as_str().expect("id").to_string();
    let req = vault.request_get(&id).expect("request");
    assert_eq!((req.proposed_scope.as_str(), req.proposed_delivery), ("workspace", k2_vault::Delivery::Agent));
    let doc = card_doc(&handle, &id);
    assert_eq!((doc["delivery"].as_str(), doc["scope"].as_str()), (Some("agent"), Some("workspace")), "{doc}");
    let key = k2_vault::ssh_agent::testkeys::ed25519();
    let saved = answer(&handle, &id, &key, serde_json::json!({}));
    assert_eq!(saved.status, "200 OK", "{}", saved.body);
    assert!(!saved.body.contains("PRIVATE KEY"), "{}", saved.body);
    let m = vault.meta(&sel(&project_id, "DEPLOY_KEY"), &k2_vault::Actor::Owner).expect("saved");
    assert_eq!((m.kind, m.default_delivery), (k2_vault::Kind::SshKey, k2_vault::Delivery::Agent));

    let other = post_card(&handle, "K2", serde_json::json!({ "scope": format!("ws:{other_handle}") }));
    assert_error_code(&other, "400 Bad Request", "bad_scope");
    let not_a_key = post_card(&handle, "K3", serde_json::json!({ "for": "", "delivery": "agent" }));
    assert_error_code(&not_a_key, "400 Bad Request", "delivery_not_for_kind");
    let bad = post_card(&handle, "K4", serde_json::json!({ "delivery": "carrier-pigeon" }));
    assert_error_code(&bad, "400 Bad Request", "bad_delivery");
    // A3 (V2): a card may propose `proxy`, and the agent's card keeps it.
    let proxied = post_card(&handle, "K5", serde_json::json!({ "delivery": "proxy" }));
    assert_eq!(proxied.status, "200 OK", "{}", proxied.body);
}

/// X13: an ssh key by agent delivery can't be kept for one session (it
/// reaches new sessions only); the agent hears so when it asks.
#[cfg(unix)]
#[test]
fn an_agent_delivery_card_for_one_session_is_refused_when_asked() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlx13");
    let chat = RunningChat::start(&project_id);
    let posted = post_json(
        "/cli/thread/secret",
        &params_of(&[("cell_session_id", &chat.sid())]),
        serde_json::json!({ "addr": handle, "name": "SESS_KEY", "scope": "session", "kind": "ssh_key", "delivery": "agent", "from": "k2" }),
    );
    assert_error_code(&posted, "400 Bad Request", "session_agent_unreachable");
    drop(chat);
}

/// X4: a card's `--ttl` is the shared duration grammar (k2_vault::duration):
/// a bad one is the vault's `bad_ttl`, and nothing is posted.
#[test]
fn a_card_ttl_uses_the_shared_grammar() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (handle, _project_id) = ws("ovlttl");
    let posted = post_card(&handle, "TTL_KEY", serde_json::json!({ "scope": "workspace", "ttl": "2w" }));
    assert_error_code(&posted, "400 Bad Request", "bad_ttl");
    assert!(json_body(&posted)["error"]["hint"].as_str().expect("hint").starts_with("--ttl: a duration is like 90s, 10m, 2h or 7d"), "{}", posted.body);
}

/// The S1 amendment: a share that followed the chat keeps the secret's own
/// time. A later card that moved the secret's time moves the share's too; a
/// share that ran out doesn't block a new one.
#[cfg(unix)]
#[test]
fn a_follow_share_takes_the_secrets_time() {
    use k2_vault::{Actor, Consumer, Delivery, GrantRequest, Kind, SecretBytes, Selector};
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlft");
    let t = k2_vault::fsutil::TestDir::new("sess-follow-ttl");
    let _run = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, t.path().join("run"));
    let spawn = |sid: &str| {
        crate::vault::spawn::materialize(&mut HashMap::new(), &project_id, sid, false, &crate::vault::spawn::SpawnRefs::default()).dir.expect("dir")
    };
    let first = RunningChat::start(&project_id);
    spawn(&first.sid());
    let posted = post_as(&first.sid(), &handle, "FOLLOW_TTL", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    let secret = vault.request_get(&id).expect("request").secret_id.expect("saved");
    let sel = Selector::Id(secret.clone());
    let first_sid = first.sid();
    crate::vault::spawn::session_ended(&first_sid);
    drop(first);
    let second = RunningChat::start(&project_id);
    spawn(&second.sid());
    vault_tick(&vault);
    let second_sid = second.sid();
    let share = |vault: &k2_vault::LocalVault| -> Vec<k2_vault::Grant> {
        vault.grants(&sel, &Actor::Owner).expect("grants").into_iter().filter(|g| g.consumer.id == second_sid).collect()
    };
    let before = vault.meta(&sel, &Actor::Owner).expect("meta").expires_at;
    assert_eq!(share(&vault)[0].expires_at, before, "the share ends when the secret does");

    // A later card for that session moves the secret's time.
    let card2 = format!("card-{}", uuid::Uuid::new_v4());
    vault
        .request_create(k2_vault::vault::SecretRequest {
            card_id: card2.clone(),
            conversation: "conv-later".into(),
            session_id: Some(first_sid.clone()),
            workspace_id: project_id.clone(),
            requested_by: "k2".into(),
            name: "FOLLOW_TTL".into(),
            kind: Kind::ApiKey,
            proposed_scope: "session".into(),
            proposed_hosts: vec![],
            proposed_delivery: Delivery::File,
            prompt: String::new(),
            ttl_secs: Some(90_000),
            status: String::new(),
            answered_by: None,
            secret_id: None,
            created_at: String::new(),
            answered_at: None,
            inject: None,
            once: false,
        })
        .expect("card");
    let ans = k2_vault::vault::CardAnswer { session_id: Some(first_sid.clone()), readable_disclosed: true, ..Default::default() };
    vault.request_answer(&card2, SecretBytes::from_string(SESSION_PW.to_string()), &ans, &Actor::Owner).expect("saved");
    let after = vault.meta(&sel, &Actor::Owner).expect("meta").expires_at;
    assert_ne!(after, before, "the second card moved the secret's time");
    vault_tick(&vault);
    let shares = share(&vault);
    assert_eq!(shares.len(), 1, "the one share moved, not a second one");
    assert_eq!(shares[0].expires_at, after, "the share moved with the secret");

    // A share that ran out doesn't block a new one.
    let expired = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    vault
        .grant(
            GrantRequest {
                selector: sel.clone(),
                consumer: Consumer::session(&second_sid),
                delivery: Delivery::File,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: Some(expired),
                version: None,
            },
            &Actor::System("card-follows-chat".into()),
        )
        .expect("an expired share");
    vault_tick(&vault);
    let shares = share(&vault);
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].expires_at, after, "an expired follow share is replaced");
    crate::vault::spawn::session_ended(&second.sid());
}

fn agent_of(project_id: &str, sid: &str) -> crate::vault::actor::VaultCaller {
    crate::vault::actor::VaultCaller {
        actor: k2_vault::Actor::Agent(k2_vault::AgentActor { workspace_id: project_id.to_string(), session_id: Some(sid.to_string()), shell: false, vault_manage: false }),
        channel: crate::vault::actor::Channel::Plain,
        session_id: Some(sid.to_string()),
    }
}

fn use_file(vault: &std::sync::Arc<k2_vault::LocalVault>, me: &crate::vault::actor::VaultCaller, body: serde_json::Value) -> CliResponse {
    crate::vault_routes::handle_on(vault, crate::vault_use_routes::FILE, Ok(me.clone()), body.to_string().as_bytes(), &HashMap::new(), "test", None)
}

/// §3.5: `--once`, end to end through the routes. Not in the session's
/// folder after Save; the first `k2 vault file` works, the second is
/// not_found; the Save line and the card both say so.
#[test]
fn a_once_card_reads_once_through_the_routes() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlon");
    let s = live_session("sess-once", &project_id);
    let posted = post_json(
        "/cli/thread/secret",
        &params_of(&[("cell_session_id", s.sid.as_str())]),
        serde_json::json!({ "addr": handle, "name": "ONCE_CODE", "scope": "session", "delivery": "file", "once": true, "from": "k2" }),
    );
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let body = json_body(&posted);
    assert_eq!(body["sends"], "ONCE_CODE goes to you once: read it with k2 vault file ONCE_CODE (used once, then deleted)", "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    assert_eq!(card_doc(&handle, &id)["once"], true);
    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert!(!set.body.contains(SESSION_PW), "the value isn't in the answer");
    assert!(json_body(&set)["line"].as_str().is_none_or(|l| !l.contains(SESSION_PW)));
    let injects = recorded_injects();
    assert!(injects.iter().any(|l| l.contains("Use: k2 vault file ONCE_CODE (once; then it is deleted)")), "{injects:?}");
    assert!(!injects.iter().any(|l| l.contains(SESSION_PW)), "the value never reaches the agent's line");
    assert!(!s.dir.join("ONCE_CODE").exists(), "never in $K2_SECRETS_DIR, not even at Save");

    let me = agent_of(&project_id, &s.sid);
    let first = use_file(&vault, &me, serde_json::json!({ "secret": "ONCE_CODE" }));
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let path = json_body(&first)["path"].as_str().expect("path").to_string();
    assert_eq!(std::fs::read(&path).expect("use file"), SESSION_PW.as_bytes());
    let second = use_file(&vault, &me, serde_json::json!({ "secret": "ONCE_CODE" }));
    assert!(second.status.starts_with("404"), "the second read is gone: {}", second.body);
    let scoped = use_file(&vault, &me, serde_json::json!({ "secret": "ONCE_CODE", "scope": "session" }));
    assert!(scoped.status.starts_with("404"), "gone with --scope session too: {}", scoped.body);
    crate::vault::spawn::session_ended(&s.sid);
}

/// §3.5: `--once` is refused at ask time unless it is session + file.
#[test]
fn a_once_card_is_refused_unless_session_and_file() {
    let _home = crate::test_support::TempHome::new();
    let _vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlof");
    let s = live_session("sess-once-f", &project_id);
    for (scope, delivery) in [("session", "fetch"), ("workspace", "file")] {
        let r = post_json(
            "/cli/thread/secret",
            &params_of(&[("cell_session_id", s.sid.as_str())]),
            serde_json::json!({ "addr": handle, "name": "ONCE_BAD", "scope": scope, "delivery": delivery, "once": true, "for": "api.example.com", "from": "k2" }),
        );
        assert_error_code(&r, "400 Bad Request", "once_needs_session_file");
        assert!(r.body.contains("--scope session --delivery file"), "{}", r.body);
    }
    crate::vault::spawn::session_ended(&s.sid);
}

/// §3.4: `--wait` returns the path when a person saves mid-wait.
#[test]
fn wait_returns_the_path_when_the_card_is_saved() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlw1");
    let s = live_session("sess-wait", &project_id);
    let posted = post_as(&s.sid, &handle, "WAIT_CODE", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let me = agent_of(&project_id, &s.sid);
    let (v2, me2) = (vault.clone(), me.clone());
    let started = std::time::Instant::now();
    let waiter = std::thread::spawn(move || use_file(&v2, &me2, serde_json::json!({ "secret": "WAIT_CODE", "wait": "8s" })));
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(!waiter.is_finished(), "still waiting for the person");
    let set = answer(&handle, &id, SESSION_PW, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    let r = waiter.join().expect("join");
    assert!(started.elapsed() < std::time::Duration::from_secs(6), "the save wakes the waiter; it doesn't run out its time");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let path = json_body(&r)["path"].as_str().expect("path").to_string();
    assert_eq!(std::fs::read(&path).expect("the file"), SESSION_PW.as_bytes());
    crate::vault::spawn::session_ended(&s.sid);
}

/// §3.4: with no card asking, `--wait` refuses at once (exit 4); no card
/// is found for another session's name either.
#[test]
fn wait_with_no_card_is_not_found_at_once() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (_handle, project_id) = ws("ovlw2");
    let s = live_session("sess-wait-nc", &project_id);
    let me = agent_of(&project_id, &s.sid);
    let r = use_file(&vault, &me, serde_json::json!({ "secret": "NOTHING_ASKS", "wait": "5s" }));
    assert_error_code(&r, "404 Not Found", "no_card");
    assert!(r.body.contains("no card asks for NOTHING_ASKS"), "{}", r.body);
    assert!(r.body.contains("k2 thread secret"), "{}", r.body);
    crate::vault::spawn::session_ended(&s.sid);
}

/// §3.4: a person dismisses the card (exit 3); a call that follows a card
/// the person already dismissed hears that too; the time-out answer names
/// the card; a ninth waiter is refused.
#[test]
fn wait_ends_on_a_declined_card_and_limits_waiters() {
    use k2_vault::Actor;
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlw3");
    let s = live_session("sess-wait-d", &project_id);
    let posted = post_as(&s.sid, &handle, "WAIT_NO", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let me = agent_of(&project_id, &s.sid);

    // The time-out answer: still waiting, on this card, with the whole budget.
    let r = use_file(&vault, &me, serde_json::json!({ "secret": "WAIT_NO", "wait": "1s" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_body(&r);
    assert_eq!(v["waiting"], true, "{v}");
    assert_eq!(v["card"], id, "{v}");
    assert_eq!(v["waitSecs"], 1, "{v}");

    // A ninth waiter gets a 409, not a blocked thread.
    let held = crate::vault_use_routes::hold_wait_slots_for_tests(&s.sid, 8);
    let r = use_file(&vault, &me, serde_json::json!({ "secret": "WAIT_NO", "wait": "1s" }));
    assert_error_code(&r, "409 Conflict", "too_many_waits");
    drop(held);

    let (v2, me2) = (vault.clone(), me.clone());
    let waiter = std::thread::spawn(move || use_file(&v2, &me2, serde_json::json!({ "secret": "WAIT_NO", "wait": "5s" })));
    std::thread::sleep(std::time::Duration::from_millis(150));
    vault.request_void(&id, &Actor::System("thread".into())).expect("dismissed");
    let r = waiter.join().expect("join");
    assert_error_code(&r, "403 Forbidden", "card_declined");

    // The next CLI call in the loop names the card.
    let r = use_file(&vault, &me, serde_json::json!({ "secret": "WAIT_NO", "wait": "1s", "waitCard": id }));
    assert_error_code(&r, "403 Forbidden", "card_declined");
    crate::vault::spawn::session_ended(&s.sid);
}

/// prd-popup-close-and-card-dismiss-v1 B5: a person dismissing the card a
/// `k2 vault file --wait` is waiting on ends the wait with 410
/// `card_dismissed` (the CLI's exit 5), naming who and why; a card the
/// asking agent withdrew ends it with 410 `card_withdrawn`. Nothing saved.
#[test]
fn wait_ends_on_a_dismissed_or_withdrawn_card_with_its_own_code() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlwd");
    let s = live_session("sess-wait-dm", &project_id);
    let posted = post_as(&s.sid, &handle, "WAIT_DISMISS", "session", "file");
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let id = json_body(&posted)["id"].as_str().expect("id").to_string();
    let me = agent_of(&project_id, &s.sid);
    let (v2, me2) = (vault.clone(), me.clone());
    let waiter = std::thread::spawn(move || use_file(&v2, &me2, serde_json::json!({ "secret": "WAIT_DISMISS", "wait": "8s" })));
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(!waiter.is_finished(), "still waiting for the person");
    let d = post_json("/cli/thread/void", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id, "reason": "Using SSO now" }));
    assert_eq!(d.status, "200 OK", "{}", d.body);
    let r = waiter.join().expect("join");
    assert_error_code(&r, "410 Gone", "card_dismissed");
    let hint = json_body(&r)["error"]["hint"].as_str().expect("hint").to_string();
    assert!(hint.contains(&id) && hint.contains("\"Using SSO now\"") && hint.contains("nothing was saved"), "{hint}");
    assert_eq!(vault.request_get(&id).expect("req").status, "void");

    // Withdrawn by the asking agent (its passport and session).
    let posted = post_json(
        "/cli/thread/secret",
        &params_of(&[
            (crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"),
            ("project_id", project_id.as_str()),
            ("from", handle.as_str()),
            ("cell_session_id", s.sid.as_str()),
        ]),
        serde_json::json!({ "addr": handle, "name": "WAIT_WITHDRAW", "scope": "session", "delivery": "file", "from": handle }),
    );
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let wid = json_body(&posted)["id"].as_str().expect("id").to_string();
    let w = post_json(
        "/cli/thread/void",
        &params_of(&[(crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"), ("project_id", project_id.as_str()), ("from", handle.as_str())]),
        serde_json::json!({ "addr": handle, "id": wid, "why": "Found it in the repo" }),
    );
    assert_eq!(w.status, "200 OK", "{}", w.body);
    assert_eq!(json_body(&w)["closedAs"], "withdrawn", "{}", w.body);
    let r = use_file(&vault, &me, serde_json::json!({ "secret": "WAIT_WITHDRAW", "wait": "1s", "waitCard": wid }));
    assert_error_code(&r, "410 Gone", "card_withdrawn");
    crate::vault::spawn::session_ended(&s.sid);
}
