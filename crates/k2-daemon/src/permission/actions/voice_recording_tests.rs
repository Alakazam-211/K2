use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, id_of, login, seed};
use super::*;
use crate::session_events::SessionEvent;
use k2_core::tool_hook::{self as hook, CHAT_KIND};

const SPACE: &str = "7001";

struct Env {
    _home: crate::test_support::TempHome,
    vault: std::sync::Arc<k2_vault::LocalVault>,
}

fn env() -> Env {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let vault = crate::vault::ensure_test_vault();
    hook::allow_target_off_linux_for_tests(true);
    hook::set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("chat target");
    Env { _home: home, vault }
}

fn level_now() -> Option<String> {
    hook::recording_space(SPACE).expect("the K5 Space").0
}

/// The logins `access_changed` went to since `rx` subscribed.
fn told(rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let SessionEvent::AccessChanged { login } = ev {
            out.push(login);
        }
    }
    out
}

fn code_of(r: &crate::cli_response::CliResponse) -> (String, String) {
    let v: Value = serde_json::from_str(&r.body).expect("json");
    (r.status.to_string(), v["error"]["code"].as_str().unwrap_or("").to_string())
}

/// Both cards: Admin floor, one time, each with its own confirm, valid
/// against the catalog, and walled where the agent is refused.
#[test]
fn the_cards_are_an_admins_once_with_their_own_confirms() {
    let _db = k2_core::db::scoped_for_test();
    for (spec, action, confirm, route) in [
        (&START_LEVEL, START_ACTION, ConfirmAction::VoiceRecordingStartLevel, START_ROUTE),
        (&DM, DM_ACTION, ConfirmAction::VoiceRecordingDm, DM_ROUTE),
    ] {
        let r = super::super::find(action).expect("registered");
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((spec.floor, spec.grant), (Floor::Admin, Grant::OneTime), "{action}");
        assert_eq!(spec.proof, Proof::SettingsConfirm { action: confirm }, "{action}");
        assert_eq!(spec.walls[0].route_prefix, route, "{action}");
        assert!(crate::permission::walls::V1_ACTIONS.contains(&action), "{action}");
    }
    assert_eq!(ConfirmAction::VoiceRecordingStartLevel.as_str(), START_CONFIRM);
    assert_eq!(ConfirmAction::VoiceRecordingDm.as_str(), DM_CONFIRM);
    assert_eq!(START_LEVEL.walls[0].matches, WallMatch::Code { status: 403, code: "recording_level_needs_owner" });
}

/// The start level's own refusal: an agent (or a Member) changing only the
/// start level, to a level the card can ask for. `--require-recording`, an
/// inherit and an admin level keep the shared refusal (no card).
#[test]
fn only_a_plain_start_level_change_is_the_cards() {
    let _db = k2_core::db::scoped_for_test();
    let _e = env();
    let ask_as = |c: &Caller, b: Value| code_of(&crate::voice::routes::recording_settings_as(c, &b));
    let mine = ("403 Forbidden".to_string(), "recording_level_needs_owner".to_string());
    let shared = ("403 Forbidden".to_string(), "owner_or_admin_only".to_string());
    for l in ["none", "moderator", "member"] {
        assert_eq!(ask_as(&Caller::Passport, json!({ "startLevel": l })), mine, "{l}");
    }
    let member = Caller::Login { username: "mo".into(), role: Role::Member, token: String::new() };
    assert_eq!(ask_as(&member, json!({ "startLevel": "member" })), mine);
    assert_eq!(ask_as(&Caller::ChatManager, json!({ "startLevel": "member" })), mine);
    assert_eq!(ask_as(&Caller::Passport, json!({ "startLevel": "member", "required": true })), shared, "required is the owner's");
    assert_eq!(ask_as(&Caller::Passport, json!({ "required": true })), shared);
    assert_eq!(ask_as(&Caller::Passport, json!({ "startLevel": null })), shared, "inherit");
    assert_eq!(ask_as(&Caller::Passport, json!({ "startLevel": "admin" })), shared, "no admin level in the K5 Space");
    assert_ne!(ask_as(&Caller::Invalid, json!({ "startLevel": "member" })), mine, "a bad token gets its own refusal first");
}

