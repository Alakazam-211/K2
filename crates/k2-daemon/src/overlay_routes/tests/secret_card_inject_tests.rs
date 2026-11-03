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
    assert!(line.contains("even with --resume"), "the agent hears it ends with the session: {line}");
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