/// Rosson's card: a Member's Allow is refused and a Deny changes nothing;
/// an Admin's Allow (a confirm bound to the request) sets the level as
/// them; then `access_changed` goes to each login whose right to start a
/// recording changed, and to no one else.
#[test]
fn a_person_sets_the_start_level_and_those_it_changes_hear_it() {
    let _db = k2_core::db::scoped_for_test();
    let e = env();
    let ws = seed("lvl");
    let (s, v) = ask(&ws, START_ACTION, json!({ "level": "member" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("sew_feature_missing")), "Sew 0.8.x: {v}");
    hook::note_features_for_tests(&["app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"]).expect("flags");
    hook::admin_space_ready_for_tests(SPACE).expect("the K5 Space");
    assert_eq!(level_now(), None);

    let member = login("lvm", Role::Member);
    let admin = login("lva", Role::Admin);
    let asked = ask_ok(&ws, START_ACTION, json!({ "level": "member" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = super::super::held::tests::view(&ws, &card);
    assert!(vw.to_string().contains("Let every member start recordings in this server's chat"), "{vw}");

    // A Member's Allow: refused below the floor, nothing changes.
    let tok = e.vault.confirm.mint(START_CONFIRM, Some(&id), &format!("connect:{member}"), "local-auth");
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow", "confirmToken": tok })), "admin");
    assert_eq!(level_now(), None, "a Member changes nothing");

    // A Deny changes nothing.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(level_now(), None, "a Deny changes nothing");

    // An Admin's Allow (another level: the denied one waits a day): the
    // level, as them; from inherit, everyone's right was unknown.
    let asked = ask_ok(&ws, START_ACTION, json!({ "level": "moderator" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let mut rx = crate::session_events::subscribe();
    let tok = e.vault.confirm.mint(START_CONFIRM, Some(&id), &format!("connect:{admin}"), "local-auth");
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": tok }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(level_now().as_deref(), Some("moderator"));
    let heard = told(&mut rx);
    assert!(heard.contains(&member) && heard.contains(&admin), "inherit to moderator: {heard:?}");

    // Moderator to none: the Admin's right goes, the Member had none.
    let asked = ask_ok(&ws, START_ACTION, json!({ "level": "none" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let mut rx = crate::session_events::subscribe();
    let tok = e.vault.confirm.mint(START_CONFIRM, Some(&id), "owner", "local-auth");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": tok }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(level_now().as_deref(), Some("none"));
    let heard = told(&mut rx);
    assert!(heard.contains(&admin), "{heard:?}");
    assert!(!heard.contains(&member), "a Member could start none either way: {heard:?}");

    let (s, again) = ask(&ws, START_ACTION, json!({ "level": "none" }));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
}

/// The dm card: a Member's Allow is refused, a Deny (of dm=off) changes
/// nothing, the
/// owner's Allow turns `recording.dm_enabled` on through the policy route
/// (its own confirm, minted for the same person), and every login hears
/// `access_changed`.
#[test]
fn a_person_turns_dm_recording_on() {
    let _db = k2_core::db::scoped_for_test();
    let e = env();
    let ws = seed("dmr");
    hook::note_features_for_tests(&["app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"]).expect("flags");
    let member = login("dmm", Role::Member);
    let asked = ask_ok(&ws, DM_ACTION, json!({ "dm": "off" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let tok = e.vault.confirm.mint(DM_CONFIRM, Some(&id), &format!("connect:{member}"), "local-auth");
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow", "confirmToken": tok })), "admin");
    assert_eq!(dm_now(), None, "a Member changes nothing");
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK");
    assert_eq!(dm_now(), None, "a Deny changes nothing");

    let asked = ask_ok(&ws, DM_ACTION, json!({ "dm": "on" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let mut rx = crate::session_events::subscribe();
    let tok = e.vault.confirm.mint(DM_CONFIRM, Some(&id), "owner", "local-auth");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": tok }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(dm_now(), Some(true));
    let rows = hook::recording_policies().expect("policies");
    let others: Vec<_> = rows.iter().filter(|r| r.key != wire::POLICY_DM_ENABLED && r.want.is_some()).map(|r| r.key.clone()).collect();
    assert!(others.is_empty(), "only dm_enabled changed: {others:?}");
    assert!(told(&mut rx).contains(&member));
    let (s, again) = ask(&ws, DM_ACTION, json!({ "dm": "on" }));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
}

/// Whose right to start a recording changed.
#[test]
fn the_logins_told_are_those_whose_right_changed() {
    let rows = vec![
        ("1".to_string(), "mem".to_string(), Role::Member, false),
        ("2".to_string(), "adm".to_string(), Role::Admin, false),
        ("3".to_string(), "own".to_string(), Role::Owner, false),
        ("4".to_string(), "vc".to_string(), Role::VoiceChat, false),
        ("5".to_string(), "off".to_string(), Role::Member, true),
    ];
    assert_eq!(start_right_changed(Some("member"), Some("moderator"), &rows), ["mem"]);
    assert_eq!(start_right_changed(Some("moderator"), Some("member"), &rows), ["mem"]);
    assert_eq!(start_right_changed(Some("none"), Some("moderator"), &rows), ["adm", "own"]);
    assert_eq!(start_right_changed(Some("none"), Some("member"), &rows), ["mem", "adm", "own"]);
    assert!(start_right_changed(Some("member"), Some("member"), &rows).is_empty());
    assert_eq!(start_right_changed(None, Some("none"), &rows), ["mem", "adm", "own"], "inherit is unknown: every live login that may record");
    assert_eq!(level_words("none"), "nobody");
    assert_eq!(level_words("moderator"), "moderators and above");
    assert_eq!(level_words("member"), "every member");
}

/// HR-12, with Sew 0.11.3's answer: Sew has no person-only step anywhere in
/// recording, so K2's person-present vault confirm is the gate on the two
/// switches that really turn recording on, `recording.enabled` and
/// `recording.dm_enabled`. Without a fresh confirm the owner token and an
/// Admin are refused `owner_confirm_required`, a Member `owner_only`, an
/// agent or a K2 terminal `person_only`; a confirm minted for another
/// action is refused too, and nothing is stored. With the person's confirm
/// it takes, once: the same confirm again is refused.
#[test]
fn the_two_on_switches_are_refused_without_a_persons_confirm() {
    let _db = k2_core::db::scoped_for_test();
    let e = env();
    hook::note_features_for_tests(&["app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"]).expect("flags");
    let install = crate::sew::routes::confirm_secret_id();
    let send = |who: Who, body: Value| code_of(&crate::sew::routes::handle(DM_ROUTE, who, body.to_string().as_bytes(), &HashMap::new()));
    let want = |key: &str| -> Option<Value> {
        hook::recording_policies().expect("policies").into_iter().find(|r| r.key == key).and_then(|r| r.want)
    };
    let forbidden = |code: &str| ("403 Forbidden".to_string(), code.to_string());
    for key in [wire::POLICY_ENABLED, wire::POLICY_DM_ENABLED] {
        let on = json!({ "changes": { key: true } });
        for who in [Who::Agent { project_id: "w".into(), session_id: "s".into() }, Who::Shell { project_id: "w".into(), session_id: "s".into() }] {
            assert_eq!(send(who.clone(), on.clone()), forbidden("person_only"), "{key} {who:?}");
        }
        assert_eq!(send(Who::Login { username: "mem".into(), role: Role::Member }, on.clone()), forbidden("owner_only"), "{key}: a Member");
        assert_eq!(send(Who::Login { username: "adm".into(), role: Role::Admin }, on.clone()), forbidden("owner_confirm_required"), "{key}: an Admin, no confirm");
        assert_eq!(send(Who::Owner, on.clone()), forbidden("owner_confirm_required"), "{key}: the owner token, no confirm");
        let other = e.vault.confirm.mint(crate::sew::recording_settings::CONFIRM_SPACE, Some(&install), "owner", "local-auth");
        let mut forged = on.clone();
        forged["confirmToken"] = json!(other);
        assert_eq!(send(Who::Owner, forged), forbidden("owner_confirm_required"), "{key}: another action's confirm");
        assert_eq!(want(key), None, "{key}: nothing stored");

        let tok = e.vault.confirm.mint(crate::sew::recording_settings::CONFIRM_POLICY, Some(&install), "owner", "local-auth");
        let mut confirmed = on.clone();
        confirmed["confirmToken"] = json!(tok);
        let (status, _) = send(Who::Owner, confirmed.clone());
        assert_eq!(status, "200 OK", "{key}: with the person's confirm");
        assert_eq!(want(key), Some(json!(true)), "{key}");
        let mut off = confirmed.clone();
        off["changes"][key] = json!(false);
        assert_eq!(send(Who::Owner, off), forbidden("owner_confirm_required"), "{key}: a confirm works once");
        assert_eq!(want(key), Some(json!(true)), "{key}: unchanged by the spent confirm");
    }
}
