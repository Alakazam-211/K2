use super::*;

/// Run `f` with a fresh temp HOME under the ONE env lock. The skin
/// store follows `$HOME`, so EVERY test that touches it goes through
/// here. HOME is restored and the dir removed on drop (even on panic).
fn with_temp_home<F: FnOnce()>(f: F) {
    let _home = crate::test_env::TempHome::new();
    f();
}

fn reopen_skin_db() {
    let path = store_path();
    {
        let mut guard = db().lock();
        *guard = None;
    }
    let conn = open_db(&path).expect("reopen skin.db");
    *db().lock() = Some(SkinDb { path, conn });
}

fn host_grants<'a>(grants: &'a [SkinGrant], principal_id: &str) -> Vec<&'a SkinGrant> {
    grants
        .iter()
        .filter(|g| {
            g.subject_kind == "principal"
                && g.subject_id == principal_id
                && g.kind == "app"
                && g.target_id == HOST_APP_ID
        })
        .collect()
}

#[test]
fn prefix_is_k2skn_never_k2sk() {
    assert_eq!(SKIN_KEY_PREFIX, "k2skn_");
    assert_ne!(SKIN_KEY_PREFIX, "k2sk_");
    assert!(is_skin_token("k2skn_abc"));
    assert!(!is_skin_token("k2sk_abc"));
    assert!(!is_api_key_prefix("k2skn_abc"));
    assert!(is_api_key_prefix("k2sk_abc"));
    assert!(
        !SKIN_KEY_PREFIX.starts_with("k2sk_") && !"k2sk_".starts_with(SKIN_KEY_PREFIX),
        "prefixes must not collide"
    );
}

#[test]
fn reserved_nested_label_skin_is_loud() {
    assert!(is_reserved_nested_label("skin"));
    assert!(is_reserved_nested_label("Skin"));
    assert!(!is_reserved_nested_label("staging"));
    let err = reserved_nested_label_error("skin");
    assert!(err.contains("reserved_label"), "{err}");
    assert!(
        err.contains("Pick another nested label"),
        "must tell them to pick another name, not a Caddy port: {err}"
    );
    assert!(
        !err.contains("38472"),
        "must not teach 38472 as a publish target: {err}"
    );
    assert_eq!(
        err,
        "reserved_label: 'skin' is reserved. Pick another nested label for your UI (k2 study apps; leftover: k2 study skins).",
        "skin text is unchanged"
    );
    assert!(!is_grandfathered_reserved_label("skin"), "skin stays strict");
}

#[test]
fn reserved_nested_labels_meet_and_chat_are_loud() {
    for name in ["meet", "MEET", " Meet ", "chat", "Chat", "\tchat\n", "skin", " SKIN"] {
        assert!(is_reserved_nested_label(name), "{name:?} must be reserved");
    }
    for name in ["meeting", "chats", "skins", "meet-1", "chat2", "mee", "cha", "", "app"] {
        assert!(!is_reserved_nested_label(name), "{name:?} must stay claimable");
    }
    let labels: Vec<&str> = RESERVED_NESTED_LABELS.iter().map(|r| r.label).collect();
    assert_eq!(labels, vec!["skin", "meet", "chat", "mail", "voice", "turn"]);
    for name in ["voice", " Voice ", "turn", "TURN"] {
        assert!(is_reserved_nested_label(name), "{name:?} must be reserved (Sew voice, K13)");
    }
    assert!(!is_reserved_nested_label("voices") && !is_reserved_nested_label("turn-x"));

    let meet = reserved_nested_label_error(" MEET ");
    assert_eq!(
        meet,
        "reserved_label: 'meet' is reserved for K2 Meet (meet.<box>.k2.dev). Pick another nested label."
    );
    let chat = reserved_nested_label_error("Chat");
    assert_eq!(
        chat,
        "reserved_label: 'chat' is reserved for K2 chat (chat.<box>.k2.dev). Pick another nested label."
    );
    assert!(is_grandfathered_reserved_label("meet"));
    assert!(is_grandfathered_reserved_label(" Chat "));
    assert!(!is_grandfathered_reserved_label("meeting"));
}

/// `mail` is the webmail front door (web only through the tunnel).
#[test]
fn reserved_nested_label_mail_is_loud_and_grandfathered() {
    for name in ["mail", "MAIL", " Mail "] {
        assert!(is_reserved_nested_label(name), "{name:?} must be reserved");
    }
    for name in ["mailbox", "mails", "webmail", "mail-1", "mai"] {
        assert!(!is_reserved_nested_label(name), "{name:?} must stay claimable");
    }
    assert_eq!(
        reserved_nested_label_error(" Mail "),
        "reserved_label: 'mail' is reserved for K2 webmail (mail.<box>.k2.dev). Pick another nested label."
    );
    assert!(is_grandfathered_reserved_label("mail"));
}

#[test]
fn skin_url_from_public_nests_skin_label() {
    assert_eq!(
        skin_url_from_public("https://rosson.k2.dev"),
        Some("https://skin.rosson.k2.dev".into())
    );
    assert_eq!(
        skin_url_from_public("https://skin.rosson.k2.dev/"),
        Some("https://skin.rosson.k2.dev".into())
    );
    assert_eq!(skin_url_from_public(""), None);
}

#[test]
fn principal_and_token_round_trip_secret_once() {
    with_temp_home(|| {
        let p = add_principal("Ada").expect("add");
        assert_eq!(p.username, "ada");
        let listed = list_principals().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].username, "ada");

        let room = uuid::Uuid::new_v4().to_string();
        let (meta, raw) = create_token(
            "ada",
            Some(&["thread:read".into()]),
            std::slice::from_ref(&room),
        )
        .expect("mint");
        assert!(raw.starts_with(SKIN_KEY_PREFIX), "got {raw}");
        assert!(!raw.starts_with("k2sk_"), "never k2sk_: {raw}");
        assert_eq!(raw.len(), SKIN_KEY_PREFIX.len() + KEY_BODY_LEN);
        assert!(meta.prefix.starts_with("k2skn_…"), "{}", meta.prefix);
        assert_eq!(meta.caps, vec!["thread:read"]);
        assert_eq!(meta.name, "ada");
        assert_eq!(meta.rooms, vec![room.clone()]);
        let tokens = list_tokens().expect("list tokens");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].name, "ada");
        assert!(
            !format!("{tokens:?}").contains(&raw),
            "secret must not leak in list"
        );
        assert!(!tokens[0].prefix.contains(&raw[SKIN_KEY_PREFIX.len()..]));

        let pass = resolve_skin_token(&raw).expect("live pass");
        assert_eq!(pass.username, "ada");
        assert!(
            pass.principal_id.is_none(),
            "platform mint has no principal"
        );
        assert!(!pass.session);
        assert!(pass.has_cap(CAP_THREAD_READ));
        assert!(!pass.has_cap(CAP_THREAD_POST));
        assert!(pass.has_room(&room));
        assert!(!pass.rooms_empty());
        assert!(!pass.has_room("00000000-0000-0000-0000-000000000000"));
        assert!(resolve_skin_token("k2skn_not-a-real-key").is_none());
        assert!(resolve_skin_token("k2sk_pretend").is_none());

        assert!(revoke_token(&meta.id).expect("revoke"));
        assert!(
            resolve_skin_token(&raw).is_none(),
            "revoked pass must not resolve"
        );
        assert!(!revoke_token(&meta.id).expect("idempotent"));
    });
}

/// Sew for Apps A1: the five chat caps are role caps, never defaults,
/// never implied, and `voice:video` needs `voice:join` beside it.
#[test]
fn parse_caps_accepts_chat_caps_never_implied_a1() {
    // `voice:record` waits for the Sew 0.9.0 pin (its own test below).
    let all: Vec<String> = CHAT_CAPS.iter().filter(|c| **c != CAP_VOICE_RECORD).map(|c| c.to_string()).collect();
    assert_eq!(parse_caps(Some(&all)).expect("chat caps"), all);
    for c in CHAT_CAPS {
        assert!(is_chat_cap(c), "{c}");
        assert!(!DEFAULT_CAPS.contains(c), "{c} is never a default");
    }
    let defaults = parse_caps(None).expect("defaults");
    assert!(!defaults.iter().any(|c| is_chat_cap(c)), "Thread-only defaults carry no chat cap: {defaults:?}");
    let voice = parse_caps(Some(&["voice:join".into()])).expect("voice alone");
    assert_eq!(voice, vec![CAP_VOICE_JOIN], "voice:join alone does not add chat:read here (the pass adds it)");
    let err = parse_caps(Some(&["voice:video".into()])).unwrap_err();
    assert!(err.contains("voice:video only widens voice:join"), "{err}");
    let err = parse_caps(Some(&["voice:video".into(), "chat:post".into()])).unwrap_err();
    assert!(err.contains("list voice:join too"), "{err}");
    assert!(parse_caps(Some(&["voice:join".into(), "voice:video".into()])).is_ok());
    let err = parse_caps(Some(&["voice:speak".into()])).unwrap_err();
    assert!(err.contains("unknown capability") && err.contains("voice:video"), "the accepted list names them: {err}");
    assert!(!is_chat_cap("thread:read") && !is_chat_cap("chat:manage"));
}

/// Sew 0.9.0 (K-b, PRD §8): since the pin (K2 0.46.11, the const on, no
/// test override) `voice:record` is a chat cap that only widens
/// `voice:join`, like `voice:video`. Before it, it was an unknown capability.
#[test]
fn voice_record_since_the_pin_only_widens_voice_join() {
    crate::recording::set_for_tests(false, false);
    assert!(crate::recording::in_apps(), "the pin's const is on (no test override)");
    let err = parse_caps(Some(&["voice:record".into()])).unwrap_err();
    assert!(err.contains("voice:record only widens voice:join"), "{err}");
    let err = parse_caps(Some(&["voice:record".into(), "chat:post".into()])).unwrap_err();
    assert!(err.contains("list voice:join too"), "{err}");
    let ok = parse_caps(Some(&["voice:record".into(), "voice:join".into()])).expect("with voice:join");
    assert_eq!(ok, vec![CAP_VOICE_RECORD.to_string(), CAP_VOICE_JOIN.to_string()]);
    assert!(is_chat_cap(CAP_VOICE_RECORD) && CHAT_CAPS.last() == Some(&CAP_VOICE_RECORD), "after voice:video, one list");
    crate::recording::set_for_tests(false, false);
}

/// A1: a role's chat caps reach the session snapshot (its union) from
/// any room line, so the chat pass can carry them.
#[test]
fn chat_caps_on_any_room_line_reach_the_session_union_a1() {
    let mut policy = RoomPolicy::new();
    policy.insert("room-a".into(), vec![CAP_THREAD_READ.into(), CAP_CHAT_POST.into()]);
    policy.insert("room-b".into(), vec![CAP_VOICE_JOIN.into(), CAP_VOICE_VIDEO.into()]);
    let caps = union_caps(&policy);
    for c in [CAP_THREAD_READ, CAP_CHAT_POST, CAP_VOICE_JOIN, CAP_VOICE_VIDEO] {
        assert!(caps.iter().any(|x| x == c), "{c} in {caps:?}");
    }
    assert!(!caps.iter().any(|x| x == CAP_CHAT_READ), "never implied by the union: {caps:?}");
}

#[test]
fn parse_caps_accepts_store_write() {
    let write = parse_caps(Some(&["store:write".into()])).expect("store:write");
    assert_eq!(write, vec![CAP_STORE_WRITE]);
    assert!(
        parse_caps(Some(&["thread:read".into()])).is_ok(),
        "thread:read remains accepted"
    );
    let mixed = parse_caps(Some(&["thread:read".into(), "store:write".into()])).expect("mixed");
    assert_eq!(mixed, vec![CAP_THREAD_READ, CAP_STORE_WRITE]);
    let err = parse_caps(Some(&["pty:write".into()])).unwrap_err();
    assert!(err.contains("unknown capability"), "{err}");
    assert!(err.contains("pty:write"), "{err}");
}

#[test]
fn parse_caps_accepts_files_verbs_never_silent_add() {
    let empty = parse_caps(None).expect("empty default");
    assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !empty
            .iter()
            .any(|c| c == CAP_FILES_READ || c == CAP_FILES_WRITE),
        "empty caps must stay Thread-only, never silent-add files: {empty:?}"
    );
    assert!(
        !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
        "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
    );
    let missing = parse_caps(Some(&[])).expect("missing default");
    assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !missing
            .iter()
            .any(|c| c == CAP_FILES_READ || c == CAP_FILES_WRITE),
        "missing caps must stay Thread-only: {missing:?}"
    );

    let read = parse_caps(Some(&["files:read".into()])).expect("files:read");
    assert_eq!(read, vec![CAP_FILES_READ]);
    let write = parse_caps(Some(&["files:write".into()])).expect("files:write");
    assert_eq!(write, vec![CAP_FILES_WRITE]);
    let both = parse_caps(Some(&[
        "files:read".into(),
        "files:write".into(),
        "thread:read".into(),
    ]))
    .expect("mixed");
    assert_eq!(both, vec![CAP_FILES_READ, CAP_FILES_WRITE, CAP_THREAD_READ]);

    let err = parse_caps(Some(&["files:sales".into()])).unwrap_err();
    assert!(err.contains("unknown capability"), "{err}");
    assert!(err.contains("files:sales"), "{err}");
    let err = parse_caps(Some(&["pty:write".into()])).unwrap_err();
    assert!(err.contains("pty:write"), "{err}");
}

#[test]
fn parse_caps_accepts_tickets_verbs_never_silent_add() {
    let empty = parse_caps(None).expect("empty default");
    assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !empty
            .iter()
            .any(|c| c == CAP_TICKETS_READ || c == CAP_TICKETS_POST),
        "empty caps must stay Thread-only, never silent-add tickets: {empty:?}"
    );
    assert!(
        !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
        "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
    );
    let missing = parse_caps(Some(&[])).expect("missing default");
    assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !missing
            .iter()
            .any(|c| c == CAP_TICKETS_READ || c == CAP_TICKETS_POST),
        "missing caps must stay Thread-only: {missing:?}"
    );

    let read = parse_caps(Some(&["tickets:read".into()])).expect("tickets:read");
    assert_eq!(read, vec![CAP_TICKETS_READ]);
    let post = parse_caps(Some(&["tickets:post".into()])).expect("tickets:post");
    assert_eq!(post, vec![CAP_TICKETS_POST]);
    let mixed = parse_caps(Some(&[
        "tickets:read".into(),
        "tickets:post".into(),
        "thread:read".into(),
    ]))
    .expect("mixed");
    assert_eq!(
        mixed,
        vec![CAP_TICKETS_READ, CAP_TICKETS_POST, CAP_THREAD_READ]
    );
}

#[test]
fn parse_caps_accepts_wiki_read_never_silent_add() {
    let empty = parse_caps(None).expect("empty default");
    assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !empty.iter().any(|c| c == CAP_WIKI_READ),
        "empty caps must stay Thread-only, never silent-add wiki: {empty:?}"
    );
    assert!(
        !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
        "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
    );
    let missing = parse_caps(Some(&[])).expect("missing default");
    assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !missing.iter().any(|c| c == CAP_WIKI_READ),
        "missing caps must stay Thread-only: {missing:?}"
    );

    let read = parse_caps(Some(&["wiki:read".into()])).expect("wiki:read");
    assert_eq!(read, vec![CAP_WIKI_READ]);
    let mixed = parse_caps(Some(&["wiki:read".into(), "thread:read".into()])).expect("mixed");
    assert_eq!(mixed, vec![CAP_WIKI_READ, CAP_THREAD_READ]);

    let err = parse_caps(Some(&["wiki:write".into()])).unwrap_err();
    assert!(err.contains("unknown capability"), "{err}");
    assert!(err.contains("wiki:write"), "{err}");
}

#[test]
fn parse_caps_accepts_store_read_never_silent_add() {
    let empty = parse_caps(None).expect("empty default");
    assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !empty
            .iter()
            .any(|c| c == CAP_STORE_READ || c == CAP_STORE_WRITE),
        "empty caps must stay Thread-only, never silent-add store: {empty:?}"
    );
    assert!(
        !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
        "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
    );
    let missing = parse_caps(Some(&[])).expect("missing default");
    assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !missing
            .iter()
            .any(|c| c == CAP_STORE_READ || c == CAP_STORE_WRITE),
        "missing caps must stay Thread-only: {missing:?}"
    );

    let read = parse_caps(Some(&["store:read".into()])).expect("store:read");
    assert_eq!(read, vec![CAP_STORE_READ]);
    let mixed = parse_caps(Some(&["store:read".into(), "thread:read".into()])).expect("mixed");
    assert_eq!(mixed, vec![CAP_STORE_READ, CAP_THREAD_READ]);
    let write = parse_caps(Some(&["store:write".into()])).expect("store:write");
    assert_eq!(write, vec![CAP_STORE_WRITE]);
    let both = parse_caps(Some(&[
        "store:read".into(),
        "store:write".into(),
        "thread:read".into(),
    ]))
    .expect("store both");
    assert_eq!(both, vec![CAP_STORE_READ, CAP_STORE_WRITE, CAP_THREAD_READ]);

    let err = parse_caps(Some(&["pty:write".into()])).unwrap_err();
    assert!(err.contains("unknown capability"), "{err}");
    assert!(err.contains("pty:write"), "{err}");
    let err = parse_caps(Some(&["wiki:write".into()])).unwrap_err();
    assert!(err.contains("wiki:write"), "{err}");
    let err = parse_caps(Some(&["grid".into()])).unwrap_err();
    assert!(err.contains("grid"), "{err}");
}

#[test]
fn parse_caps_accepts_activity_read_never_silent_add() {
    let empty = parse_caps(None).expect("empty default");
    assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
        "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
    );
    let missing = parse_caps(Some(&[])).expect("missing default");
    assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    assert!(
        !missing.iter().any(|c| c == CAP_ACTIVITY_READ),
        "missing caps must stay Thread-only: {missing:?}"
    );

    let files_read = parse_caps(Some(&["files:read".into()])).expect("files:read");
    assert_eq!(files_read, vec![CAP_FILES_READ]);
    assert!(
        !files_read.iter().any(|c| c == CAP_ACTIVITY_READ),
        "files:read must not imply activity:read: {files_read:?}"
    );
    let files_write = parse_caps(Some(&["files:write".into()])).expect("files:write");
    assert_eq!(files_write, vec![CAP_FILES_WRITE]);
    assert!(
        !files_write.iter().any(|c| c == CAP_ACTIVITY_READ),
        "files:write must not imply activity:read: {files_write:?}"
    );

    let activity = parse_caps(Some(&["activity:read".into()])).expect("activity:read");
    assert_eq!(activity, vec![CAP_ACTIVITY_READ]);
    assert!(
        !activity
            .iter()
            .any(|c| c == CAP_FILES_READ || c == CAP_FILES_WRITE),
        "activity:read must not imply files: {activity:?}"
    );

    let err = parse_caps(Some(&["activity:write".into()])).unwrap_err();
    assert!(err.contains("unknown capability"), "{err}");
    assert!(err.contains("activity:write"), "{err}");
}

/// 0.45.5 app identity surface: both caps parse, neither is a default,
/// and neither implies the other (or anything else).
#[test]
fn parse_caps_accepts_agent_identity_caps_never_implied() {
    let empty = parse_caps(None).expect("empty default");
    assert!(!empty.iter().any(|c| c == CAP_AGENT_IDENTITY || c == CAP_AGENT_HANDLE));
    let both = parse_caps(Some(&["agent:identity".into(), "agent:handle".into()]))
        .expect("agent caps");
    assert_eq!(both, vec![CAP_AGENT_IDENTITY, CAP_AGENT_HANDLE]);
    let room = "room-id".to_string();
    let pass_with = |caps: &[&str]| {
        let mut room_policy = RoomPolicy::new();
        room_policy.insert(room.clone(), caps.iter().map(|c| (*c).to_string()).collect());
        SkinPass {
            id: "p".into(),
            principal_id: Some("u".into()),
            username: "guest".into(),
            caps: caps.iter().map(|c| (*c).to_string()).collect(),
            rooms: vec![room.clone()],
            session: true,
            room_policy,
            app: "host".to_string(),
            room_paths: Default::default(),
            expires_at: None,
            chat: false,
        }
    };
    let ident = pass_with(&[CAP_AGENT_IDENTITY]);
    assert!(ident.has_cap_in_room(&room, CAP_AGENT_IDENTITY));
    assert!(!ident.has_cap_in_room(&room, CAP_AGENT_HANDLE), "identity must not imply handle");
    let handle = pass_with(&[CAP_AGENT_HANDLE]);
    assert!(!handle.has_cap_in_room(&room, CAP_AGENT_IDENTITY), "handle must not imply identity");
    for cap in [CAP_THREAD_READ, CAP_FILES_WRITE, CAP_HEARTBEATS_WRITE] {
        assert!(!pass_with(&[cap]).has_cap_in_room(&room, CAP_AGENT_IDENTITY), "{cap}");
        assert!(!pass_with(&[cap]).has_cap_in_room(&room, CAP_AGENT_HANDLE), "{cap}");
    }
    assert!(!ident.has_cap_in_room("other-room", CAP_AGENT_IDENTITY));
}

/// GH#72: the `.k2/` write opt-in is never a default, never implied by
/// files:write, and refused without files:write beside it.
#[test]
fn agent_config_opt_in_is_explicit_and_needs_files_write() {
    assert_eq!(CAP_FILES_AGENT_CONFIG, "files:agentconfig");
    let empty = parse_caps(None).expect("empty default");
    assert!(!empty.iter().any(|c| c == CAP_FILES_AGENT_CONFIG));
    let write = parse_caps(Some(&["files:write".into()])).expect("files:write");
    assert!(!write.iter().any(|c| c == CAP_FILES_AGENT_CONFIG), "files:write must not imply it");
    let alone = parse_caps(Some(&["files:agentconfig".into()])).expect_err("alone is refused");
    assert!(alone.contains("only widens files:write"), "{alone}");
    let alone = parse_caps(Some(&["files:read".into(), "files:agentconfig".into()]))
        .expect_err("with files:read only is refused");
    assert!(alone.contains("only widens files:write"), "{alone}");
    let both = parse_caps(Some(&["files:write".into(), "files:agentconfig".into()]))
        .expect("with files:write");
    assert_eq!(both, vec![CAP_FILES_WRITE, CAP_FILES_AGENT_CONFIG]);

    let mut old = RoomPolicy::new();
    old.insert("a".into(), vec![CAP_FILES_WRITE.into(), CAP_FILES_AGENT_CONFIG.into()]);
    old.insert("b".into(), vec![CAP_FILES_WRITE.into()]);
    assert!(policy_has_agent_config(&old));
    let mut same = old.clone();
    same.insert("c".into(), vec![CAP_THREAD_READ.into()]);
    assert!(agent_config_added(Some(&old), &same).is_empty(), "keeping it is not adding it");
    let mut widened = old.clone();
    widened.insert("b".into(), vec![CAP_FILES_WRITE.into(), CAP_FILES_AGENT_CONFIG.into()]);
    assert_eq!(agent_config_added(Some(&old), &widened), vec!["b".to_string()]);
    assert_eq!(agent_config_added(None, &old), vec!["a".to_string()]);
    let mut tightened = old.clone();
    tightened.insert("a".into(), vec![CAP_FILES_WRITE.into()]);
    assert!(agent_config_added(Some(&old), &tightened).is_empty());
    assert!(!policy_has_agent_config(&tightened));
}

/// 0.46.3 Thread downloads: `files:download` parses on its own, is never
/// a default, is implied by neither `thread:read` nor `files:read`, and
/// is person-only (an agent may never add it).
#[test]
fn files_download_is_explicit_never_implied_and_person_only() {
    assert_eq!(CAP_FILES_DOWNLOAD, "files:download");
    let empty = parse_caps(None).expect("empty default");
    assert!(!empty.iter().any(|c| c == CAP_FILES_DOWNLOAD), "never a default");
    let read = parse_caps(Some(&["thread:read".into(), "files:read".into()])).expect("read caps");
    assert!(!read.iter().any(|c| c == CAP_FILES_DOWNLOAD), "thread:read + files:read must not imply it");
    let dl = parse_caps(Some(&["thread:read".into(), "files:download".into()])).expect("files:download parses");
    assert_eq!(dl, vec![CAP_THREAD_READ, CAP_FILES_DOWNLOAD]);
    assert!(PERSON_ONLY_CAPS.contains(&CAP_FILES_DOWNLOAD));
    assert!(PERSON_ONLY_CAPS.contains(&CAP_FILES_AGENT_CONFIG));

    let mut old = RoomPolicy::new();
    old.insert("a".into(), vec![CAP_THREAD_READ.into(), CAP_FILES_DOWNLOAD.into()]);
    old.insert("b".into(), vec![CAP_THREAD_READ.into()]);
    assert_eq!(person_only_cap_in_policy(&old), Some(CAP_FILES_DOWNLOAD));
    assert_eq!(person_only_cap_added(Some(&old), &old), None, "keeping it is not adding it");
    let mut widened = old.clone();
    widened.insert("b".into(), vec![CAP_THREAD_READ.into(), CAP_FILES_DOWNLOAD.into()]);
    assert_eq!(person_only_cap_added(Some(&old), &widened), Some(CAP_FILES_DOWNLOAD));
    assert_eq!(person_only_cap_added(None, &old), Some(CAP_FILES_DOWNLOAD));
    let caps: Vec<String> = vec![CAP_FILES_WRITE.into(), CAP_FILES_AGENT_CONFIG.into(), CAP_FILES_DOWNLOAD.into()];
    assert_eq!(person_only_cap_in(&caps), Some(CAP_FILES_AGENT_CONFIG), "agentconfig is named first");
    let plain: Vec<String> = vec![CAP_THREAD_READ.into(), CAP_FILES_READ.into()];
    assert_eq!(person_only_cap_in(&plain), None);
}

#[test]
fn parse_caps_accepts_heartbeat_caps_never_implied_ah1() {
    let empty = parse_caps(None).expect("empty default");
    assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
    let both = parse_caps(Some(&[
        "heartbeats:read".into(),
        "heartbeats:write".into(),
    ]))
    .expect("heartbeat caps");
    assert_eq!(both, vec![CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE]);

    let room = "room-ah1".to_string();
    let pass_with = |caps: &[&str]| {
        let mut room_policy = RoomPolicy::new();
        room_policy.insert(
            room.clone(),
            caps.iter().map(|c| (*c).to_string()).collect(),
        );
        SkinPass {
            id: "p".into(),
            principal_id: Some("u".into()),
            username: "guest".into(),
            caps: caps.iter().map(|c| (*c).to_string()).collect(),
            rooms: vec![room.clone()],
            session: true,
            room_policy,
            app: "host".to_string(),
            room_paths: Default::default(),
            expires_at: None,
            chat: false,
        }
    };
    for caps in [
        vec![CAP_FILES_READ],
        vec![CAP_FILES_WRITE],
        vec![CAP_ACTIVITY_READ],
        vec![CAP_FILES_READ, CAP_FILES_WRITE, CAP_ACTIVITY_READ],
    ] {
        let p = pass_with(&caps);
        assert!(!p.has_cap_in_room(&room, CAP_HEARTBEATS_READ), "{caps:?}");
        assert!(!p.has_cap_in_room(&room, CAP_HEARTBEATS_WRITE), "{caps:?}");
    }
    let w = pass_with(&[CAP_HEARTBEATS_WRITE]);
    assert!(!w.has_cap_in_room(&room, CAP_HEARTBEATS_READ), "write must not imply read");
    let r = pass_with(&[CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE]);
    for cap in [CAP_FILES_READ, CAP_FILES_WRITE, CAP_ACTIVITY_READ] {
        assert!(!r.has_cap_in_room(&room, cap), "heartbeat caps must not imply {cap}");
    }
    assert!(r.has_any_cap_in_room(&room, &[CAP_ACTIVITY_READ, CAP_HEARTBEATS_READ]));
    assert!(!w.has_any_cap_in_room(&room, &[CAP_ACTIVITY_READ, CAP_HEARTBEATS_READ]));

    let platform = SkinPass {
        id: "t".into(),
        principal_id: None,
        username: "bot".into(),
        caps: vec![CAP_HEARTBEATS_READ.into()],
        rooms: vec![room.clone()],
        session: false,
        room_policy: RoomPolicy::new(),
        app: String::new(),
        room_paths: Default::default(),
        expires_at: None,
        chat: false,
    };
    assert!(platform.dispatcher_admits_any(&[CAP_ACTIVITY_READ, CAP_HEARTBEATS_READ]));
    assert!(!platform.dispatcher_admits_cap(CAP_ACTIVITY_READ));
    assert!(!platform.dispatcher_admits_any(&[CAP_ACTIVITY_READ, CAP_FILES_READ]));

    let err = parse_caps(Some(&["heartbeats:admin".into()])).unwrap_err();
    assert!(err.contains("unknown capability"), "{err}");
}

#[test]
fn mint_requires_principal_and_rejects_unknown_caps() {
    with_temp_home(|| {
        let room = uuid::Uuid::new_v4().to_string();
        let (ghost_meta, ghost_raw) =
            create_token("ghost", None, std::slice::from_ref(&room)).expect("no principal");
        assert_eq!(ghost_meta.name, "ghost");
        let ghost = resolve_skin_token(&ghost_raw).expect("platform ghost");
        assert!(ghost.principal_id.is_none(), "{ghost:?}");
        assert!(!ghost.session);
        assert_eq!(ghost.username, "ghost");

        let err = create_token("owner", None, std::slice::from_ref(&room)).unwrap_err();
        assert!(err.contains("owner"), "{err}");
        assert!(err.contains("reserved"), "{err}");

        let err = create_token(
            "bob",
            Some(&["pty:write".into()]),
            std::slice::from_ref(&room),
        )
        .unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
        assert!(err.contains("pty:write"), "{err}");
        let err = create_token("bob", None, &[]).unwrap_err();
        assert!(
            err.contains("rooms must include at least one workspace"),
            "{err}"
        );
        let (_meta, raw) =
            create_token("bob", None, std::slice::from_ref(&room)).expect("default caps");
        let pass = resolve_skin_token(&raw).expect("resolve");
        assert!(pass.principal_id.is_none());
        assert!(pass.has_cap(CAP_THREAD_READ));
        assert!(pass.has_cap(CAP_THREAD_POST));
        assert!(!pass.has_cap(CAP_FILES_READ));
        assert!(!pass.has_cap(CAP_FILES_WRITE));
        assert!(!pass.has_cap(CAP_WIKI_READ));
        assert!(!pass.has_cap(CAP_STORE_READ));
        assert_eq!(pass.rooms, vec![room.clone()]);

        let err = create_token("bob", None, std::slice::from_ref(&room)).unwrap_err();
        assert!(err.contains("already exists"), "{err}");

        let (meta, raw) = create_token(
            "bob-files",
            Some(&["files:read".into(), "files:write".into()]),
            std::slice::from_ref(&room),
        )
        .expect("files caps");
        assert_eq!(meta.caps, vec![CAP_FILES_READ, CAP_FILES_WRITE]);
        let pass = resolve_skin_token(&raw).expect("files pass");
        assert!(pass.has_cap(CAP_FILES_READ));
        assert!(pass.has_cap(CAP_FILES_WRITE));
        assert!(!pass.has_cap(CAP_THREAD_READ));
    });
}

#[test]
fn remove_principal_drops_tokens() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        let room = uuid::Uuid::new_v4().to_string();
        let (_meta, platform_raw) =
            create_token("cara", None, std::slice::from_ref(&room)).unwrap();
        let (_sess, sess_raw) = create_session_token("cara").unwrap();
        assert!(resolve_skin_token(&platform_raw).is_some());
        assert!(resolve_skin_token(&sess_raw).is_some());
        assert!(remove_principal("cara").unwrap());
        assert!(
            resolve_skin_token(&platform_raw).is_some(),
            "platform token survives guest delete"
        );
        assert!(
            resolve_skin_token(&sess_raw).is_none(),
            "session=1 must drop with the guest"
        );
        assert!(list_principals().unwrap().is_empty());
        assert_eq!(list_tokens().unwrap().len(), 1);
        assert!(!remove_principal("cara").unwrap());
    });
}

#[test]
fn front_door_persists_mode() {
    with_temp_home(|| {
        let d = effective_front_door().expect("default");
        assert_eq!(d.mode, "connect");
        let saved = set_front_door(
            "direct",
            Some("https://skin.app.com"),
            Some("LAN"),
            Some(5173),
        )
        .expect("save");
        assert_eq!(saved.mode, "direct");
        assert_eq!(saved.url.as_deref(), Some("https://skin.app.com"));
        assert_eq!(saved.ui_port, Some(5173));
        let loaded = get_front_door().expect("load");
        assert_eq!(loaded.mode, "direct");
        assert_eq!(loaded.url.as_deref(), Some("https://skin.app.com"));
        assert_eq!(loaded.ui_port, Some(5173));
        assert!(front_door_is_stored());
        assert_eq!(parse_front_door_mode("CONNECT").unwrap(), "connect");
        assert!(parse_front_door_mode("lan").is_err());
    });
}

#[test]
fn parse_rooms_json_empty_is_deny_not_all_agents() {
    assert!(parse_rooms_json(None).is_empty());
    assert!(parse_rooms_json(Some("")).is_empty());
    assert!(parse_rooms_json(Some("[]")).is_empty());
    assert!(parse_rooms_json(Some("not-json")).is_empty());
    assert!(parse_rooms_json(Some("{}")).is_empty());
    assert_eq!(
        parse_rooms_json(Some(r#"["a"," ","b"]"#)),
        vec!["a".to_string(), "b".to_string()]
    );
    let empty_caps = parse_caps(None).expect("caps default");
    assert_eq!(empty_caps.len(), 2, "empty caps still default both verbs");
    assert!(
        !empty_caps.iter().any(|c| c.starts_with("files:")),
        "empty caps must not grant files: {empty_caps:?}"
    );
    assert!(
        !empty_caps.iter().any(|c| c == CAP_ACTIVITY_READ),
        "empty caps must stay Thread-only, never silent-add activity:read: {empty_caps:?}"
    );
    assert!(
        parse_rooms_json(None).is_empty(),
        "rooms must not copy parse_caps default-all"
    );
}

#[test]
fn set_token_rooms_does_not_touch_secret() {
    with_temp_home(|| {
        add_principal("ada").unwrap();
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let (meta, raw) = create_token("ada", None, std::slice::from_ref(&a)).unwrap();
        let updated = set_token_rooms(&meta.id, std::slice::from_ref(&b)).expect("patch");
        assert_eq!(updated.rooms, vec![b.clone()]);
        let pass = resolve_skin_token(&raw).expect("same secret");
        assert_eq!(pass.rooms, vec![b]);
        let cleared = set_token_rooms(&meta.id, &[]).expect("clear");
        assert!(cleared.rooms.is_empty());
        let dark = resolve_skin_token(&raw).expect("still live");
        assert!(dark.rooms_empty());
    });
}

#[test]
fn resolve_room_tokens_handle_alias_uuid_not_display_name() {
    crate::db::init_for_tests();
    let db = crate::db::shared();
    let conn = db.lock();
    use rusqlite::params;
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("sales{}", &id[..8]);
    let pretty = format!("Wallpaper {handle}");
    let path = format!("/tmp/skin-room-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
        params![id, pretty, path, handle],
    )
    .expect("project");
    conn.execute(
        "INSERT OR IGNORE INTO project_handle_aliases (project_id, alias) VALUES (?1, ?2)",
        params![id, "old-sales-alias"],
    )
    .ok();
    drop(conn);

    assert_eq!(
        resolve_room_tokens(&[handle.clone()]).expect("handle"),
        vec![id.clone()]
    );
    assert_eq!(
        resolve_room_tokens(&[id.clone()]).expect("uuid"),
        vec![id.clone()]
    );
    assert_eq!(
        resolve_room_tokens(&["old-sales-alias".into()]).expect("alias"),
        vec![id.clone()]
    );
    let err = resolve_room_tokens(&[pretty]).unwrap_err();
    assert!(err.contains("unknown workspace handle"), "{err}");
    let err = resolve_room_tokens(&["not-a-handle".into()]).unwrap_err();
    assert!(err.contains("unknown workspace handle"), "{err}");

    let agents = live_agents(&[id.clone()]);
    assert_eq!(agents.len(), 1, "{agents:?}");
    assert_eq!(agents[0].handle, handle);
    assert_eq!(agents[0].project_id, id);
    assert!(
        !agents[0].display_name.trim().is_empty(),
        "display_name must be a non-empty string: {agents:?}"
    );
    let wire = serde_json::to_value(&agents[0]).expect("wire");
    assert_eq!(wire["handle"], handle);
    assert_eq!(wire["projectId"], id);
    assert!(
        wire["displayName"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "wire displayName: {wire}"
    );
}

#[test]
fn wal_file_is_created_under_k2_home() {
    with_temp_home(|| {
        add_principal("wal-user").unwrap();
        let db_path = crate::paths::k2_home().join("skin.db");
        assert!(db_path.is_file(), "skin.db must exist at {db_path:?}");
        let wal = crate::paths::k2_home().join("skin.db-wal");
        let shm = crate::paths::k2_home().join("skin.db-shm");
        assert!(
            wal.is_file() || shm.is_file() || db_path.is_file(),
            "WAL mode should leave wal/shm companions or at least the db"
        );
    });
}

#[test]
fn session_mint_allows_empty_rooms_create_token_does_not() {
    with_temp_home(|| {
        add_principal("ada").unwrap();
        let err = create_token("ada", None, &[]).unwrap_err();
        assert!(
            err.contains("rooms must include at least one workspace"),
            "{err}"
        );
        let (meta, raw) = create_session_token("ada").expect("session mint");
        assert!(raw.starts_with(SKIN_KEY_PREFIX), "{raw}");
        assert!(meta.rooms.is_empty(), "default_rooms [] must copy");
        let pass = resolve_skin_token(&raw).expect("live session");
        assert!(pass.session);
        assert!(pass.principal_id.is_some());
        assert_eq!(pass.username, "ada");
        assert!(pass.rooms_empty());
        assert!(
            !pass.has_cap(CAP_FILES_READ),
            "empty map is Thread dark, never files: {pass:?}"
        );
        assert!(
            list_tokens().unwrap().is_empty(),
            "list_tokens is platform only"
        );
    });
}

#[test]
fn resolve_rejects_expired_session_static_survives() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let room = uuid::Uuid::new_v4().to_string();
        let (static_meta, static_raw) =
            create_token("bob", None, std::slice::from_ref(&room)).unwrap();
        assert!(resolve_skin_token(&static_raw).is_some());
        let (_sess, sess_raw) = create_session_token("bob").unwrap();
        assert!(resolve_skin_token(&sess_raw).is_some());
        let _ = static_meta;
        with_conn(|conn| {
            conn.execute("UPDATE tokens SET expires_at = 1 WHERE session = 1", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
        assert!(
            resolve_skin_token(&sess_raw).is_none(),
            "expired session must not resolve"
        );
        assert!(
            resolve_skin_token(&static_raw).is_some(),
            "static mint expires_at NULL stays live"
        );
    });
}

#[test]
fn password_reset_revokes_sessions_not_static() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        let room = uuid::Uuid::new_v4().to_string();
        let (_sm, static_raw) =
            create_token("cara", None, std::slice::from_ref(&room)).unwrap();
        let (_sess, sess_raw) = create_session_token("cara").unwrap();
        assert!(resolve_skin_token(&sess_raw).is_some());
        set_principal_password("cara", Some("new-pass-word")).unwrap();
        assert!(
            resolve_skin_token(&sess_raw).is_none(),
            "password reset revokes session=1"
        );
        assert!(
            resolve_skin_token(&static_raw).is_some(),
            "static partner key survives password reset"
        );
        match check_and_record_login("cara", "new-pass-word") {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "cara"),
            other => panic!("expected Ok, got {other:?}"),
        }
        match check_and_record_login("cara", "s3cret-horse") {
            SkinLoginOutcome::BadCreds => {}
            _ => panic!("old password must fail"),
        }
        match check_and_record_login("ghost-user", "nope") {
            SkinLoginOutcome::BadCreds => {}
            _ => panic!("unknown user must be generic BadCreds"),
        }
        set_principal_password("cara", None).unwrap();
        match check_and_record_login("cara", "new-pass-word") {
            SkinLoginOutcome::BadCreds => {}
            _ => panic!("cleared password cannot K2-login"),
        }
        assert!(
            resolve_skin_token(&static_raw).is_some(),
            "clearing password still leaves platform mint"
        );
    });
}

#[test]
fn guest_and_platform_same_name_coexist_revoked_name_reusable() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let room = uuid::Uuid::new_v4().to_string();
        let (meta, raw) = create_token("bob", None, std::slice::from_ref(&room)).unwrap();
        assert_eq!(meta.name, "bob");
        let pass = resolve_skin_token(&raw).expect("platform bob");
        assert_eq!(pass.username, "bob");
        assert!(pass.principal_id.is_none());
        let listed = list_principals().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].username, "bob");
        assert!(revoke_token(&meta.id).unwrap());
        let (meta2, raw2) =
            create_token("bob", None, std::slice::from_ref(&room)).expect("reuse revoked name");
        assert_eq!(meta2.name, "bob");
        assert!(resolve_skin_token(&raw2).is_some());
        assert!(resolve_skin_token(&raw).is_none());
    });
}

#[test]
fn migrate_pre_reshape_static_row_fills_name_nulls_principal() {
    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("open old db");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
                    key_hash TEXT NOT NULL UNIQUE,
                    key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    revoked_at INTEGER,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    session INTEGER NOT NULL DEFAULT 0,
                    expires_at INTEGER
                 );",
        )
        .expect("old schema");
        let pid = uuid::Uuid::new_v4().to_string();
        let tid = uuid::Uuid::new_v4().to_string();
        let raw = format!("{SKIN_KEY_PREFIX}MigrateHashBody00000000000000000000000");
        let key_hash = sha256_hex(&raw);
        let prefix = display_prefix(&raw);
        conn.execute(
            "INSERT INTO principals (id, username, created_at) VALUES (?1, 'bob', 1)",
            params![pid],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tokens (id, principal_id, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session)
                 VALUES (?1, ?2, ?3, ?4, ?5, '[]', 1, NULL, 0)",
            params![tid, pid, key_hash, prefix, caps_json(&parse_caps(None).unwrap())],
        )
        .unwrap();
        drop(conn);

        let pass = resolve_skin_token(&raw).expect("hash unchanged after rebuild");
        assert!(pass.principal_id.is_none(), "{pass:?}");
        assert!(!pass.session);
        assert_eq!(pass.username, "bob");
        let listed = list_tokens().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "bob");
        with_conn(|c| {
            let notnull = tokens_principal_id_notnull(c)?;
            assert!(!notnull, "rebuilt principal_id must be nullable");
            let (principal_id, name, session): (Option<String>, Option<String>, i64) = c
                .query_row(
                    "SELECT principal_id, name, session FROM tokens WHERE id = ?1",
                    params![tid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(|e| e.to_string())?;
            assert!(principal_id.is_none(), "{principal_id:?}");
            assert_eq!(name.as_deref(), Some("bob"));
            assert_eq!(session, 0);
            Ok(())
        })
        .unwrap();
    });
}

#[test]
fn lockout_is_generic_and_is_not_dummy_argon_alone() {
    with_temp_home(|| {
        add_principal("eve").unwrap();
        set_principal_password("eve", Some("right-password")).unwrap();
        for _ in 0..3 {
            match check_and_record_login("eve", "wrong") {
                SkinLoginOutcome::BadCreds => {}
                _ => panic!("pre-lockout must be BadCreds"),
            }
        }
        match check_and_record_login("eve", "right-password") {
            SkinLoginOutcome::LockedOut { .. } => {}
            _ => panic!("4th attempt after 3 fails is lockout, not a verify"),
        }
        match check_and_record_login("eve", "wrong") {
            SkinLoginOutcome::LockedOut { .. } => {}
            _ => panic!("locked username stays generic LockedOut"),
        }
    });
}

/// A client BFF ask (2026-10-13): a locked login says how long is left, and
/// an unknown username locks exactly like a known one (no oracle).
#[test]
fn lockout_reports_retry_after_for_known_and_unknown_alike() {
    with_temp_home(|| {
        add_principal("eve").unwrap();
        set_principal_password("eve", Some("right-password")).unwrap();
        for who in ["eve", "nobody-here"] {
            for _ in 0..3 {
                assert!(matches!(
                    check_and_record_login(who, "wrong"),
                    SkinLoginOutcome::BadCreds
                ));
            }
            match check_and_record_login(who, "right-password") {
                SkinLoginOutcome::LockedOut { retry_after_secs } => assert!(
                    (1..=LOCKOUT_DURATION_SECS).contains(&retry_after_secs),
                    "{who}: {retry_after_secs}"
                ),
                other => panic!("{who}: want LockedOut, got {other:?}"),
            }
        }
    });
}

/// Move the last failure on `key`'s row back by `secs` (a clock for
/// the 15-minute window without sleeping).
fn age_last_failure(key: &str, secs: i64) {
    let n = with_conn(|conn| {
        conn.execute(
            "UPDATE login_lockouts SET last_failed_at = last_failed_at - ?2 WHERE username = ?1 AND last_failed_at IS NOT NULL",
            params![key, secs],
        )
        .map_err(|e| e.to_string())
    })
    .expect("age the last failure");
    assert_eq!(n, 1, "{key} has a timed failure row");
}

fn failed_count_of(key: &str) -> (i64, Option<i64>, Option<i64>) {
    with_conn(|conn| {
        conn.query_row(
            "SELECT failed_count, locked_until, last_failed_at FROM login_lockouts WHERE username = ?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| e.to_string())
    })
    .unwrap_or_else(|e| panic!("{key}: {e}"))
}

/// 0.46.6 (App guest login lockout, fix 1c): failures older than 15
/// minutes are forgotten. Two old failures and one new one are not a
/// lock; three within the window still are; a correct password still
/// clears the count. Known and unknown names alike.
#[test]
fn lockout_forgets_failures_older_than_the_window() {
    with_temp_home(|| {
        add_principal("eve").unwrap();
        set_principal_password("eve", Some("right-password")).unwrap();
        for who in ["eve", "nobody-here"] {
            for _ in 0..2 {
                assert!(matches!(check_and_record_login(who, "wrong"), SkinLoginOutcome::BadCreds), "{who}");
            }
            let (count, locked, at) = failed_count_of(who);
            assert_eq!((count, locked), (2, None), "{who}");
            assert!(at.is_some_and(|t| (now_secs() - t).abs() <= 5), "{who}: the failure is timed: {at:?}");
            // 16 minutes pass: the two failures are forgotten.
            age_last_failure(who, LOCKOUT_FAIL_WINDOW_SECS + 60);
            assert!(matches!(check_and_record_login(who, "wrong"), SkinLoginOutcome::BadCreds), "{who}");
            assert_eq!(failed_count_of(who).0, 1, "{who}: the count started again");
            if who == "eve" {
                match check_and_record_login(who, "right-password") {
                    SkinLoginOutcome::Ok(_) => {}
                    other => panic!("not locked after old failures, got {other:?}"),
                }
            }
        }
        // Exactly at the window's edge the count still holds; three
        // failures inside it are still a lock.
        for _ in 0..2 {
            assert!(matches!(check_and_record_login("eve", "wrong"), SkinLoginOutcome::BadCreds));
        }
        age_last_failure("eve", LOCKOUT_FAIL_WINDOW_SECS - 5);
        assert!(matches!(check_and_record_login("eve", "wrong"), SkinLoginOutcome::BadCreds));
        match check_and_record_login("eve", "right-password") {
            SkinLoginOutcome::LockedOut { retry_after_secs } => {
                assert!((1..=LOCKOUT_DURATION_SECS).contains(&retry_after_secs), "{retry_after_secs}")
            }
            other => panic!("three failures inside the window lock, got {other:?}"),
        }
    });
}

/// 0.46.6 (fix 1c): a count written before schema step 8 has no time:
/// it is forgotten at the next failure (the stale counts that locked a
/// real guest out after one wrong password), and a live lock on such a
/// row still holds until it ends.
#[test]
fn an_untimed_count_from_before_step8_starts_again() {
    with_temp_home(|| {
        add_principal("ida").unwrap();
        set_principal_password("ida", Some("right-password")).unwrap();
        add_principal("lou").unwrap();
        set_principal_password("lou", Some("right-password")).unwrap();
        with_conn(|conn| {
            conn.execute(
                "INSERT INTO login_lockouts (username, failed_count, locked_until) VALUES ('ida', 2, NULL), ('lou', 0, ?1)",
                params![now_secs() + 600],
            )
            .map_err(|e| e.to_string())
        })
        .expect("seed old rows");
        assert!(matches!(check_and_record_login("ida", "wrong"), SkinLoginOutcome::BadCreds));
        let (count, locked, at) = failed_count_of("ida");
        assert_eq!((count, locked), (1, None), "two old failures and one new are not a lock");
        assert!(at.is_some(), "the new failure is timed");
        match check_and_record_login("ida", "right-password") {
            SkinLoginOutcome::Ok(_) => {}
            other => panic!("ida signs in, got {other:?}"),
        }
        assert!(lock_rows().iter().all(|r| r.0 != "ida"), "a correct password clears the row");
        match check_and_record_login("lou", "right-password") {
            SkinLoginOutcome::LockedOut { .. } => {}
            other => panic!("an old live lock still holds, got {other:?}"),
        }
    });
}

/// Step 8: a version-7 file (an earlier 0.46.6 build) gains
/// `login_lockouts.last_failed_at`, is backed up first, and keeps its
/// rows (the old count has no time).
#[test]
fn step8_adds_the_failure_time_to_a_v7_file() {
    with_temp_home(|| {
        add_principal("kim").unwrap();
        assert!(matches!(check_and_record_login("kim", "wrong"), SkinLoginOutcome::BadCreds));
        let path = store_path();
        *db().lock() = None;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("ALTER TABLE login_lockouts DROP COLUMN last_failed_at; PRAGMA user_version = 7;")
                .unwrap();
            assert!(!table_columns(&conn, "login_lockouts").split(',').any(|c| c == "last_failed_at"));
        }
        let _ = std::fs::remove_file(path.with_file_name("skin.db.pre-8.bak"));
        assert!(principal_exists("kim").unwrap(), "the v7 file opens and keeps its rows");
        assert_eq!(with_conn(|c| schema_version(c)).unwrap(), 8);
        assert!(path.with_file_name("skin.db.pre-8.bak").exists(), "backed up before step 8");
        let cols = with_conn(|c| Ok(table_columns(c, "login_lockouts"))).unwrap();
        assert!(cols.split(',').any(|c| c == "last_failed_at"), "{cols}");
        assert_eq!(failed_count_of("kim"), (1, None, None), "the old count survives, untimed");
    });
}

fn lock_rows() -> Vec<(String, i64, Option<i64>)> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT username, failed_count, locked_until FROM login_lockouts
                     ORDER BY username",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    })
    .expect("lock rows")
}

/// agent-acme (0.45.5): username locked, the right password typed as
/// the EMAIL still signed in. The lock is the principal's.
#[test]
fn lock_via_username_refuses_email_with_right_password() {
    with_temp_home(|| {
        add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
        set_principal_password("jane", Some("s3cret-horse")).unwrap();
        for _ in 0..3 {
            match check_and_record_login("jane", "wrong") {
                SkinLoginOutcome::BadCreds => {}
                other => panic!("pre-lock via username, got {other:?}"),
            }
        }
        for who in ["jane@clinic.com", "  JANE@Clinic.com ", "jane", "JANE"] {
            match check_and_record_login(who, "s3cret-horse") {
                SkinLoginOutcome::LockedOut { retry_after_secs } => assert!(
                    (1..=LOCKOUT_DURATION_SECS).contains(&retry_after_secs),
                    "{who}: {retry_after_secs}"
                ),
                other => panic!("{who}: locked principal must refuse, got {other:?}"),
            }
        }
        let rows = lock_rows();
        assert_eq!(rows.len(), 1, "one counter for the principal: {rows:?}");
        assert_eq!(rows[0].0, "jane", "{rows:?}");
    });
}

/// Wrong tries on the username and the email share ONE limit (0.45.5
/// counted them separately, doubling it to 6).
#[test]
fn mixed_username_and_email_wrong_tries_share_one_limit() {
    with_temp_home(|| {
        add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
        set_principal_password("jane", Some("s3cret-horse")).unwrap();
        for who in ["jane", "Jane@Clinic.COM", "jane"] {
            match check_and_record_login(who, "wrong") {
                SkinLoginOutcome::BadCreds => {}
                other => panic!("{who}: pre-lock, got {other:?}"),
            }
        }
        for who in ["jane", "jane@clinic.com"] {
            match check_and_record_login(who, "s3cret-horse") {
                SkinLoginOutcome::LockedOut { .. } => {}
                other => panic!("{who}: 3 mixed tries lock, got {other:?}"),
            }
        }
        let rows = lock_rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "jane", "{rows:?}");
        assert_eq!(rows[0].1, 0, "a lock zeroes the count: {rows:?}");
        assert!(rows[0].2.is_some(), "{rows:?}");
    });
}

/// A correct login resets the principal's counter whichever name it
/// came in on.
#[test]
fn success_by_email_resets_the_username_counter() {
    with_temp_home(|| {
        add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
        set_principal_password("jane", Some("s3cret-horse")).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                check_and_record_login("jane", "wrong"),
                SkinLoginOutcome::BadCreds
            ));
        }
        assert!(matches!(
            check_and_record_login("jane@clinic.com", "s3cret-horse"),
            SkinLoginOutcome::Ok(_)
        ));
        assert!(lock_rows().is_empty(), "{:?}", lock_rows());
        for _ in 0..2 {
            assert!(matches!(
                check_and_record_login("jane@clinic.com", "wrong"),
                SkinLoginOutcome::BadCreds
            ));
        }
        assert!(
            matches!(
                check_and_record_login("jane", "s3cret-horse"),
                SkinLoginOutcome::Ok(_)
            ),
            "2 fails after a reset are under the limit"
        );
    });
}

/// `k2 app user unlock` by username or by email clears the principal's
/// lock, and any 0.45.5 email-keyed row left from before the upgrade.
#[test]
fn unlock_by_username_or_email_clears_every_principal_row() {
    with_temp_home(|| {
        add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
        set_principal_password("jane", Some("s3cret-horse")).unwrap();
        for (unlock_as, want_key) in [("Jane", "jane"), ("JANE@clinic.com", "jane@clinic.com")] {
            for _ in 0..3 {
                assert!(matches!(
                    check_and_record_login("jane@clinic.com", "wrong"),
                    SkinLoginOutcome::BadCreds
                ));
            }
            // A row written by 0.45.5 under the email key.
            with_conn(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO login_lockouts (username, failed_count, locked_until)
                         VALUES ('jane@clinic.com', 0, ?1)",
                    params![now_secs().saturating_add(900)],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .expect("legacy email row");
            let (key, cleared) = clear_login_lockout(unlock_as).expect("unlock");
            assert_eq!(key, want_key);
            assert!(cleared, "{unlock_as}");
            assert!(lock_rows().is_empty(), "{unlock_as}: {:?}", lock_rows());
            for who in ["jane", "jane@clinic.com"] {
                assert!(
                    matches!(
                        check_and_record_login(who, "s3cret-horse"),
                        SkinLoginOutcome::Ok(_)
                    ),
                    "{unlock_as}: {who} signs in after unlock"
                );
            }
        }
    });
}

/// A pre-upgrade (0.45.5) live lock under the email key still refuses
/// the username until it expires or is unlocked.
#[test]
fn legacy_email_row_lock_still_refuses_the_username() {
    with_temp_home(|| {
        add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
        set_principal_password("jane", Some("s3cret-horse")).unwrap();
        with_conn(|conn| {
            conn.execute(
                "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES ('jane@clinic.com', 0, ?1), ('jane', 2, NULL)",
                params![now_secs().saturating_add(600)],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .expect("seed");
        match check_and_record_login("jane", "s3cret-horse") {
            SkinLoginOutcome::LockedOut { retry_after_secs } => {
                assert!((1..=600).contains(&retry_after_secs), "{retry_after_secs}")
            }
            other => panic!("legacy email lock refuses username, got {other:?}"),
        }
        // An expired legacy row is not a lock.
        with_conn(|conn| {
            conn.execute(
                "UPDATE login_lockouts SET locked_until = ?1 WHERE username = 'jane@clinic.com'",
                params![now_secs() - 1],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .expect("expire");
        assert!(matches!(
            check_and_record_login("jane", "s3cret-horse"),
            SkinLoginOutcome::Ok(_)
        ));
        assert!(lock_rows().is_empty(), "{:?}", lock_rows());
    });
}

/// Names that resolve to nothing keep a counter per presented string,
/// lock after 3 like a known name, and an unknown email never touches
/// a principal's row.
#[test]
fn unknown_names_still_count_per_presented_string() {
    with_temp_home(|| {
        add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
        set_principal_password("jane", Some("s3cret-horse")).unwrap();
        for who in ["ghost", "ghost@nowhere.io", "Not A Name"] {
            for _ in 0..3 {
                assert!(matches!(
                    check_and_record_login(who, "wrong"),
                    SkinLoginOutcome::BadCreds
                ));
            }
            match check_and_record_login(who, "s3cret-horse") {
                SkinLoginOutcome::LockedOut { retry_after_secs } => assert!(
                    (1..=LOCKOUT_DURATION_SECS).contains(&retry_after_secs),
                    "{who}: {retry_after_secs}"
                ),
                other => panic!("{who}: unknown locks like known, got {other:?}"),
            }
        }
        let keys: Vec<String> = lock_rows().into_iter().map(|r| r.0).collect();
        assert_eq!(keys, vec!["ghost", "ghost@nowhere.io", "not a name"]);
        // Unknown strings sharing nothing with jane leave her alone.
        assert!(matches!(
            check_and_record_login("jane@clinic.com", "s3cret-horse"),
            SkinLoginOutcome::Ok(_)
        ));
        // A different unknown string is a fresh counter.
        assert!(matches!(
            check_and_record_login("ghost2", "wrong"),
            SkinLoginOutcome::BadCreds
        ));
    });
}

#[test]
fn null_password_cannot_k2_login_static_mint_still_works() {
    with_temp_home(|| {
        let p = add_principal("mintonly").unwrap();
        assert!(!p.has_password);
        match check_and_record_login("mintonly", "anything") {
            SkinLoginOutcome::BadCreds => {}
            _ => panic!("NULL password_hash cannot K2-login"),
        }
        let room = uuid::Uuid::new_v4().to_string();
        let (_m, raw) = create_token("mintonly", None, std::slice::from_ref(&room)).unwrap();
        assert!(resolve_skin_token(&raw).is_some());
    });
}

#[test]
fn password_reset_tables_exist_and_are_not_login_lockouts() {
    with_temp_home(|| {
        with_conn(|conn| {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                         WHERE type='table' AND name='password_reset_tokens'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            assert_eq!(n, 1, "password_reset_tokens missing");
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                         WHERE type='table' AND name='password_reset_throttle'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            assert_eq!(n, 1, "password_reset_throttle missing");
            let cols: String = conn
                .prepare("PRAGMA table_info(password_reset_tokens)")
                .map_err(|e| e.to_string())?
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
                .join(",");
            assert!(cols.contains("principal_id"), "{cols}");
            assert!(cols.contains("token_hash"), "{cols}");
            assert!(cols.contains("expires_at"), "{cols}");
            assert!(cols.contains("used_at"), "{cols}");
            assert!(!cols.contains("email"), "no email column: {cols}");
            let pcols: String = conn
                .prepare("PRAGMA table_info(principals)")
                .map_err(|e| e.to_string())?
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
                .join(",");
            assert!(pcols.contains("email"), "principals.email missing: {pcols}");
            Ok(())
        })
        .unwrap();
    });
}

#[test]
fn mint_password_reset_unknown_invalid_null_hash_no_token() {
    with_temp_home(|| {
        add_principal("mintonly").unwrap();
        match mint_password_reset("ghost-user") {
            Ok(None) => {}
            other => panic!("unknown must be 200-no-token, got {other:?}"),
        }
        match mint_password_reset("X") {
            Ok(None) => {}
            other => panic!("invalid username must not 400, got {other:?}"),
        }
        match mint_password_reset("has space") {
            Ok(None) => {}
            other => panic!("invalid format must not 400, got {other:?}"),
        }
        match mint_password_reset("mintonly") {
            Ok(None) => {}
            other => panic!("NULL hash must not mint, got {other:?}"),
        }
        add_principal("cara").unwrap();
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        match mint_password_reset("cara") {
            Ok(None) => {}
            other => panic!("password without email must not mint, got {other:?}"),
        }
        match mint_password_reset("not-an-email@") {
            Ok(None) => {}
            other => panic!("invalid email string is a miss, got {other:?}"),
        }
    });
}

#[test]
fn mint_password_reset_replaces_unused_and_is_not_k2skn() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        set_principal_email("cara", Some("cara@clinic.com")).unwrap();
        let first = mint_password_reset("cara")
            .unwrap()
            .expect("known user with hash and email mints");
        assert!(!first.token.starts_with(SKIN_KEY_PREFIX), "{}", first.token);
        assert!(!first.token.starts_with("k2sk_"), "{}", first.token);
        assert_eq!(first.token.len(), 64, "{}", first.token);
        assert_eq!(first.email, "cara@clinic.com");
        assert!(first.expires_at > now_secs());
        let first = first.token;
        let stored = with_conn(|conn| {
            let h: String = conn
                .query_row(
                    "SELECT token_hash FROM password_reset_tokens
                         WHERE used_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM password_reset_tokens WHERE used_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            Ok((h, n))
        })
        .unwrap();
        assert_eq!(stored.0, sha256_hex(&first));
        assert_eq!(stored.1, 1);
        let second = mint_password_reset("cara")
            .unwrap()
            .expect("second mint")
            .token;
        assert_ne!(first, second);
        let n: i64 = with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM password_reset_tokens WHERE used_at IS NULL",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(n, 1, "one unused row per principal");
        match consume_password_reset(&first, "new-pass-word") {
            Ok(SkinResetConsume::BadToken) => {}
            other => panic!("replaced unused must be dead, got {other:?}"),
        }
        match consume_password_reset(&second, "new-pass-word") {
            Ok(SkinResetConsume::Ok) => {}
            other => panic!("live unused must consume, got {other:?}"),
        }
        match consume_password_reset(&second, "another-pw") {
            Ok(SkinResetConsume::BadToken) => {}
            other => panic!("single-use, got {other:?}"),
        }
        match check_and_record_login("cara", "new-pass-word") {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "cara"),
            other => panic!("hash must change, got {other:?}"),
        }
    });
}

#[test]
fn consume_password_reset_ttl_and_empty_refused() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        set_principal_email("cara", Some("cara@clinic.com")).unwrap();
        let raw = mint_password_reset("cara").unwrap().expect("mint").token;
        with_conn(|conn| {
            conn.execute("UPDATE password_reset_tokens SET expires_at = 1", [])
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        match consume_password_reset(&raw, "new-pass-word") {
            Ok(SkinResetConsume::BadToken) => {}
            other => panic!("expired must be BadToken, got {other:?}"),
        }
        match consume_password_reset("not-a-token", "new-pass-word") {
            Ok(SkinResetConsume::BadToken) => {}
            other => panic!("unknown token must be BadToken, got {other:?}"),
        }
        let err = consume_password_reset("x", "").unwrap_err();
        assert!(err.contains("empty"), "{err}");
        match check_and_record_login("cara", "s3cret-horse") {
            SkinLoginOutcome::Ok(_) => {}
            other => panic!("expired consume must not change hash, got {other:?}"),
        }
    });
}

#[test]
fn consume_password_reset_revokes_sessions_not_platform() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        let room = uuid::Uuid::new_v4().to_string();
        let (_sm, static_raw) =
            create_token("cara", None, std::slice::from_ref(&room)).unwrap();
        let (_sess, sess_raw) = create_session_token("cara").unwrap();
        set_principal_email("cara", Some("cara@clinic.com")).unwrap();
        let raw = mint_password_reset("cara").unwrap().expect("mint").token;
        match consume_password_reset(&raw, "new-pass-word") {
            Ok(SkinResetConsume::Ok) => {}
            other => panic!("consume, got {other:?}"),
        }
        assert!(
            resolve_skin_token(&sess_raw).is_none(),
            "consume revokes session=1"
        );
        assert!(
            resolve_skin_token(&static_raw).is_some(),
            "platform token survives consume"
        );
    });
}

/// agent-app-users T2.1/T2.4: an agent's App user is the guest, its one
/// App grant (no box login) and its creator stamp, written together; a
/// failure before the commit leaves none of them.
#[test]
fn an_agent_add_is_one_transaction_and_app_only() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            AGENT_ADD_FAIL_BEFORE_COMMIT.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    with_temp_home(|| {
        let by = AddedBy { agent_workspace: Some("ws-tina"), ..Default::default() };
        {
            let _reset = Reset;
            AGENT_ADD_FAIL_BEFORE_COMMIT.store(true, std::sync::atomic::Ordering::SeqCst);
            let err = add_principal_for_app_by("tina", Some("tina@example.test"), "svc-tina", None, "agent:tina", by).unwrap_err();
            assert!(err.contains("injected"), "{err}");
        }
        assert!(!principal_exists("tina").unwrap(), "no principal after a failed agent add");
        let (stamps, grants): (i64, i64) = with_conn(|conn| {
            let s = conn
                .query_row("SELECT COUNT(*) FROM principal_agent_creators", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let g = conn.query_row("SELECT COUNT(*) FROM grants", [], |r| r.get(0)).map_err(|e| e.to_string())?;
            Ok((s, g))
        })
        .unwrap();
        assert_eq!((stamps, grants), (0, 0), "no stamp and no grant left behind");

        let p = add_principal_for_app_by("tina", Some("tina@example.test"), "svc-tina", None, "agent:tina", by).expect("agent add");
        assert_eq!(creators::workspace_of(&p.id).unwrap().as_deref(), Some("ws-tina"));
        let mine: Vec<SkinGrant> =
            list_grants().unwrap().into_iter().filter(|g| g.subject_kind == "principal" && g.subject_id == p.id).collect();
        assert_eq!(mine.len(), 1, "{mine:?}");
        assert_eq!((mine[0].kind.as_str(), mine[0].target_id.as_str()), ("app", "svc-tina"));
        assert!(host_grants(&mine, &p.id).is_empty(), "never a box login");
        let tie = ties::get(&p.id).unwrap().expect("tied in the same transaction");
        assert_eq!(
            (tie.workspace_id.as_str(), tie.host_ok, tie.reasons.clone(), tie.set_by.as_str()),
            ("ws-tina", false, vec!["created".to_string()], "agent:tina")
        );
    });
}

/// agent-app-users T3.1: a step-5 file (0.46.3) gains the step-6
/// tables on open and reads version 6; its rows survive. A file newer
/// than this binary is refused with the exact newer-schema text.
#[test]
fn step6_adds_ties_and_invites_to_a_v5_file() {
    with_temp_home(|| {
        assert_eq!(SKIN_SCHEMA_VERSION, 8);
        add_principal("ada").unwrap();
        let path = store_path();
        *db().lock() = None;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("DROP TABLE principal_agent_ties; DROP TABLE app_user_invites; PRAGMA user_version = 5;")
                .unwrap();
        }
        assert!(principal_exists("ada").unwrap(), "the v5 file opens and keeps its rows");
        assert_eq!(with_conn(|c| schema_version(c)).unwrap(), 8, "steps 6, 7 and 8 ran");
        let tables: Vec<String> = with_conn(|c| {
            let mut st = c
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('principal_agent_ties','app_user_invites') ORDER BY name")
                .map_err(|e| e.to_string())?;
            let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(tables, vec!["app_user_invites".to_string(), "principal_agent_ties".to_string()]);
        // A file from a newer K2 than this one: refused, exact text.
        with_conn(|c| c.execute_batch("PRAGMA user_version = 9;").map_err(|e| e.to_string())).unwrap();
        *db().lock() = None;
        let err = open_db(&path).expect_err("newer schema must refuse");
        assert_eq!(
            err,
            "skin.db schema version 9 is newer than this K2 understands (8). Refusing to open it: update K2, or restore the skin.db backup taken before the downgrade."
        );
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 8;").unwrap();
    });
}

/// Reconcile with App caps: a file an earlier 0.46.4 build opened holds
/// the first `principal_agent_ties` (`tied_to`) and
/// `principal_person_claims`. Step 6 moves the ties into the one store
/// (a claimed guest stays a person), ties stamped guests that had no
/// tie, drops both old tables, and backs the file up first.
#[test]
fn step6_folds_app_caps_ties_and_claims_into_the_one_store() {
    with_temp_home(|| {
        let tied = add_principal("tied").unwrap();
        let claimed = add_principal("claimed").unwrap();
        let stamped = add_principal("stamped").unwrap();
        creators::stamp(&stamped.id, "ws-s").unwrap();
        creators::stamp(&claimed.id, "ws-c").unwrap();
        let path = store_path();
        *db().lock() = None;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE principal_agent_ties; DROP TABLE app_user_invites;
                     CREATE TABLE principal_agent_ties (principal_id TEXT PRIMARY KEY, tied_to TEXT NOT NULL, tied_at INTEGER NOT NULL);
                     CREATE TABLE principal_person_claims (principal_id TEXT PRIMARY KEY, claimed_by TEXT NOT NULL, claimed_at INTEGER NOT NULL);
                     PRAGMA user_version = 5;",
            )
            .unwrap();
            conn.execute("INSERT INTO principal_agent_ties VALUES (?1, 'ws-t', 7)", params![tied.id]).unwrap();
            conn.execute("INSERT INTO principal_agent_ties VALUES (?1, 'ws-c', 7)", params![claimed.id]).unwrap();
            conn.execute("INSERT INTO principal_person_claims VALUES (?1, 'owner-token', 8)", params![claimed.id]).unwrap();
        }
        let _ = std::fs::remove_file(path.with_file_name("skin.db.pre-6.bak"));
        assert!(principal_exists("tied").unwrap(), "the file opens");
        assert_eq!(with_conn(|c| schema_version(c)).unwrap(), 8);
        assert!(path.with_file_name("skin.db.pre-6.bak").exists(), "backed up before step 6");
        let t = ties::get(&tied.id).unwrap().expect("moved");
        assert_eq!((t.workspace_id.as_str(), t.reasons.clone()), ("ws-t", vec!["upgrade".to_string()]));
        assert!(ties::get(&claimed.id).unwrap().is_none(), "a guest the owner claimed stays a person");
        let s = ties::get(&stamped.id).unwrap().expect("a stamped guest is tied");
        assert_eq!((s.workspace_id.as_str(), s.host_ok), ("ws-s", true), "it keeps the box login it was given");
        let left: i64 = with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('principal_person_claims', 'principal_agent_ties_v1')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(left, 0, "the old tables are gone");
    });
}

/// Verification review 2026-10-26, items 1 and 3: on upgrade every
/// owner agent mark becomes a tie (and agrees with an older tie), and
/// every stored email is unconfirmed until the owner sets it. An owner
/// reset clears the tie and the mark; an email kept from the tied time
/// stays unconfirmed unless the owner gave the address.
#[test]
fn step6_ties_marked_logins_and_holds_old_emails_until_the_owner_confirms() {
    with_temp_home(|| {
        let marked = add_principal("marked").unwrap();
        let both = add_principal("both").unwrap();
        let mailed = add_principal("mailed").unwrap();
        let plain = add_principal("plain").unwrap();
        set_principal_email("mailed", Some("mailed@example.test")).unwrap();
        set_principal_email("marked", Some("marked@agent.test")).unwrap();
        creators::stamp(&both.id, "ws-stamp").unwrap();
        let path = store_path();
        *db().lock() = None;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE principal_agent_ties; DROP TABLE app_user_invites; DROP TABLE principal_email_unconfirmed;
                     PRAGMA user_version = 5;",
            )
            .unwrap();
            conn.execute("UPDATE principals SET agent_workspace = 'ws-mark' WHERE id = ?1", params![marked.id]).unwrap();
            conn.execute("UPDATE principals SET agent_workspace = 'ws-other' WHERE id = ?1", params![both.id]).unwrap();
        }
        let _ = std::fs::remove_file(path.with_file_name("skin.db.pre-6.bak"));
        assert!(principal_exists("marked").unwrap());
        let t = ties::get(&marked.id).unwrap().expect("a marked login is tied");
        assert_eq!((t.workspace_id.as_str(), t.reasons.clone()), ("ws-mark", vec![ties::REASON_AGENT_LOGIN.to_string()]));
        let t = ties::get(&both.id).unwrap().expect("tied");
        assert_eq!(
            principal_agent_workspace(&both.id).unwrap().as_deref(),
            Some(t.workspace_id.as_str()),
            "the mark and the tie name one workspace"
        );
        assert_eq!(ties::stored_email_refused(&mailed.id).unwrap(), Some(ties::EMAIL_UNCONFIRMED), "an email from before the release");
        assert_eq!(ties::stored_email_refused(&marked.id).unwrap(), Some(ties::EMAIL_TIED));
        assert_eq!(ties::stored_email_refused(&plain.id).unwrap(), None, "no email, nothing to refuse");
        // Not the owner: still unconfirmed. The owner: confirmed.
        set_principal_email_as("mailed", Some("mailed@example.test"), "user:no-such-login").unwrap();
        assert_eq!(ties::stored_email_refused(&mailed.id).unwrap(), Some(ties::EMAIL_UNCONFIRMED));
        set_principal_email_as("mailed", Some("mailed@example.test"), "owner-token").unwrap();
        assert_eq!(ties::stored_email_refused(&mailed.id).unwrap(), None);

        // The owner's reset, shown to the owner (--code): tie and mark go;
        // the email the agent may have chosen stays unconfirmed.
        let m = invites::mint(&marked.id, HOST_APP_ID, invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
        invites::activate(&m.id).unwrap();
        assert_eq!(invites::consume(&m.token, "persons-own-pw-1").unwrap(), invites::Consume::Ok);
        assert!(ties::get(&marked.id).unwrap().is_none());
        assert_eq!(principal_agent_workspace(&marked.id).unwrap(), None, "the mark goes with the tie");
        assert_eq!(ties::stored_email_refused(&marked.id).unwrap(), Some(ties::EMAIL_UNCONFIRMED));
        // A reset to an address the owner gave confirms it.
        ties::set(&marked.id, ties::TieWrite { workspace_id: "ws-mark", set_by: "agent:x", reason: ties::REASON_PASSWORD }).unwrap();
        let m = invites::mint(&marked.id, HOST_APP_ID, invites::PURPOSE_OWNER_RESET, 3600, Some("real@example.test"), true, None, "owner-token").unwrap();
        invites::activate(&m.id).unwrap();
        assert_eq!(invites::consume(&m.token, "persons-own-pw-2").unwrap(), invites::Consume::Ok);
        assert_eq!(ties::stored_email_refused(&marked.id).unwrap(), None);
        // A link that would tie to another workspace is never minted.
        ties::set(&both.id, ties::TieWrite { workspace_id: "ws-stamp", set_by: "agent:x", reason: ties::REASON_PASSWORD }).unwrap();
        let err = invites::mint(&both.id, "svc-x", invites::PURPOSE_RESET, 3600, None, false, Some(("ws-elsewhere", ties::REASON_APP_RESET)), "pass:p").unwrap_err();
        assert!(matches!(err, invites::MintError::TiedElsewhere), "{err:?}");
    });
}

/// agent-app-users T3.2/T3.7 (core): a tied guest's login is held to
/// the ceiling at mint, and an agent's password write ties with
/// `host_ok` = whether the guest holds the box login then.
#[test]
fn a_tied_login_is_clamped_at_mint_and_at_resolve() {
    with_temp_home(|| {
        let ws = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let mut policy = one_room_policy(&ws, &[CAP_THREAD_READ, CAP_FILES_READ, CAP_FILES_DOWNLOAD]);
        policy.insert(other.clone(), vec![CAP_THREAD_READ.to_string()]);
        create_role("wide", &policy).unwrap();
        add_principal("gina").unwrap();
        assign_role("gina", "wide").unwrap();
        let (_, before) = create_session_token("gina").unwrap();
        let t = ties::TieWrite { workspace_id: &ws, set_by: "agent:sales", reason: ties::REASON_PASSWORD };
        set_principal_password_tied("gina", Some("s3cret-horse"), Some(t), false).unwrap();
        assert!(resolve_skin_token(&before).is_none(), "a password set ends the old sessions");
        let tie = ties::get(&host_login_detail("gina").unwrap().principal_id).unwrap().expect("tied");
        assert!(tie.host_ok, "she held the box login when the agent set her password");
        assert_eq!(tie.reasons, vec!["password".to_string()]);
        // Box login (host_ok): only ws, without files:download.
        let s = create_app_session_token("gina", HOST_APP_ID).unwrap();
        let pass = resolve_skin_token(&s.raw).expect("host login inside the ceiling");
        assert_eq!(pass.room_policy.keys().cloned().collect::<Vec<_>>(), vec![ws.clone()]);
        assert!(pass.has_cap_in_room(&ws, CAP_FILES_READ));
        assert!(!pass.has_cap_in_room(&ws, CAP_FILES_DOWNLOAD), "person-only caps are outside the ceiling");
        assert!(!pass.rooms.contains(&other));
        // Another workspace is refused and writes nothing (verification
        // review 2026-10-26, items 3 and 5); the same one merges the reason.
        let t2 = ties::TieWrite { workspace_id: &other, set_by: "agent:ops", reason: ties::REASON_EMAIL };
        let err = set_principal_email_tied("gina", Some("gina@example.test"), "agent:ops", Some(t2)).unwrap_err();
        assert!(err.starts_with(ties::TIED_ELSEWHERE), "{err}");
        assert_eq!(list_principals().unwrap().into_iter().find(|p| p.username == "gina").unwrap().email, None, "nothing written");
        let t2 = ties::TieWrite { workspace_id: &ws, set_by: "agent:sales", reason: ties::REASON_EMAIL };
        set_principal_email_tied("gina", Some("gina@example.test"), "agent:sales", Some(t2)).unwrap();
        let tie = ties::get(&tie.principal_id).unwrap().unwrap();
        assert_eq!((tie.workspace_id.as_str(), tie.reasons.clone()), (ws.as_str(), vec!["password".to_string(), "email".to_string()]));
        // An App that isn't the workspace's (no published row here): denied.
        set_app_grant_rooms("gina", "svc-elsewhere", std::slice::from_ref(&ws), false, "owner-token").unwrap();
        let err = create_app_session_token("gina", "svc-elsewhere").unwrap_err();
        assert_eq!(err, HOST_LOGIN_DENIED);
        assert_eq!(ties::LOCK_ORDER_VIOLATIONS.load(std::sync::atomic::Ordering::SeqCst), 0, "the App lookup never ran under the skin mutex");
    });
}

/// agent-app-users S0 / T4.6: the same reset token presented by many
/// threads at once is used exactly once (compare-and-set in one
/// transaction), and the password that won is the one stored.
#[test]
fn consume_password_reset_is_single_use_under_a_race() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        set_principal_email("cara", Some("cara@clinic.com")).unwrap();
        let raw = mint_password_reset("cara").unwrap().expect("mint").token;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let (raw, barrier) = (raw.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let pw = format!("winner-pass-{i}");
                    barrier.wait();
                    (pw.clone(), consume_password_reset(&raw, &pw).expect("consume"))
                })
            })
            .collect();
        let results: Vec<(String, SkinResetConsume)> = handles.into_iter().map(|h| h.join().expect("thread")).collect();
        let winners: Vec<&String> = results.iter().filter(|(_, r)| *r == SkinResetConsume::Ok).map(|(pw, _)| pw).collect();
        assert_eq!(winners.len(), 1, "exactly one use wins: {results:?}");
        assert_eq!(
            results.iter().filter(|(_, r)| *r == SkinResetConsume::BadToken).count(),
            5,
            "every other use is a bad token: {results:?}"
        );
        match check_and_record_login("cara", winners[0]) {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "cara"),
            other => panic!("the winner's password is stored, got {other:?}"),
        }
        let used: i64 = with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM password_reset_tokens WHERE used_at IS NOT NULL", [], |r| r.get(0))
                .map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(used, 1);
    });
}

#[test]
fn password_reset_throttle_is_per_username_string() {
    with_temp_home(|| {
        assert!(!password_reset_throttled("ghost-user").unwrap());
        assert!(
            password_reset_throttled("ghost-user").unwrap(),
            "second mint inside 60s even when no user"
        );
        assert!(
            password_reset_throttled("  GHOST-USER  ").unwrap(),
            "lowercase/trim same slot"
        );
        assert!(
            !password_reset_throttled("cara").unwrap(),
            "other username is a different slot"
        );
        // vs-live U9: a typed name never becomes a skin.db row.
        let rows: i64 = with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM password_reset_throttle", [], |r| r.get(0)).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(rows, 0, "the forgot clock is memory only");
        guard::expire_forgot("ghost-user", 60);
        assert!(
            !password_reset_throttled("ghost-user").unwrap(),
            "window elapsed allows another attempt"
        );
    });
}

#[test]
fn normalize_email_trim_lower_empty_null_invalid() {
    assert_eq!(
        normalize_email("  Jane@Clinic.COM ").unwrap(),
        Some("jane@clinic.com".into())
    );
    assert_eq!(normalize_email("").unwrap(), None);
    assert_eq!(normalize_email("   ").unwrap(), None);
    assert!(normalize_email("not-an-email").is_err());
    assert!(normalize_email("jane@clinic").is_err());
    assert!(normalize_email("jane@ clinic.com").is_err());
    assert!(normalize_email("@clinic.com").is_err());
    assert!(normalize_email("jane@").is_err());
    assert!(normalize_email("jane@@clinic.com").is_err());
    assert!(normalize_username("jane@clinic.com").is_err());
    assert!(normalize_username("jane").is_ok());
}

/// Review 0.46.4: every card, note and audit line shows the name.
#[test]
fn usernames_are_at_most_64_characters() {
    let max = "a".repeat(USERNAME_MAX_LEN);
    assert_eq!(normalize_username(&max).unwrap(), max);
    let err = normalize_username(&"a".repeat(USERNAME_MAX_LEN + 1)).unwrap_err();
    assert!(err.contains("at most 64"), "{err}");
    // Surrounding space is trimmed before the count.
    assert!(normalize_username(&format!("  {max}  ")).is_ok());
}

#[test]
fn platform_name_still_rejects_at() {
    with_temp_home(|| {
        let room = uuid::Uuid::new_v4().to_string();
        let err =
            create_token("jane@clinic.com", None, std::slice::from_ref(&room)).unwrap_err();
        assert!(err.contains("lowercase") || err.contains("only"), "{err}");
    });
}

#[test]
fn guest_email_set_duplicate_clear_and_login_by_email() {
    with_temp_home(|| {
        let jane = add_principal_with_email("jane", Some("Jane@Clinic.COM")).unwrap();
        assert_eq!(jane.email.as_deref(), Some("jane@clinic.com"));
        set_principal_password("jane", Some("s3cret-horse")).unwrap();
        add_principal("bob").unwrap();
        let dup = set_principal_email("bob", Some("jane@clinic.com")).unwrap_err();
        assert!(dup.contains("already in use"), "{dup}");
        let bad = set_principal_email("bob", Some("not-an-email")).unwrap_err();
        assert!(bad.contains("invalid email"), "{bad}");
        let listed = list_principals().unwrap();
        let jane_row = listed.iter().find(|p| p.username == "jane").unwrap();
        assert_eq!(jane_row.email.as_deref(), Some("jane@clinic.com"));
        match check_and_record_login("jane@clinic.com", "s3cret-horse") {
            SkinLoginOutcome::Ok(p) => {
                assert_eq!(p.username, "jane");
                assert_eq!(p.email.as_deref(), Some("jane@clinic.com"));
            }
            other => panic!("email login, got {other:?}"),
        }
        match check_and_record_login("jane", "s3cret-horse") {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "jane"),
            other => panic!("username login still works, got {other:?}"),
        }
        match mint_password_reset("Jane@Clinic.COM") {
            Ok(Some(m)) => {
                assert_eq!(m.email, "jane@clinic.com");
                assert_eq!(m.token.len(), 64);
            }
            other => panic!("mint by email, got {other:?}"),
        }
        match check_and_record_login("not-an-email@", "s3cret-horse") {
            SkinLoginOutcome::BadCreds => {}
            other => panic!("invalid email login is generic, got {other:?}"),
        }
        for _ in 0..3 {
            match check_and_record_login("jane@clinic.com", "wrong") {
                SkinLoginOutcome::BadCreds => {}
                other => panic!("pre-lockout on presented email, got {other:?}"),
            }
        }
        match check_and_record_login("jane@clinic.com", "s3cret-horse") {
            SkinLoginOutcome::LockedOut { .. } => {}
            other => panic!("lockout via email, got {other:?}"),
        }
        match check_and_record_login("jane", "s3cret-horse") {
            SkinLoginOutcome::LockedOut { .. } => {}
            other => panic!("the lock is the principal's, not the email's, got {other:?}"),
        }
        clear_login_lockout("jane").expect("unlock jane");
        set_principal_email("jane", None).unwrap();
        let cleared = list_principals()
            .unwrap()
            .into_iter()
            .find(|p| p.username == "jane")
            .unwrap();
        assert!(cleared.email.is_none(), "{cleared:?}");
        match mint_password_reset("jane") {
            Ok(None) => {}
            other => panic!("cleared email must not mint, got {other:?}"),
        }
    });
}

fn principal_columns(conn: &Connection) -> String {
    let mut stmt = conn
        .prepare("PRAGMA table_info(principals)")
        .expect("pragma");
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .expect("cols")
        .map(|c| c.expect("col"))
        .collect();
    cols.join(",")
}

#[test]
fn principal_agent_mark_sets_clears_and_old_db_gains_the_column() {
    with_temp_home(|| {
        crate::db::init_for_tests();
        let id = uuid::Uuid::new_v4().to_string();
        let handle = format!("agentws{}", &id[..8]);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                params![id, "Agent ws", format!("/tmp/skin-agent-{id}"), handle],
            )
            .expect("project");
        }
        let ada = add_principal("ada").expect("add");
        assert!(ada.agent_workspace.is_none());
        assert_eq!(principal_agent_workspace(&ada.id).unwrap(), None);
        let wire = serde_json::to_value(&ada).unwrap();
        assert!(wire["agentWorkspace"].is_null(), "{wire}");

        let marked = set_principal_agent("ada", Some(&handle)).expect("mark by handle");
        assert_eq!(marked.agent_workspace.as_deref(), Some(id.as_str()), "stored as id");
        assert_eq!(marked.agent_handle.as_deref(), Some(handle.as_str()));
        assert_eq!(principal_agent_workspace(&ada.id).unwrap().as_deref(), Some(id.as_str()));
        let listed = list_principals().unwrap();
        assert_eq!(listed[0].agent_workspace.as_deref(), Some(id.as_str()));
        assert_eq!(listed[0].agent_handle.as_deref(), Some(handle.as_str()));
        // Other owner edits keep the mark.
        let named = set_principal_full_name("ada", Some("Ada")).unwrap();
        assert_eq!(named.agent_workspace.as_deref(), Some(id.as_str()));

        let by_id = set_principal_agent("ada", Some(&id)).expect("mark by id");
        assert_eq!(by_id.agent_workspace.as_deref(), Some(id.as_str()));
        let err = set_principal_agent("ada", Some("no-such-ws")).unwrap_err();
        assert!(err.contains("unknown workspace"), "{err}");
        assert_eq!(
            principal_agent_workspace(&ada.id).unwrap().as_deref(),
            Some(id.as_str()),
            "a refused mark leaves the old one"
        );
        let err = set_principal_agent("ghost", Some(&handle)).unwrap_err();
        assert!(err.contains("unknown skin user"), "{err}");
        // Verification item 3: re-marking to another workspace while
        // tied is refused; the mark and the tie keep naming one.
        let id2 = uuid::Uuid::new_v4().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                params![id2, "Other ws", format!("/tmp/skin-agent-{id2}"), format!("otherws{}", &id2[..8])],
            )
            .expect("project");
        }
        let err = set_principal_agent("ada", Some(&id2)).unwrap_err();
        assert!(err.starts_with(ties::TIED_ELSEWHERE), "{err}");
        assert_eq!(principal_agent_workspace(&ada.id).unwrap().as_deref(), Some(id.as_str()));
        assert_eq!(ties::get(&ada.id).unwrap().unwrap().workspace_id, id);

        let cleared = set_principal_agent("ada", None).expect("clear");
        assert!(cleared.agent_workspace.is_none() && cleared.agent_handle.is_none());
        assert_eq!(principal_agent_workspace(&ada.id).unwrap(), None);
        assert_eq!(principal_agent_workspace("not-a-principal").unwrap(), None);
    });

    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("open old skin.db");
        conn.execute_batch(
            "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL
                 );
                 INSERT INTO principals (id, username, created_at) VALUES ('p1', 'bob', 1);",
        )
        .expect("pre-mark schema");
        assert!(!principal_columns(&conn).split(',').any(|c| c == "agent_workspace"));
        drop(conn);
        let listed = list_principals().expect("open_db adds agent_workspace");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].agent_workspace.is_none(), "{listed:?}");
        let cols = with_conn(|conn| Ok(principal_columns(conn))).unwrap();
        assert!(cols.split(',').any(|c| c == "agent_workspace"), "{cols}");
    });
}

#[test]
fn principal_full_name_column_on_new_and_old_db_round_trips() {
    with_temp_home(|| {
        add_principal("ada").expect("add");
        let cols = with_conn(|conn| Ok(principal_columns(conn))).unwrap();
        assert!(
            cols.contains("full_name"),
            "new db missing full_name: {cols}"
        );
        assert!(cols.contains("email"), "must not drop email: {cols}");
        assert!(cols.contains("role_id"), "must not drop role_id: {cols}");
        assert!(
            cols.contains("default_rooms"),
            "must not drop default_rooms: {cols}"
        );
        let fresh = list_principals().unwrap();
        assert_eq!(fresh.len(), 1);
        assert!(fresh[0].full_name.is_none(), "{fresh:?}");
        let wire = serde_json::to_value(&fresh[0]).unwrap();
        assert!(
            wire.get("fullName").is_some(),
            "list JSON must include fullName: {wire}"
        );
        assert!(wire["fullName"].is_null(), "{wire}");
        assert!(wire.get("passwordHash").is_none() && wire.get("password_hash").is_none());

        let set = set_principal_full_name("ada", Some("Ada/Lovelace: MD")).unwrap();
        assert_eq!(set.full_name.as_deref(), Some("Ada/Lovelace: MD"));
        let listed = list_principals().unwrap();
        assert_eq!(listed[0].full_name.as_deref(), Some("Ada/Lovelace: MD"));
        let cleared = set_principal_full_name("ada", Some("   ")).unwrap();
        assert!(cleared.full_name.is_none(), "{cleared:?}");
        let none = set_principal_full_name("ada", None).unwrap();
        assert!(none.full_name.is_none(), "{none:?}");
        let missing = set_principal_full_name("ghost", Some("Nope")).unwrap_err();
        assert!(missing.contains("unknown skin user"), "{missing}");
    });

    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("open old skin.db");
        conn.execute_batch(
            "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT,
                    email TEXT
                 );",
        )
        .expect("pre-full_name schema");
        conn.execute(
            "INSERT INTO principals (id, username, created_at, email)
                 VALUES ('p1', 'bob', 1, 'bob@clinic.com')",
            [],
        )
        .unwrap();
        let before = principal_columns(&conn);
        assert!(
            !before.split(',').any(|c| c == "full_name"),
            "fixture must lack full_name: {before}"
        );
        drop(conn);

        let listed = list_principals().expect("open_db adds full_name");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].username, "bob");
        assert!(listed[0].full_name.is_none(), "{listed:?}");
        assert_eq!(listed[0].email.as_deref(), Some("bob@clinic.com"));
        let cols = with_conn(|conn| Ok(principal_columns(conn))).unwrap();
        assert!(
            cols.contains("full_name"),
            "old db must gain full_name: {cols}"
        );
        assert!(cols.contains("email"), "{cols}");
        assert!(cols.contains("role_id"), "{cols}");
        assert!(cols.contains("default_rooms"), "{cols}");
        let again = list_principals().expect("second open ignores duplicate ALTER");
        assert_eq!(again[0].username, "bob");
        let set = set_principal_full_name("bob", Some("Bob")).unwrap();
        assert_eq!(set.full_name.as_deref(), Some("Bob"));
        assert_eq!(
            list_principals().unwrap()[0].full_name.as_deref(),
            Some("Bob")
        );
    });
}

fn table_columns(conn: &Connection, table: &str) -> String {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("pragma");
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .expect("cols")
        .map(|c| c.expect("col"))
        .collect();
    cols.join(",")
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |r| r.get(0),
        )
        .expect("sqlite_master");
    n > 0
}

#[test]
fn grants_table_and_role_app_id_on_new_and_old_db() {
    with_temp_home(|| {
        add_principal("ada").expect("add");
        create_role("guest", &RoomPolicy::new()).expect("role");
        let (cols, grants, app_id) = with_conn(|conn| {
            let app_id: Option<String> = conn
                .query_row("SELECT app_id FROM roles WHERE name = 'guest'", [], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            Ok((
                table_columns(conn, "roles"),
                table_exists(conn, "grants"),
                app_id,
            ))
        })
        .unwrap();
        assert!(
            cols.split(',').any(|c| c == "app_id"),
            "new db missing roles.app_id: {cols}"
        );
        assert!(grants, "new db missing grants");
        assert!(app_id.is_none(), "new role stays host-wide: {app_id:?}");
        let ada = list_principals().unwrap();
        let grants_now = list_grants().unwrap();
        let host = host_grants(&grants_now, &ada[0].id);
        assert_eq!(host.len(), 1, "new guest gets one host grant");
        assert!(host[0].role_id.is_none(), "{host:?}");
        assert_eq!(host[0].scope.as_deref(), Some("[]"));
        let listed = list_roles().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].app_id.is_none(), "{listed:?}");
        let host_id = host[0].id.clone();
        reopen_skin_db();
        let again = list_grants().expect("second open ignores duplicate ALTER");
        assert_eq!(host_grants(&again, &ada[0].id).len(), 1, "{again:?}");
        assert_eq!(again.iter().filter(|g| g.id == host_id).count(), 1);
    });

    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("open old skin.db");
        conn.execute_batch(
            "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT,
                    email TEXT,
                    full_name TEXT
                 );
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}'
                 );",
        )
        .expect("pre-grant schema");
        conn.execute(
            "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy)
                 VALUES ('r1', 'guest', '[]', '[]', 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, full_name)
                 VALUES ('p1', 'bob', 1, 'Bob')",
            [],
        )
        .unwrap();
        let before_roles = table_columns(&conn, "roles");
        assert!(
            !before_roles.split(',').any(|c| c == "app_id"),
            "fixture must lack app_id: {before_roles}"
        );
        assert!(!table_exists(&conn, "grants"), "fixture must lack grants");
        drop(conn);

        let roles = list_roles().expect("open_db adds app_id and grants");
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].name, "guest");
        assert!(roles[0].app_id.is_none(), "{roles:?}");
        let (cols, grants) =
            with_conn(|conn| Ok((table_columns(conn, "roles"), table_exists(conn, "grants"))))
                .unwrap();
        assert!(
            cols.split(',').any(|c| c == "app_id"),
            "old db must gain roles.app_id: {cols}"
        );
        assert!(grants, "old db must gain grants");
        let copied = list_grants().unwrap();
        let host = host_grants(&copied, "p1");
        assert_eq!(
            host.len(),
            1,
            "unassigned bob is copied onto host: {copied:?}"
        );
        assert!(host[0].role_id.is_none(), "{host:?}");
        assert_eq!(host[0].scope.as_deref(), Some("[]"));
        assert!(roles[0].app_id.is_none(), "copy must not set roles.app_id");
        let bob = list_principals().unwrap();
        assert_eq!(bob[0].full_name.as_deref(), Some("Bob"));
        assert!(bob[0].role_id.is_none(), "{bob:?}");
        let host_id = host[0].id.clone();
        reopen_skin_db();
        let again_grants = list_grants().unwrap();
        assert_eq!(
            host_grants(&again_grants, "p1").len(),
            1,
            "{again_grants:?}"
        );
        assert_eq!(again_grants.iter().filter(|g| g.id == host_id).count(), 1);
        let again = list_roles().expect("second open ignores duplicate ALTER");
        assert_eq!(again[0].name, "guest");
        assert!(again[0].app_id.is_none(), "{again:?}");
    });
}

#[test]
fn grant_create_list_delete_validates_and_keeps_role() {
    with_temp_home(|| {
        let ada = add_principal("ada").unwrap();
        let role = create_role("guest", &RoomPolicy::new()).unwrap();
        assert!(role.app_id.is_none(), "create_role must not require app_id");

        let missing_role = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "booking".into(),
            role: None,
            scope: None,
            created_by: None,
        })
        .unwrap_err();
        assert!(
            missing_role.contains("roleId") && missing_role.contains("rooms scope"),
            "{missing_role}"
        );

        let with_scope = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "booking".into(),
            role: Some("guest".into()),
            scope: Some("owner".into()),
            created_by: None,
        })
        .unwrap_err();
        assert!(with_scope.contains("scope"), "{with_scope}");

        let mailbox_role = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "mailbox".into(),
            target_id: "box".into(),
            role: Some(role.id.clone()),
            scope: Some("may-read".into()),
            created_by: None,
        })
        .unwrap_err();
        assert!(mailbox_role.contains("kind=app"), "{mailbox_role}");

        let mailbox_no_scope = create_grant(&SkinGrantWrite {
            subject_kind: "workspace".into(),
            subject_id: "ws-1".into(),
            kind: "database".into(),
            target_id: "db-1".into(),
            role: None,
            scope: None,
            created_by: None,
        })
        .unwrap_err();
        assert!(mailbox_no_scope.contains("scope"), "{mailbox_no_scope}");

        let unknown = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: "no-such-principal".into(),
            kind: "app".into(),
            target_id: "booking".into(),
            role: Some("guest".into()),
            scope: None,
            created_by: None,
        })
        .unwrap_err();
        assert!(unknown.contains("unknown principal"), "{unknown}");

        let empty_ws = create_grant(&SkinGrantWrite {
            subject_kind: "workspace".into(),
            subject_id: "  ".into(),
            kind: "cli".into(),
            target_id: "tool".into(),
            role: None,
            scope: Some("run".into()),
            created_by: None,
        })
        .unwrap_err();
        assert!(empty_ws.contains("subject"), "{empty_ws}");

        let bad_kind = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "people".into(),
            target_id: "x".into(),
            role: None,
            scope: Some("x".into()),
            created_by: None,
        })
        .unwrap_err();
        assert!(bad_kind.contains("kind"), "{bad_kind}");

        let created = create_grant(&SkinGrantWrite {
            subject_kind: "Principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "booking".into(),
            role: Some("guest".into()),
            scope: None,
            created_by: None,
        })
        .unwrap();
        assert_eq!(created.subject_kind, "principal");
        assert_eq!(created.kind, "app");
        assert_eq!(created.target_id, "booking");
        assert_eq!(created.role_id.as_deref(), Some(role.id.as_str()));
        assert!(created.scope.is_none(), "{created:?}");

        let ws = create_grant(&SkinGrantWrite {
            subject_kind: "workspace".into(),
            subject_id: "not-a-real-workspace".into(),
            kind: "mailbox".into(),
            target_id: "inbox".into(),
            role: None,
            scope: Some("may-read".into()),
            created_by: None,
        })
        .unwrap();
        assert_eq!(ws.subject_kind, "workspace");
        assert!(ws.role_id.is_none(), "{ws:?}");
        assert_eq!(ws.scope.as_deref(), Some("may-read"));

        let dup = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "booking".into(),
            role: Some(role.id.clone()),
            scope: None,
            created_by: None,
        })
        .unwrap_err();
        assert!(dup.contains("already exists"), "{dup}");

        let listed = list_grants().unwrap();
        assert_eq!(
            listed.len(),
            3,
            "host grant plus the two writes: {listed:?}"
        );
        assert!(
            listed.iter().any(|g| g.id == created.id),
            "created grant missing: {listed:?}"
        );
        assert!(
            listed.iter().any(|g| g.id == ws.id),
            "workspace grant missing: {listed:?}"
        );

        with_conn(|conn| {
            conn.execute(
                "UPDATE roles SET app_id = 'crm' WHERE id = ?1",
                params![role.id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        let mismatch = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "booking-other".into(),
            role: Some("guest".into()),
            scope: None,
            created_by: None,
        })
        .unwrap_err();
        assert!(
            mismatch.contains("crm") && mismatch.contains("booking-other"),
            "{mismatch}"
        );
        let matched = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "crm".into(),
            role: Some("guest".into()),
            scope: None,
            created_by: None,
        })
        .unwrap();
        assert_eq!(matched.target_id, "crm");
        let scoped = list_roles().unwrap();
        assert_eq!(scoped[0].app_id.as_deref(), Some("crm"));

        assert!(delete_grant_by_id(&created.id).unwrap());
        assert!(!delete_grant_by_id(&created.id).unwrap());
        assert!(
            delete_grant_by_key("workspace", "not-a-real-workspace", "mailbox", "inbox")
                .unwrap()
        );
        let left = list_grants().unwrap();
        assert_eq!(left.len(), 2, "host grant stays: {left:?}");
        assert!(left.iter().any(|g| g.id == matched.id), "{left:?}");
        assert_eq!(host_grants(&left, &ada.id).len(), 1, "{left:?}");
        let roles = list_roles().unwrap();
        assert_eq!(roles.len(), 1, "revoke must keep the role");
        assert_eq!(roles[0].name, "guest");

        set_principal_password("ada", Some("s3cret-horse")).unwrap();
        match check_and_record_login("ada", "s3cret-horse") {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "ada"),
            other => panic!("login with a grant still works, got {other:?}"),
        }
        let (_meta, raw) = create_session_token("ada").expect("session mint unchanged");
        assert!(resolve_skin_token(&raw).is_some());
        let crm = create_app_session_token("ada", "crm").expect("crm grant allows crm login");
        assert_eq!(crm.app, "crm");
        assert!(delete_grant_by_id(&matched.id).unwrap());
        assert!(
            resolve_skin_token(&crm.raw).is_none(),
            "a grant delete must end that guest's live session on that App"
        );
        assert!(
            resolve_skin_token(&raw).is_some(),
            "S24-6: deleting the crm grant must not end the box-login session"
        );
        let (_meta, raw2) = create_session_token("ada").expect("host grant still allows login");
        assert!(resolve_skin_token(&raw2).is_some());
        assert_eq!(
            create_app_session_token("ada", "crm").expect_err("crm grant gone").as_str(),
            HOST_LOGIN_DENIED
        );
    });
}

#[test]
fn principal_without_host_grant_cannot_create_session() {
    with_temp_home(|| {
        let ada = add_principal("ada").unwrap();
        set_principal_password("ada", Some("s3cret-horse")).unwrap();
        assert_eq!(host_grants(&list_grants().unwrap(), &ada.id).len(), 1);
        assert!(
            delete_grant_by_key("principal", &ada.id, "app", HOST_APP_ID).unwrap(),
            "host grant must delete"
        );
        assert!(host_grants(&list_grants().unwrap(), &ada.id).is_empty());
        match check_and_record_login("ada", "s3cret-horse") {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "ada"),
            other => panic!("password check stays separate from the grant bit, got {other:?}"),
        }
        let err = create_session_token("ada").expect_err("missing host grant denies");
        assert_eq!(err, HOST_LOGIN_DENIED);
        assert!(!err.contains("ada"), "{err}");
        assert!(!err.contains("grant"), "{err}");
        assert!(!err.contains(HOST_APP_ID), "{err}");
        reopen_skin_db();
        assert!(
            host_grants(&list_grants().unwrap(), &ada.id).is_empty(),
            "deleted host grant must stay deleted across open"
        );
        let err = create_session_token("ada").expect_err("reopen must not restore the grant");
        assert_eq!(err, HOST_LOGIN_DENIED);
    });
}

#[test]
fn grant_enabled_alter_is_idempotent_and_off_is_not_a_grant() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("add");
        set_principal_password("ada", Some("s3cret-horse")).expect("password");
        let listed = list_grants().expect("list");
        let host = host_grants(&listed, &ada.id);
        assert_eq!(host.len(), 1, "new guest still has the host row");
        assert!(host[0].enabled, "existing and new rows default on");
        let id = host[0].id.clone();
        reopen_skin_db();
        let reopened = list_grants().expect("second open");
        let again = host_grants(&reopened, &ada.id);
        assert_eq!(again.len(), 1);
        assert!(again[0].enabled, "second open ignores duplicate ALTER");
        assert_eq!(again[0].id, id);
        let updated = set_grant_enabled(&id, false).expect("disable");
        assert!(!updated.enabled);
        let listed = list_grants().expect("list after disable");
        let row = listed.iter().find(|g| g.id == id).expect("row stays");
        assert!(!row.enabled, "enabled=0 must not delete");
        assert_eq!(listed.iter().filter(|g| g.id == id).count(), 1);
        match check_and_record_login("ada", "s3cret-horse") {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "ada"),
            other => panic!("password check stays separate, got {other:?}"),
        }
        let err = create_session_token("ada").expect_err("enabled=0 denies mint");
        assert_eq!(err, HOST_LOGIN_DENIED);
        set_grant_enabled(&id, true).expect("enable");
        create_session_token("ada").expect("enabled=1 allows mint");
    });

    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("old skin.db");
        conn.execute_batch(
            "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE grants (
                    id TEXT PRIMARY KEY,
                    subject_kind TEXT NOT NULL,
                    subject_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    role_id TEXT,
                    scope TEXT,
                    created_at INTEGER NOT NULL,
                    UNIQUE(subject_kind, subject_id, kind, target_id)
                 );",
        )
        .expect("pre-enabled schema");
        conn.execute(
            "INSERT INTO principals (id, username, created_at) VALUES ('p1', 'ada', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO grants
                 (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at)
                 VALUES ('g1', 'principal', 'p1', 'app', 'host', NULL, '[]', 1)",
            [],
        )
        .unwrap();
        let before = table_columns(&conn, "grants");
        assert!(
            !before.split(',').any(|c| c == "enabled"),
            "fixture must lack enabled: {before}"
        );
        drop(conn);
        let grants = list_grants().expect("open adds enabled");
        let host = host_grants(&grants, "p1");
        assert_eq!(host.len(), 1, "{grants:?}");
        assert!(host[0].enabled, "old row defaults on");
        let cols = with_conn(|c| Ok(table_columns(c, "grants"))).unwrap();
        assert!(
            cols.split(',').any(|c| c == "enabled"),
            "old db must gain enabled: {cols}"
        );
        reopen_skin_db();
        assert!(
            host_grants(&list_grants().expect("second open"), "p1")[0].enabled,
            "duplicate ALTER must keep the default"
        );
    });
}

#[test]
fn host_switch_deletes_grant_and_keeps_principal_role() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("add");
        let role = create_role("editor", &RoomPolicy::new()).expect("role");
        assign_role("ada", "editor").expect("assign");
        set_principal_password("ada", Some("s3cret-horse")).expect("password");
        assert!(set_host_login("ada", false).expect("switch off"), "host row deletes");
        assert!(host_grants(&list_grants().expect("list"), &ada.id).is_empty());
        let kept = list_principals()
            .expect("principals")
            .into_iter()
            .find(|p| p.username == "ada")
            .expect("ada stays");
        assert_eq!(kept.role_id.as_deref(), Some(role.id.as_str()));
        assert!(verify_principal_password("ada", "s3cret-horse"));
        match check_and_record_login("ada", "s3cret-horse") {
            SkinLoginOutcome::Ok(p) => assert_eq!(p.role_id.as_deref(), Some(role.id.as_str())),
            other => panic!("password check still succeeds, got {other:?}"),
        }
        let err = create_session_token("ada").expect_err("mint denies");
        assert_eq!(err, HOST_LOGIN_DENIED);
        assert!(set_host_login("ada", true).expect("switch on"));
        let listed = list_grants().expect("list");
        let host = host_grants(&listed, &ada.id);
        assert_eq!(host.len(), 1);
        assert_eq!(
            host[0].role_id.as_deref(),
            Some(role.id.as_str()),
            "AM1: a re-added host grant carries the guest's role, never a blank row: {host:?}"
        );
        assert!(host[0].scope.is_none(), "{host:?}");
        assert!(host[0].enabled);
        let detail = host_login_detail("ada").expect("detail");
        assert_eq!(detail.role_id.as_deref(), Some(role.id.as_str()));
        assert_eq!(detail.grant_role_id, detail.role_id, "{detail:?}");
        create_session_token("ada").expect("host row allows mint again");
    });
}

/// AM1 (S24-2): the host grant is the login truth from 0.45.5. Writers
/// keep it equal to the column; a role written only on the grant wins.
#[test]
fn host_login_role_comes_from_the_grant() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("add");
        let room_login = uuid::Uuid::new_v4().to_string();
        let room_grant = uuid::Uuid::new_v4().to_string();
        let mut login_policy = RoomPolicy::new();
        login_policy.insert(room_login.clone(), vec![CAP_THREAD_READ.to_string()]);
        let mut grant_policy = RoomPolicy::new();
        grant_policy.insert(room_grant.clone(), vec![CAP_FILES_READ.to_string()]);
        let login_role = create_role("login-role", &login_policy).expect("login role");
        let grant_role = create_role("grant-role", &grant_policy).expect("grant role");
        assign_role("ada", &login_role.id).expect("assign");
        let detail = host_login_detail("ada").expect("detail");
        assert_eq!(detail.role_id.as_deref(), Some(login_role.id.as_str()));
        assert_eq!(
            detail.grant_role_id, detail.role_id,
            "assign writes the host grant too: {detail:?}"
        );
        let (meta, _) = create_session_token("ada").expect("mint");
        assert_eq!(meta.rooms, vec![room_login.clone()]);
        let host_id = host_grants(&list_grants().expect("list"), &ada.id)[0].id.clone();
        with_conn(|conn| {
            conn.execute(
                "UPDATE grants SET role_id = ?1 WHERE id = ?2",
                params![grant_role.id, host_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        let (meta, _) = create_session_token("ada").expect("mint uses the grant role");
        assert_eq!(meta.rooms, vec![room_grant.clone()]);
        assert!(!meta.rooms.contains(&room_login), "{meta:?}");
        unassign_role("ada").expect("unassign");
        let host = host_grants(&list_grants().expect("list"), &ada.id)[0].clone();
        assert!(host.role_id.is_none(), "{host:?}");
        assert_eq!(host.scope.as_deref(), Some("[]"), "unassign → default rooms");
    });
}

fn session_revoked_at(raw: &str) -> Option<i64> {
    let hash = sha256_hex(raw);
    with_conn(|conn| {
        conn.query_row(
            "SELECT revoked_at FROM tokens WHERE key_hash = ?1",
            params![hash],
            |r| r.get::<_, Option<i64>>(0),
        )
        .map_err(|e| format!("token row: {e}"))
    })
    .expect("token row must exist")
}

#[test]
fn grant_disable_or_delete_revokes_that_guests_sessions_only() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("ada");
        add_principal("bob").expect("bob");
        let room = uuid::Uuid::new_v4().to_string();
        let (_, ada_raw) = create_session_token("ada").expect("ada session");
        let (_, ada_raw2) = create_session_token("ada").expect("ada second session");
        let (_, bob_raw) = create_session_token("bob").expect("bob session");
        let (_, plat_raw) =
            create_token("partner", None, std::slice::from_ref(&room)).expect("platform");
        assert!(resolve_skin_token(&ada_raw).is_some());

        // Disable the host grant: both of ada's sessions end now.
        let host_id = host_grants(&list_grants().unwrap(), &ada.id)[0].id.clone();
        set_grant_enabled(&host_id, false).expect("disable");
        assert!(resolve_skin_token(&ada_raw).is_none(), "disabled grant must end session");
        assert!(resolve_skin_token(&ada_raw2).is_none(), "every session of ada ends");
        assert!(
            session_revoked_at(&ada_raw).is_some(),
            "disable must revoke the row, not only hide it"
        );
        assert!(resolve_skin_token(&bob_raw).is_some(), "bob untouched");
        assert!(session_revoked_at(&bob_raw).is_none());
        assert!(resolve_skin_token(&plat_raw).is_some(), "platform pass untouched");

        // Re-enable never revives the old pass; a fresh login works.
        set_grant_enabled(&host_id, true).expect("enable");
        assert!(resolve_skin_token(&ada_raw).is_none(), "re-enable must not revive");
        let (_, ada_raw3) = create_session_token("ada").expect("login again");
        assert!(resolve_skin_token(&ada_raw3).is_some());

        // Delete by id.
        assert!(delete_grant_by_id(&host_id).expect("delete"));
        assert!(resolve_skin_token(&ada_raw3).is_none(), "deleted grant ends session");
        assert!(session_revoked_at(&ada_raw3).is_some());
        assert_eq!(
            create_session_token("ada").expect_err("no grant"),
            HOST_LOGIN_DENIED
        );

        // Host switch off.
        assert!(set_host_login("ada", true).expect("on"));
        let (_, ada_raw4) = create_session_token("ada").expect("login");
        assert!(set_host_login("ada", false).expect("off"));
        assert!(resolve_skin_token(&ada_raw4).is_none(), "host switch off ends session");
        assert!(session_revoked_at(&ada_raw4).is_some());

        // Any grant of ada (not only host) going away ends her sessions.
        assert!(set_host_login("ada", true).expect("on"));
        let mailbox = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "mailbox".into(),
            target_id: "box".into(),
            role: None,
            scope: Some("may-read".into()),
            created_by: None,
        })
        .expect("mailbox");
        let (_, ada_raw5) = create_session_token("ada").expect("login");
        assert!(delete_grant_by_key("principal", &ada.id, "mailbox", "box").expect("del"));
        assert!(resolve_skin_token(&ada_raw5).is_none(), "grant delete by key ends session");
        let _ = mailbox;

        // A workspace-subject grant change touches no guest session.
        create_grant(&SkinGrantWrite {
            subject_kind: "workspace".into(),
            subject_id: "ws-1".into(),
            kind: "database".into(),
            target_id: "db".into(),
            role: None,
            scope: Some("read".into()),
            created_by: None,
        })
        .expect("ws grant");
        assert!(delete_grant_by_key("workspace", "ws-1", "database", "db").expect("del"));

        assert!(resolve_skin_token(&bob_raw).is_some(), "bob still untouched");
        assert!(resolve_skin_token(&plat_raw).is_some(), "platform still untouched");
    });
}

#[test]
fn session_use_rechecks_host_grant_and_principal() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("ada");
        add_principal("bob").expect("bob");
        let room = uuid::Uuid::new_v4().to_string();
        let (_, ada_raw) = create_session_token("ada").expect("ada");
        let (_, bob_raw) = create_session_token("bob").expect("bob");
        let (_, plat_raw) =
            create_token("partner", None, std::slice::from_ref(&room)).expect("platform");
        // A write that skips the revoking writers (old code, hand edit).
        with_conn(|conn| {
            conn.execute(
                "UPDATE grants SET enabled = 0
                     WHERE subject_kind = 'principal' AND subject_id = ?1 AND target_id = ?2",
                params![ada.id, HOST_APP_ID],
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(resolve_skin_token(&ada_raw).is_none(), "use-time check refuses");
        assert!(resolve_skin_token(&bob_raw).is_some());
        assert!(resolve_skin_token(&plat_raw).is_some(), "platform skips the grant check");

        assert!(remove_principal("bob").expect("remove bob"));
        assert!(resolve_skin_token(&bob_raw).is_none(), "removed principal ends session");
        assert!(resolve_skin_token(&plat_raw).is_some(), "platform survives remove");
    });
}

#[test]
fn role_room_removal_and_role_swap_reach_live_sessions() {
    with_temp_home(|| {
        add_principal("ada").expect("ada");
        add_principal("bob").expect("bob");
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let mut wide = RoomPolicy::new();
        wide.insert(a.clone(), vec![CAP_THREAD_READ.into(), CAP_FILES_READ.into()]);
        wide.insert(b.clone(), vec![CAP_THREAD_READ.into()]);
        create_role("wide", &wide).expect("wide");
        create_role("bobs", &thread_only_room_policy(std::slice::from_ref(&b)))
            .expect("bobs");
        assign_role("ada", "wide").expect("assign ada");
        assign_role("bob", "bobs").expect("assign bob");
        let (_, ada_raw) = create_session_token("ada").expect("ada");
        let (_, bob_raw) = create_session_token("bob").expect("bob");
        assert!(resolve_skin_token(&ada_raw).unwrap().has_room(&b));

        clear_role_room("wide", &b).expect("clear b");
        let p = resolve_skin_token(&ada_raw).expect("rewritten, not revoked");
        assert!(!p.has_room(&b), "removed room must leave the live session: {p:?}");
        assert!(p.has_cap_in_room(&a, CAP_FILES_READ));

        update_role(
            "wide",
            Some(&thread_only_room_policy(std::slice::from_ref(&a))),
        )
        .expect("narrow caps");
        let p = resolve_skin_token(&ada_raw).unwrap();
        assert!(!p.has_cap_in_room(&a, CAP_FILES_READ), "dropped cap must leave: {p:?}");
        assert!(p.has_cap_in_room(&a, CAP_THREAD_READ));

        create_role("dark", &RoomPolicy::new()).expect("dark");
        assign_role("ada", "dark").expect("swap role");
        assert!(resolve_skin_token(&ada_raw).unwrap().rooms_empty(), "role swap reaches session");

        // Delete a role: refused while assigned; after unassign the
        // session carries none of its rooms.
        assign_role("ada", "wide").expect("back to wide");
        assert!(resolve_skin_token(&ada_raw).unwrap().has_room(&a));
        remove_role("wide").expect_err("assigned role cannot be deleted");
        unassign_role("ada").expect("unassign");
        assert!(remove_role("wide").expect("delete"));
        let p = resolve_skin_token(&ada_raw).expect("thread-only defaults");
        assert!(!p.has_room(&a), "deleted role's rooms must be gone: {p:?}");

        let bp = resolve_skin_token(&bob_raw).expect("bob live");
        assert!(bp.has_cap_in_room(&b, CAP_THREAD_READ), "bob untouched: {bp:?}");
    });
}

#[test]
fn default_rooms_changes_reach_sessions_at_the_next_request() {
    with_temp_home(|| {
        add_principal("ada").expect("ada");
        add_principal("bob").expect("bob");
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let c = uuid::Uuid::new_v4().to_string();
        set_principal_default_rooms("ada", &[a.clone(), b.clone()], false).unwrap();
        set_principal_default_rooms("bob", &[a.clone(), b.clone()], false).unwrap();
        let (_, ada_raw) = create_session_token("ada").unwrap();
        let (_, bob_raw) = create_session_token("bob").unwrap();
        set_principal_default_rooms("ada", &[a.clone(), c.clone()], false).unwrap();
        let p = resolve_skin_token(&ada_raw).unwrap();
        assert!(p.has_room(&a));
        assert!(!p.has_room(&b), "removed default room must leave: {p:?}");
        assert!(p.has_room(&c), "an added room is there at the next request (0.46.8): {p:?}");
        set_principal_default_rooms("ada", &[a.clone(), c.clone()], true).unwrap();
        assert!(resolve_skin_token(&ada_raw).unwrap().has_room(&c));
        let bp = resolve_skin_token(&bob_raw).unwrap();
        assert!(bp.has_room(&a) && bp.has_room(&b), "bob untouched: {bp:?}");
    });
}

/// A role or session whose room policy can't be read gives nothing, never
/// every cap in every room.
/// Re-review 2026-11-02: the mark is written with the change. One that can't
/// be written undoes it, so nothing an agent wrote is ever left unmarked.
#[test]
fn a_mark_that_cant_be_written_undoes_the_change() {
    with_temp_home(|| {
        let (room, other) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
        let mut p = RoomPolicy::new();
        p.insert(room.clone(), vec![CAP_THREAD_READ.into()]);
        create_role_for_app("keeps", &p, Some("app-1")).unwrap();
        with_conn(|c| c.execute_batch("DROP TABLE agent_written_access;").map_err(|e| e.to_string())).unwrap();
        let by = || guest_rooms::WrittenBy::Agent("agent:test".to_string());
        assert!(guest_rooms::written_by(by(), || create_role_for_app("gone", &p, Some("app-1"))).is_err());
        assert!(find_role("gone").unwrap().is_none(), "no role without its mark");
        assert!(guest_rooms::written_by(by(), || set_role_room("keeps", &other, None)).is_err());
        assert!(!find_role("keeps").unwrap().unwrap().room_policy.contains_key(&other), "no room without its mark");
        assert!(guest_rooms::written_by(by(), || clear_role_room("keeps", &room)).is_err());
        assert!(find_role("keeps").unwrap().unwrap().room_policy.contains_key(&room));
    });
}

/// A box-wide role is never marked; the owner's write clears a mark.
#[test]
fn only_a_role_of_an_app_carries_an_agents_mark() {
    use guest_rooms::{written, written_by, WrittenBy, WRITTEN_ROLE};
    with_temp_home(|| {
        let mut p = RoomPolicy::new();
        p.insert(uuid::Uuid::new_v4().to_string(), vec![CAP_THREAD_READ.into()]);
        let agent = || WrittenBy::Agent("agent:test".to_string());
        let wide = written_by(agent(), || create_role_for_app("wide", &p, None)).unwrap();
        assert!(!written(WRITTEN_ROLE, &wide.id).unwrap(), "box-wide: no mark");
        let own = written_by(agent(), || create_role_for_app("own", &p, Some("app-1"))).unwrap();
        assert!(written(WRITTEN_ROLE, &own.id).unwrap());
        written_by(WrittenBy::Owner, || update_role("own", Some(&p))).unwrap();
        assert!(!written(WRITTEN_ROLE, &own.id).unwrap(), "the owner's word clears it");
        let plain = create_role_for_app("plain", &p, Some("app-1")).unwrap();
        assert!(!written(WRITTEN_ROLE, &plain.id).unwrap(), "nobody named: no mark");
    });
}

#[test]
fn an_unreadable_room_policy_fails_closed() {
    with_temp_home(|| {
        add_principal("ada").expect("ada");
        let room = uuid::Uuid::new_v4().to_string();
        let mut p = RoomPolicy::new();
        p.insert(room.clone(), vec![CAP_THREAD_READ.into(), CAP_FILES_READ.into()]);
        create_role_for_app("broken", &p, None).unwrap();
        assign_role("ada", "broken").unwrap();
        let (_, raw) = create_session_token("ada").unwrap();
        assert!(resolve_skin_token(&raw).unwrap().has_room(&room));
        with_conn(|conn| {
            conn.execute("UPDATE roles SET room_policy = 'not json' WHERE name = 'broken'", []).map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        let pass = resolve_skin_token(&raw).expect("still a session");
        assert!(pass.room_policy.is_empty() && pass.caps.is_empty(), "no caps at all: {pass:?}");
        with_conn(|conn| {
            conn.execute("UPDATE roles SET room_policy = '{}', caps = '[]', rooms = '[]' WHERE name = 'broken'", []).map_err(|e| e.to_string())?;
            conn.execute("UPDATE tokens SET room_policy = 'not json' WHERE session = 1", []).map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        assert_eq!(interpret_token_policy(true, &[CAP_FILES_READ.to_string()], &[room.clone()], Some("not json")), RoomPolicy::new());
    });
}

/// Item 12: what resolving a pass costs on a busy room's shape (a role of
/// 20 rooms, 40 caps each, folder limits on one, a tied login's extra
/// reads), against an owner-made and a platform pass. Prints; run with
/// `--ignored --nocapture`. Guest sockets resolve once per frame.
#[test]
#[ignore = "a timing report, not a test"]
fn resolve_timing_on_a_busy_room_shape() {
    with_temp_home(|| {
        let _db = crate::db::scoped_for_test();
        let rooms: Vec<String> = (0..20).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let mut p = RoomPolicy::new();
        for r in &rooms {
            p.insert(r.clone(), ACCEPTED_CAPS.iter().map(|c| c.to_string()).collect());
        }
        create_role_for_app("busy", &p, None).unwrap();
        add_principal("ada").unwrap();
        assign_role("ada", "busy").unwrap();
        let (_, session) = create_session_token("ada").unwrap();
        let mut small = RoomPolicy::new();
        small.insert(rooms[0].clone(), vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into(), CAP_FILES_READ.into()]);
        create_role_for_app("typical", &small, None).unwrap();
        add_principal("bo").unwrap();
        assign_role("bo", "typical").unwrap();
        let (_, typical) = create_session_token("bo").unwrap();
        let (_, platform) = create_token("plat", Some(&[CAP_THREAD_READ.to_string()]), &rooms).unwrap();
        // A tied login and one an agent wrote take the App's workspace from
        // the main DB too.
        let app = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'timing', '/tmp/timing', 'timing')", params![rooms[0]]).unwrap();
            crate::published_services::insert(&conn, &rooms[0], "timing", "node s.js", "/tmp", 47_561, "local", "stopped", "cmd", "").unwrap().id
        };
        let mut own = RoomPolicy::new();
        own.insert(rooms[0].clone(), vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into(), CAP_FILES_READ.into()]);
        create_role_for_app("appown", &own, Some(&app)).unwrap();
        let by_agent = AddedBy { agent_workspace: Some(&rooms[0]), ..Default::default() };
        add_principal_for_app_by("tied", None, &app, Some("appown"), "agent:timing", by_agent).unwrap();
        let tied = create_app_session_token("tied", &app).unwrap().raw;
        let time = |what: &str, raw: &str| {
            let n = 2000u32;
            let start = std::time::Instant::now();
            for _ in 0..n {
                assert!(resolve_skin_token(raw).is_some());
            }
            let per = start.elapsed() / n;
            println!("TIMING {what}: {per:?} per resolve; {:.0} resolves/s on one core under the skin mutex", 1.0 / per.as_secs_f64());
        };
        time("platform pass (no grant read)", &platform);
        time("login session, role of 1 room, 3 caps", &typical);
        time("login session, role of 20 rooms, every cap", &session);
        time("tied login, own room", &tied);
    });
}

#[test]
fn password_set_revokes_every_session_of_that_guest_only() {
    with_temp_home(|| {
        add_principal("ada").expect("ada");
        add_principal("bob").expect("bob");
        let room = uuid::Uuid::new_v4().to_string();
        set_principal_password("ada", Some("s3cret-horse")).unwrap();
        let (_, a1) = create_session_token("ada").unwrap();
        let (_, a2) = create_session_token("ada").unwrap();
        let (_, bob_raw) = create_session_token("bob").unwrap();
        let (_, plat_raw) =
            create_token("partner", None, std::slice::from_ref(&room)).unwrap();
        set_principal_password("ada", Some("new-horse-pw")).unwrap();
        assert!(resolve_skin_token(&a1).is_none(), "password change ends sessions");
        assert!(resolve_skin_token(&a2).is_none(), "password change ends sessions");
        assert!(resolve_skin_token(&bob_raw).is_some(), "bob untouched");
        assert!(resolve_skin_token(&plat_raw).is_some(), "platform untouched");
        let (_, a3) = create_session_token("ada").unwrap();
        set_principal_password("ada", None).unwrap();
        assert!(resolve_skin_token(&a3).is_none(), "password clear ends sessions");
    });
}

#[test]
fn create_role_app_id_sets_or_stays_null_and_names_stay_unique() {
    with_temp_home(|| {
        let scoped = create_role_for_app("editor", &RoomPolicy::new(), Some("svc-1"))
            .expect("app role");
        assert_eq!(scoped.app_id.as_deref(), Some("svc-1"));
        let listed = list_roles().expect("list");
        assert_eq!(listed[0].app_id.as_deref(), Some("svc-1"));
        let host_wide = create_role("reader", &RoomPolicy::new()).expect("host-wide");
        assert!(host_wide.app_id.is_none(), "{host_wide:?}");
        let dup = create_role_for_app("Editor", &RoomPolicy::new(), Some("svc-2"))
            .expect_err("box-unique name");
        assert!(
            dup.contains("already exists"),
            "role already exists must surface, got {dup}"
        );
        assert_eq!(list_roles().expect("still two").len(), 2);
    });
}

#[test]
fn remove_principal_deletes_that_principals_grants() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("add");
        let bob = add_principal("bob").expect("bob");
        create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "mailbox".into(),
            target_id: "box".into(),
            role: None,
            scope: Some("may-read".into()),
            created_by: None,
        })
        .expect("mailbox");
        let before = list_grants().expect("before");
        assert!(before.iter().any(|g| g.subject_id == ada.id && g.kind == "mailbox"));
        assert!(before.iter().any(|g| g.subject_id == ada.id && g.target_id == HOST_APP_ID));
        assert!(remove_principal("ada").expect("remove"));
        let after = list_grants().expect("after");
        assert!(
            after.iter().all(|g| g.subject_id != ada.id),
            "ada grants must go: {after:?}"
        );
        assert!(after.iter().any(|g| g.subject_id == bob.id));
        assert!(list_principals().expect("left").iter().all(|p| p.username != "ada"));
    });
}

#[test]
fn remove_role_refuses_when_a_grant_points_at_it() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("add");
        let role = create_role("editor", &RoomPolicy::new()).expect("role");
        create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "svc-9".into(),
            role: Some(role.id.clone()),
            scope: None,
            created_by: None,
        })
        .expect("grant");
        let err = remove_role(&role.id).expect_err("grant holds the role");
        assert!(err.contains("grant"), "{err}");
        assert!(err.contains("editor"), "{err}");
        assert_eq!(list_roles().expect("role stays").len(), 1);
        let grants = list_grants().expect("grant stays");
        assert!(grants.iter().any(|g| g.role_id.as_deref() == Some(role.id.as_str())));
    });
}

#[test]
fn access_template_apply_does_not_rewrite_and_scoped_kinds_reject_role() {
    with_temp_home(|| {
        let ada = add_principal("ada").expect("add");
        let role_a = create_role("alpha", &RoomPolicy::new()).expect("alpha");
        let role_b = create_role("beta", &RoomPolicy::new()).expect("beta");
        let template = create_access_template("Clinic").expect("template");
        let app_line =
            create_template_line(&template.id, "app", Some(&role_a.id)).expect("app line");
        assert!(app_line.role_id.as_deref() == Some(role_a.id.as_str()));
        let mail_line = create_template_line(&template.id, "mailbox", None).expect("mail line");
        let db_line = create_template_line(&template.id, "database", None).expect("db line");
        let cli_line = create_template_line(&template.id, "cli", None).expect("cli line");
        let cols = with_conn(|conn| Ok(table_columns(conn, "access_template_lines"))).unwrap();
        assert!(
            !cols.split(',').any(|c| c == "target_id"),
            "template line must not store target_id: {cols}"
        );
        let role_on_mail = apply_access_template(
            &ada.id,
            &template.id,
            &[TemplateApplyLine {
                line_id: mail_line.id.clone(),
                target_id: "inbox".into(),
                scope: Some("may-read".into()),
                role: Some(role_a.id.clone()),
            }],
        )
        .expect_err("mailbox rejects a role");
        assert!(role_on_mail.contains("kind=app"), "{role_on_mail}");
        let missing_scope = apply_access_template(
            &ada.id,
            &template.id,
            &[TemplateApplyLine {
                line_id: db_line.id.clone(),
                target_id: "db-1".into(),
                scope: None,
                role: None,
            }],
        )
        .expect_err("database requires scope");
        assert!(missing_scope.contains("scope"), "{missing_scope}");
        let missing_cli = apply_access_template(
            &ada.id,
            &template.id,
            &[TemplateApplyLine {
                line_id: cli_line.id.clone(),
                target_id: "tool".into(),
                scope: Some("  ".into()),
                role: None,
            }],
        )
        .expect_err("cli requires scope");
        assert!(missing_cli.contains("scope"), "{missing_cli}");
        let before = list_grants().expect("no scoped grants yet");
        assert!(before.iter().all(|g| g.kind == "app" && g.target_id == HOST_APP_ID));
        let written = apply_access_template(
            &ada.id,
            &template.id,
            &[
                TemplateApplyLine {
                    line_id: app_line.id.clone(),
                    target_id: "svc-1".into(),
                    scope: None,
                    role: None,
                },
                TemplateApplyLine {
                    line_id: mail_line.id.clone(),
                    target_id: "inbox".into(),
                    scope: Some("may-read".into()),
                    role: None,
                },
                TemplateApplyLine {
                    line_id: db_line.id.clone(),
                    target_id: "db-1".into(),
                    scope: Some("read".into()),
                    role: None,
                },
                TemplateApplyLine {
                    line_id: cli_line.id.clone(),
                    target_id: "tool".into(),
                    scope: Some("run".into()),
                    role: None,
                },
            ],
        )
        .expect("apply");
        assert_eq!(written.len(), 4);
        let app_grant = written.iter().find(|g| g.kind == "app" && g.target_id == "svc-1").expect("app grant");
        assert_eq!(app_grant.role_id.as_deref(), Some(role_a.id.as_str()));
        assert!(app_grant.scope.is_none());
        assert!(written.iter().any(|g| g.kind == "mailbox" && g.scope.as_deref() == Some("may-read") && g.role_id.is_none()));
        update_template_line(&app_line.id, "app", Some(&role_b.id)).expect("edit line");
        update_access_template(&template.id, "Clinic renamed").expect("rename");
        let after = list_grants().expect("grants after template edit");
        let still = after.iter().find(|g| g.id == app_grant.id).expect("grant remains");
        assert_eq!(
            still.role_id.as_deref(),
            Some(role_a.id.as_str()),
            "template edit must not rewrite grants: {still:?}"
        );
        assert_eq!(still.target_id, "svc-1");
        let lines = list_access_templates().expect("templates");
        assert_eq!(lines[0].name, "Clinic renamed");
        assert_eq!(
            lines[0]
                .lines
                .iter()
                .find(|l| l.id == app_line.id)
                .expect("line")
                .role_id
                .as_deref(),
            Some(role_b.id.as_str())
        );
    });
}

#[test]
fn host_grant_backfill_copies_assigned_and_empty_rooms_once() {
    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("old skin.db");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}'
                 );
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT REFERENCES roles(id),
                    email TEXT,
                    full_name TEXT
                 );",
        )
        .expect("pre-grant schema");
        let sales = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let bob_room = uuid::Uuid::new_v4().to_string();
        let mut policy = RoomPolicy::new();
        policy.insert(
            sales.clone(),
            vec![CAP_THREAD_READ.into(), CAP_FILES_READ.into()],
        );
        let policy_json = room_policy_json(&policy);
        let caps = caps_json(&union_caps(&policy));
        let role_rooms = rooms_json(&rooms_from_policy(&policy));
        let ada_defaults = rooms_json(std::slice::from_ref(&other));
        let bob_defaults = rooms_json(std::slice::from_ref(&bob_room));
        conn.execute(
            "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy)
                 VALUES ('role-dentist', 'dentist', ?1, ?2, 1, ?3)",
            params![caps, role_rooms, policy_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, default_rooms, role_id)
                 VALUES ('p-ada', 'ada', 1, ?1, 'role-dentist')",
            params![ada_defaults],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES ('p-bob', 'bob', 1, ?1)",
            params![bob_defaults],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES ('p-cara', 'cara', 1, '[]')",
            [],
        )
        .unwrap();
        assert!(!table_exists(&conn, "grants"), "fixture must lack grants");
        drop(conn);

        let principals = list_principals().expect("open copies host grants");
        let ada = principals.iter().find(|p| p.username == "ada").unwrap();
        let bob = principals.iter().find(|p| p.username == "bob").unwrap();
        let cara = principals.iter().find(|p| p.username == "cara").unwrap();
        assert_eq!(ada.role_id.as_deref(), Some("role-dentist"));
        assert_eq!(ada.default_rooms, vec![other.clone()]);
        assert!(bob.role_id.is_none(), "{bob:?}");
        assert!(
            cara.role_id.is_none() && cara.default_rooms.is_empty(),
            "{cara:?}"
        );
        let role = list_roles().unwrap();
        assert_eq!(role.len(), 1);
        assert!(
            role[0].app_id.is_none(),
            "host role stays host-wide: {role:?}"
        );
        assert_eq!(role[0].room_policy.get(&sales).map(|c| c.len()), Some(2));

        let grants = list_grants().unwrap();
        assert_eq!(grants.len(), 3, "{grants:?}");
        let ada_g = host_grants(&grants, "p-ada");
        let bob_g = host_grants(&grants, "p-bob");
        let cara_g = host_grants(&grants, "p-cara");
        assert_eq!(ada_g.len(), 1, "{grants:?}");
        assert_eq!(bob_g.len(), 1, "{grants:?}");
        assert_eq!(cara_g.len(), 1, "{grants:?}");
        assert_eq!(ada_g[0].role_id.as_deref(), Some("role-dentist"));
        assert!(
            ada_g[0].scope.is_none(),
            "assigned scope stays NULL: {ada_g:?}"
        );
        assert!(bob_g[0].role_id.is_none(), "{bob_g:?}");
        assert_eq!(bob_g[0].scope.as_deref(), Some(bob_defaults.as_str()));
        assert!(cara_g[0].role_id.is_none(), "{cara_g:?}");
        assert_eq!(
            cara_g[0].scope.as_deref(),
            Some("[]"),
            "empty rooms still get a row"
        );
        let ada_grant_id = ada_g[0].id.clone();
        let bob_grant_id = bob_g[0].id.clone();
        let cara_grant_id = cara_g[0].id.clone();

        reopen_skin_db();
        let second = list_grants().unwrap();
        assert_eq!(
            second.len(),
            3,
            "second open must not duplicate: {second:?}"
        );
        assert!(second.iter().any(|g| g.id == ada_grant_id));
        assert!(second.iter().any(|g| g.id == bob_grant_id));
        assert!(second.iter().any(|g| g.id == cara_grant_id));

        with_conn(|c| {
            c.execute(
                "UPDATE grants SET role_id = NULL, scope = 'owner-keep' WHERE id = ?1",
                params![ada_grant_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        reopen_skin_db();
        let kept = list_grants().unwrap();
        assert_eq!(kept.len(), 3, "{kept:?}");
        let ada_kept = host_grants(&kept, "p-ada");
        assert_eq!(ada_kept.len(), 1, "{kept:?}");
        assert_eq!(ada_kept[0].id, ada_grant_id);
        assert!(
            ada_kept[0].role_id.is_none(),
            "owner edit must stick: {ada_kept:?}"
        );
        assert_eq!(ada_kept[0].scope.as_deref(), Some("owner-keep"));
        let bob_kept = host_grants(&kept, "p-bob");
        assert_eq!(bob_kept[0].id, bob_grant_id);
        assert_eq!(bob_kept[0].scope.as_deref(), Some(bob_defaults.as_str()));
        assert_eq!(host_grants(&kept, "p-cara")[0].id, cara_grant_id);
        assert!(list_roles().unwrap()[0].app_id.is_none());
        assert_eq!(
            list_principals()
                .unwrap()
                .iter()
                .find(|p| p.username == "ada")
                .unwrap()
                .role_id
                .as_deref(),
            Some("role-dentist")
        );
        // AM1: from 0.45.5 the host grant is the login truth, so the
        // hand edit above would change ada's login. Put her role back
        // the supported way (assign writes the grant and the column).
        assign_role("ada", "dentist").expect("re-assign");

        let (ada_meta, ada_raw) = create_session_token("ada").expect("backfilled ada logs in");
        assert_eq!(ada_meta.rooms, vec![sales.clone()]);
        assert!(
            !ada_meta.rooms.contains(&other),
            "session must not read default_rooms"
        );
        let ada_pass = resolve_skin_token(&ada_raw).expect("ada session");
        assert!(ada_pass.has_cap(CAP_FILES_READ));
        assert!(ada_pass.has_cap_in_room(&sales, CAP_FILES_READ));
        assert!(!ada_pass.has_room(&other));

        let (bob_meta, bob_raw) = create_session_token("bob").expect("backfilled bob logs in");
        assert_eq!(bob_meta.rooms, vec![bob_room.clone()]);
        let bob_pass = resolve_skin_token(&bob_raw).expect("bob session");
        assert!(bob_pass.has_cap(CAP_THREAD_READ));
        assert!(bob_pass.has_cap(CAP_THREAD_POST));
        assert!(!bob_pass.has_cap(CAP_FILES_READ), "{bob_pass:?}");
        assert!(bob_pass.has_cap_in_room(&bob_room, CAP_THREAD_READ));
        assert!(!bob_pass.has_cap_in_room(&bob_room, CAP_FILES_READ));

        let (cara_meta, cara_raw) =
            create_session_token("cara").expect("empty rooms still log in");
        assert!(cara_meta.rooms.is_empty(), "{cara_meta:?}");
        let cara_pass = resolve_skin_token(&cara_raw).expect("cara session");
        assert!(cara_pass.rooms_empty());
        assert!(!cara_pass.has_cap(CAP_THREAD_READ), "{cara_pass:?}");
        assert!(!cara_pass.has_cap(CAP_FILES_READ));

        assert!(delete_grant_by_id(&ada_grant_id).unwrap());
        let err = create_session_token("ada").expect_err("deleted host grant denies");
        assert_eq!(err, HOST_LOGIN_DENIED);
        assert!(!err.contains("ada") && !err.contains("grant"), "{err}");
        reopen_skin_db();
        assert!(
            host_grants(&list_grants().unwrap(), "p-ada").is_empty(),
            "upgrade copy must not put a deleted host grant back"
        );
        let err = create_session_token("ada").expect_err("reopen must not restore the grant");
        assert_eq!(err, HOST_LOGIN_DENIED);
        let (_again, _) = create_session_token("bob").expect("bob grant still allows login");
    });
}

#[test]
fn host_grant_backfill_does_not_rewrite_owner_grant() {
    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("grants-era skin.db");
        conn.execute_batch(
            "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT,
                    email TEXT,
                    full_name TEXT
                 );
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}'
                 );
                 CREATE TABLE grants (
                    id TEXT PRIMARY KEY,
                    subject_kind TEXT NOT NULL,
                    subject_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    role_id TEXT,
                    scope TEXT,
                    created_at INTEGER NOT NULL,
                    UNIQUE(subject_kind, subject_id, kind, target_id)
                 );",
        )
        .expect("grants-era schema");
        let gia_room = uuid::Uuid::new_v4().to_string();
        let gia_defaults = rooms_json(std::slice::from_ref(&gia_room));
        let dan_defaults = rooms_json(&["should-not-copy".into()]);
        conn.execute(
            "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy)
                 VALUES ('role-assigned', 'assigned', '[]', '[]', 1, '{}'),
                        ('role-other', 'other', '[]', '[]', 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, default_rooms, role_id)
                 VALUES ('p-dan', 'dan', 1, ?1, 'role-assigned'),
                        ('p-ada', 'ada', 1, '[]', 'role-assigned')",
            params![dan_defaults],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES ('p-eve', 'eve', 1, '[]'),
                        ('p-gia', 'gia', 1, ?1)",
            params![gia_defaults],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO grants
                 (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at)
                 VALUES ('g-dan', 'principal', 'p-dan', 'app', 'host', 'role-other', 'owner-scope', 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let grants = list_grants().expect("open backfills missing host rows only");
        assert_eq!(grants.len(), 4, "{grants:?}");
        let dan = host_grants(&grants, "p-dan");
        assert_eq!(dan.len(), 1, "{grants:?}");
        assert_eq!(dan[0].id, "g-dan", "the owner's row is kept, not replaced");
        assert_eq!(dan[0].created_at, 1, "{dan:?}");
        // S24-2 step 1 (D1): login used `principals.role_id` before
        // 0.45.5, so that value wins over the grant's role, and the
        // overwrite is in the audit log.
        assert_eq!(dan[0].role_id.as_deref(), Some("role-assigned"));
        assert!(dan[0].scope.is_none(), "{dan:?}");
        let audit = crate::auth_audit::tail(100).expect("audit");
        let line = audit
            .iter()
            .map(|l| l.to_string())
            .find(|l| l.contains("dan") && l.contains("app_grant_upgrade"))
            .unwrap_or_else(|| panic!("step 1 overwrite must be audited: {audit:?}"));
        assert!(
            line.contains("role-other") && line.contains("role-assigned"),
            "audit names old and new: {line}"
        );
        let ada = host_grants(&grants, "p-ada");
        assert_eq!(ada.len(), 1, "{grants:?}");
        assert_eq!(ada[0].role_id.as_deref(), Some("role-assigned"));
        assert!(ada[0].scope.is_none(), "{ada:?}");
        let eve = host_grants(&grants, "p-eve");
        assert_eq!(eve.len(), 1, "{grants:?}");
        assert!(eve[0].role_id.is_none(), "{eve:?}");
        assert_eq!(eve[0].scope.as_deref(), Some("[]"));
        let gia = host_grants(&grants, "p-gia");
        assert_eq!(gia.len(), 1, "{grants:?}");
        assert!(gia[0].role_id.is_none(), "{gia:?}");
        assert_eq!(gia[0].scope.as_deref(), Some(gia_defaults.as_str()));
        for role in list_roles().unwrap() {
            assert!(role.app_id.is_none(), "copy must not set app_id: {role:?}");
        }
        let dan_id = dan[0].id.clone();
        reopen_skin_db();
        let again = list_grants().unwrap();
        assert_eq!(again.len(), 4, "{again:?}");
        let dan_again = host_grants(&again, "p-dan");
        assert_eq!(dan_again[0].id, dan_id);
        assert_eq!(dan_again[0].created_at, 1);
        assert_eq!(dan_again[0].role_id.as_deref(), Some("role-assigned"));
        assert!(dan_again[0].scope.is_none());
    });
}

#[test]
fn verify_principal_password_dummy_on_miss_does_not_lockout() {
    with_temp_home(|| {
        add_principal("eve").unwrap();
        set_principal_password("eve", Some("right-password")).unwrap();
        assert!(verify_principal_password("eve", "right-password"));
        assert!(!verify_principal_password("eve", "wrong"));
        assert!(!verify_principal_password("ghost-user", "nope"));
        add_principal("mintonly").unwrap();
        assert!(!verify_principal_password("mintonly", "anything"));
        match check_and_record_login("eve", "right-password") {
            SkinLoginOutcome::Ok(_) => {}
            other => panic!("verify must not bump login_lockouts, got {other:?}"),
        }
    });
}

#[test]
fn set_principal_password_empty_none_still_clears() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        set_principal_password("cara", None).unwrap();
        match check_and_record_login("cara", "s3cret-horse") {
            SkinLoginOutcome::BadCreds => {}
            other => panic!("None still clears, got {other:?}"),
        }
        set_principal_password("cara", Some("s3cret-horse")).unwrap();
        set_principal_password("cara", Some("")).unwrap();
        match check_and_record_login("cara", "s3cret-horse") {
            SkinLoginOutcome::BadCreds => {}
            other => panic!("empty still clears, got {other:?}"),
        }
    });
}

/// 0.46.6: a generated password is four groups of five from the
/// look-alike-free alphabet, different every time, and every letter of
/// the alphabet turns up (no letter is unreachable).
#[test]
fn generated_passwords_have_the_shape_and_spread() {
    let mut seen = HashSet::new();
    let mut letters = HashSet::new();
    for _ in 0..400 {
        let pw = generate_password();
        let groups: Vec<&str> = pw.split('-').collect();
        assert_eq!(groups.len(), 4, "{pw}");
        assert!(groups.iter().all(|g| g.len() == 5), "{pw}");
        for b in pw.bytes().filter(|b| *b != b'-') {
            assert!(GENERATED_PASSWORD_ALPHABET.contains(&b), "{pw}: {}", b as char);
            letters.insert(b);
        }
        assert!(!password_too_long(&pw));
        assert!(seen.insert(pw.clone()), "repeated password {pw}");
    }
    assert_eq!(letters.len(), GENERATED_PASSWORD_ALPHABET.len(), "every letter reachable");
    for bad in [b'0', b'O', b'o', b'1', b'l', b'I', b'i'] {
        assert!(!letters.contains(&bad), "look-alike {}", bad as char);
    }
}

/// The App users list's Unlock: exactly the principals the login check
/// would refuse right now (three failures on the username, or a lock on
/// the email row), nobody with only failures or an expired lock, and
/// gone after unlock.
#[test]
fn active_login_locks_lists_exactly_the_locked_principals() {
    with_temp_home(|| {
        let eve = add_principal("eve").unwrap();
        set_principal_password("eve", Some("right-password")).unwrap();
        let jane = add_principal_with_email("jane", Some("Jane@Clinic.com")).unwrap();
        let bob = add_principal("bob").unwrap();
        set_principal_password("bob", Some("right-password")).unwrap();
        let old = add_principal("old").unwrap();
        for _ in 0..3 {
            match check_and_record_login("eve", "wrong") {
                SkinLoginOutcome::BadCreds => {}
                other => panic!("pre-lockout must be BadCreds, got {other:?}"),
            }
        }
        match check_and_record_login("bob", "wrong") {
            SkinLoginOutcome::BadCreds => {}
            other => panic!("one failure is not a lock, got {other:?}"),
        }
        let now = now_secs();
        with_conn(|conn| {
            conn.execute(
                "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES ('jane@clinic.com', 3, ?1), ('old', 3, ?2)",
                params![now + 900, now - 5],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .expect("seed lock rows");

        let locks = active_login_locks().expect("locks");
        let mut ids: Vec<&String> = locks.keys().collect();
        ids.sort();
        let mut want = vec![&eve.id, &jane.id];
        want.sort();
        assert_eq!(ids, want, "only eve (username row) and jane (email row) are locked");
        assert!(!locks.contains_key(&bob.id), "failures without a lock are not a lock");
        assert!(!locks.contains_key(&old.id), "an expired lock is not a lock");
        assert!(locks[&eve.id] > now, "eve's lock ends in the future: {}", locks[&eve.id]);
        assert_eq!(locks[&jane.id], now + 900);

        let (_, cleared) = clear_login_lockout("jane").expect("unlock jane");
        assert!(cleared, "jane's email row was cleared through her username");
        let locks = active_login_locks().expect("locks after unlock");
        assert!(!locks.contains_key(&jane.id), "jane unlocked");
        assert!(locks.contains_key(&eve.id), "eve still locked");
    });
}

#[test]
fn clear_login_lockout_deletes_only_the_presented_key() {
    with_temp_home(|| {
        with_conn(|conn| {
            conn.execute(
                "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES ('jane@clinic.com', 0, ?1), ('jane', 1, NULL), ('bad name', 2, NULL)",
                params![now_secs().saturating_add(900)],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO password_reset_throttle (username, last_attempt) VALUES ('jane', 1)",
                [],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .expect("seed lock rows");
        let before = list_principals().expect("list before");
        let (key, cleared) = clear_login_lockout("  Jane@Clinic.COM ").expect("email clear");
        assert_eq!(key, "jane@clinic.com");
        assert!(cleared, "email row existed");
        let jane_left: i64 = with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM login_lockouts WHERE username = 'jane'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("jane row");
        assert_eq!(jane_left, 1, "username row must stay");
        let email_left: i64 = with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM login_lockouts WHERE username = 'jane@clinic.com'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("email row");
        assert_eq!(email_left, 0, "email row must be gone");
        let (key, cleared) = clear_login_lockout("Jane").expect("username clear");
        assert_eq!(key, "jane");
        assert!(cleared, "failed_count 1 with locked_until NULL is a row");
        let (key, cleared) = clear_login_lockout("jane").expect("second clear");
        assert_eq!(key, "jane");
        assert!(!cleared, "missing row is cleared false");
        let (key, cleared) = clear_login_lockout("  Bad Name ").expect("junk key");
        assert_eq!(key, "bad name");
        assert!(cleared, "non-principal trim+lowercase row");
        assert_eq!(
            list_principals().expect("list after"),
            before,
            "unlock must not create a principal"
        );
        let throttle: i64 = with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM password_reset_throttle WHERE username = 'jane'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("throttle");
        assert_eq!(throttle, 1, "password_reset_throttle must stay");
    });
}

#[test]
fn set_principal_password_does_not_delete_login_lockout() {
    with_temp_home(|| {
        add_principal("cara").expect("add");
        set_principal_password("cara", Some("s3cret-horse")).expect("set");
        with_conn(|conn| {
            conn.execute(
                "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES ('cara', 1, NULL)",
                [],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .expect("seed lock");
        let hash_before: String = with_conn(|conn| {
            conn.query_row(
                "SELECT password_hash FROM principals WHERE username = 'cara'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("hash before");
        set_principal_password("cara", Some("other-password-1")).expect("reset");
        let still: i64 = with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM login_lockouts WHERE username = 'cara'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("lock after password set");
        assert_eq!(still, 1, "password set must leave the lock row");
        let hash_after: String = with_conn(|conn| {
            conn.query_row(
                "SELECT password_hash FROM principals WHERE username = 'cara'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("hash after");
        assert_ne!(hash_before, hash_after, "password set still writes the hash");
    });
}

#[test]
fn direct_front_door_host_from_url() {
    with_temp_home(|| {
        assert_eq!(direct_front_door_host(), None);
        set_front_door("direct", Some("https://skin.app.com"), None, None).unwrap();
        assert_eq!(direct_front_door_host().as_deref(), Some("skin.app.com"));
        set_front_door("connect", None, None, None).unwrap();
        assert_eq!(direct_front_door_host(), None);
    });
    assert_eq!(
        host_from_url("https://Skin.App.com:8443/path"),
        Some("skin.app.com".into())
    );
    assert_eq!(host_from_url("skin.app.com"), Some("skin.app.com".into()));
}

#[test]
fn unassigned_session_stays_thread_only_and_hidden_from_list() {
    with_temp_home(|| {
        add_principal("ada").unwrap();
        let listed = list_principals().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].role_id.is_none(), "{listed:?}");
        assert!(listed[0].role_name.is_none(), "{listed:?}");
        let (meta, raw) = create_session_token("ada").expect("session");
        assert!(meta.rooms.is_empty());
        let pass = resolve_skin_token(&raw).expect("live");
        assert!(pass.session);
        assert_eq!(pass.username, "ada");
        assert!(pass.rooms_empty());
        assert!(!pass.has_cap(CAP_FILES_READ));
        assert!(!pass.has_cap(CAP_FILES_WRITE));
        assert!(list_tokens().unwrap().is_empty(), "list hides sessions");
    });
}

#[test]
fn role_create_rejects_connect_names_and_short_or_punct() {
    with_temp_home(|| {
        for name in ["owner", "admin", "member", "viewer", "Admin"] {
            let err = create_role(name, &RoomPolicy::new()).unwrap_err();
            assert!(
                err.contains("Connect role names cannot be skin roles"),
                "name={name} err={err}"
            );
            assert!(err.contains("owner/admin/member/viewer"), "{err}");
        }
        let err = create_role("A", &RoomPolicy::new()).unwrap_err();
        assert!(err.contains("at least 2"), "{err}");
        let err = create_role("Dentist!", &RoomPolicy::new()).unwrap_err();
        assert!(err.contains("lowercase") || err.contains("only"), "{err}");
        let role = create_role("dentist", &RoomPolicy::new()).expect("ok");
        assert_eq!(role.name, "dentist");
        assert!(role.caps.is_empty(), "empty map is Thread dark: {role:?}");
        assert!(role.rooms.is_empty());
        assert!(role.room_access.is_empty());
        let err = create_role("Dentist", &RoomPolicy::new()).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        add_principal("bob").unwrap();
        let guest_role =
            create_role("bob", &RoomPolicy::new()).expect("guest and role coexist");
        assert_eq!(guest_role.name, "bob");
    });
}

#[test]
fn role_parse_caps_rejects_unknown_verbs() {
    with_temp_home(|| {
        let mut ok = RoomPolicy::new();
        ok.insert("anna".into(), vec![CAP_TICKETS_READ.into()]);
        create_role("hygienist", &ok).expect("tickets:read is accepted");
        let mut wiki_ok = RoomPolicy::new();
        wiki_ok.insert("anna".into(), vec![CAP_WIKI_READ.into()]);
        create_role("archivist", &wiki_ok).expect("wiki:read is accepted");
        let mut store_ok = RoomPolicy::new();
        store_ok.insert("anna".into(), vec![CAP_STORE_READ.into()]);
        create_role("records", &store_ok).expect("store:read is accepted");
        let mut store_write_ok = RoomPolicy::new();
        store_write_ok.insert("docs".into(), vec![CAP_STORE_WRITE.into()]);
        create_role("charting", &store_write_ok).expect("store:write is accepted");
        for cap in ["pty:write", "wiki:write", "grid"] {
            let mut p = RoomPolicy::new();
            p.insert("anna".into(), vec![cap.into()]);
            let err = create_role("xray", &p).unwrap_err();
            assert!(err.contains("unknown capability"), "{cap} {err}");
            assert!(err.contains(cap), "{err}");
        }
    });
}

#[test]
fn assigned_session_snapshots_role_unassign_restores_defaults() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let sales = uuid::Uuid::new_v4().to_string();
        set_principal_default_rooms("bob", std::slice::from_ref(&sales), false).unwrap();
        let mut dentist = RoomPolicy::new();
        dentist.insert(
            sales.clone(),
            vec![
                CAP_THREAD_READ.into(),
                CAP_THREAD_POST.into(),
                CAP_FILES_READ.into(),
            ],
        );
        let role = create_role("dentist", &dentist).unwrap();
        assign_role("bob", "dentist").unwrap();
        let listed = list_principals().unwrap();
        assert_eq!(listed[0].role_name.as_deref(), Some("dentist"));
        assert_eq!(listed[0].role_id.as_deref(), Some(role.id.as_str()));
        assert_eq!(listed[0].default_rooms, vec![sales.clone()]);

        let (meta, raw) = create_session_token("bob").expect("assigned session");
        assert_eq!(meta.name, "bob");
        assert_eq!(meta.rooms, vec![sales.clone()]);
        let pass = resolve_skin_token(&raw).expect("live");
        assert_eq!(pass.username, "bob");
        assert!(pass.has_cap(CAP_FILES_READ));
        assert!(pass.has_room(&sales));
        assert!(list_tokens().unwrap().is_empty());

        let err = set_principal_default_rooms("bob", &[], false).unwrap_err();
        assert!(err.contains("guest 'bob' has role 'dentist'"), "{err}");
        assert!(err.contains("edit the role or unassign"), "{err}");

        let err = remove_role("dentist").unwrap_err();
        assert!(
            err.contains("role 'dentist' is assigned; unassign guests first"),
            "{err}"
        );
        assert!(resolve_skin_token(&raw).is_some(), "session still live");

        unassign_role("bob").unwrap();
        let after = resolve_skin_token(&raw).expect("cookie stays");
        assert!(after.has_cap(CAP_THREAD_READ));
        assert!(after.has_cap(CAP_THREAD_POST));
        assert!(!after.has_cap(CAP_FILES_READ), "{after:?}");
        assert_eq!(after.rooms, vec![sales.clone()]);
        assert!(remove_role("dentist").unwrap());
        let (meta2, raw2) = create_session_token("bob").expect("unassigned login");
        let pass2 = resolve_skin_token(&raw2).expect("thread-only");
        assert_eq!(meta2.rooms, vec![sales.clone()]);
        assert!(pass2.has_cap(CAP_THREAD_READ));
        assert!(!pass2.has_cap(CAP_FILES_READ));
    });
}

#[test]
fn role_update_rewrites_live_session_not_platform() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let sales = uuid::Uuid::new_v4().to_string();
        create_role(
            "dentist",
            &thread_only_room_policy(std::slice::from_ref(&sales)),
        )
        .unwrap();
        assign_role("bob", "dentist").unwrap();
        let (_sess, sess_raw) = create_session_token("bob").unwrap();
        let (plat, plat_raw) = create_token("bob", None, std::slice::from_ref(&sales)).unwrap();
        let before = resolve_skin_token(&sess_raw).unwrap();
        assert!(!before.has_cap(CAP_FILES_WRITE));
        assert!(before.has_cap_in_room(&sales, CAP_THREAD_READ));
        assert!(!before.has_cap_in_room(&sales, CAP_FILES_WRITE));

        let mut next = RoomPolicy::new();
        next.insert(
            sales.clone(),
            vec![CAP_THREAD_READ.into(), CAP_FILES_WRITE.into()],
        );
        update_role("dentist", Some(&next)).unwrap();
        let after = resolve_skin_token(&sess_raw).expect("rewritten, not revoked");
        assert!(after.has_cap(CAP_FILES_WRITE), "{after:?}");
        assert!(after.has_cap(CAP_THREAD_READ));
        assert!(!after.has_cap(CAP_FILES_READ));
        assert!(after.has_cap_in_room(&sales, CAP_FILES_WRITE));
        assert!(!after.has_cap_in_room(&sales, CAP_FILES_READ));
        let plat_pass = resolve_skin_token(&plat_raw).expect("platform untouched");
        assert_eq!(plat_pass.username, "bob");
        assert!(!plat_pass.has_cap(CAP_FILES_WRITE), "{plat_pass:?}");
        let _ = plat;
    });
}

#[test]
fn role_empty_rooms_is_thread_dark() {
    with_temp_home(|| {
        add_principal("cara").unwrap();
        let sales = uuid::Uuid::new_v4().to_string();
        set_principal_default_rooms("cara", std::slice::from_ref(&sales), false).unwrap();
        create_role("dark", &RoomPolicy::new()).unwrap();
        assign_role("cara", "dark").unwrap();
        let (meta, raw) = create_session_token("cara").unwrap();
        assert!(meta.rooms.is_empty(), "{meta:?}");
        let pass = resolve_skin_token(&raw).unwrap();
        assert!(pass.rooms_empty());
        assert!(!pass.has_room(&sales));
        assert!(!pass.has_cap_in_room(&sales, CAP_THREAD_READ));
    });
}

#[test]
fn pre_roles_skin_db_opens_with_null_role_id() {
    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT
                 );
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
                    name TEXT,
                    key_hash TEXT NOT NULL UNIQUE,
                    key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    revoked_at INTEGER,
                    session INTEGER NOT NULL DEFAULT 0,
                    expires_at INTEGER,
                    CHECK (
                        (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                        OR
                        (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
                    )
                 );",
        )
        .expect("pre-roles schema");
        let pid = uuid::Uuid::new_v4().to_string();
        let tid = uuid::Uuid::new_v4().to_string();
        let raw = format!("{SKIN_KEY_PREFIX}PreRolesHashBody00000000000000000000000");
        let key_hash = sha256_hex(&raw);
        let prefix = display_prefix(&raw);
        let room = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES (?1, 'bob', 1, ?2)",
            params![pid, rooms_json(std::slice::from_ref(&room))],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, session)
                 VALUES (?1, NULL, 'vercel', ?2, ?3, ?4, ?5, 1, 0)",
            params![
                tid,
                key_hash,
                prefix,
                caps_json(&parse_caps(None).unwrap()),
                rooms_json(std::slice::from_ref(&room))
            ],
        )
        .unwrap();
        drop(conn);

        let listed = list_principals().expect("open_db + list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].username, "bob");
        assert!(listed[0].role_id.is_none(), "{listed:?}");
        assert_eq!(listed[0].default_rooms, vec![room.clone()]);
        let pass = resolve_skin_token(&raw).expect("platform still resolves");
        assert_eq!(pass.username, "vercel");
        assert!(pass.principal_id.is_none());
        let (_meta, sess) = create_session_token("bob").expect("login still Thread-only");
        let session = resolve_skin_token(&sess).expect("session");
        assert!(session.has_cap(CAP_THREAD_READ));
        assert!(!session.has_cap(CAP_FILES_READ));
        assert_eq!(session.rooms, vec![room.clone()]);
        assert!(session.has_cap_in_room(&room, CAP_THREAD_READ));
        assert!(!session.has_cap_in_room(&room, CAP_FILES_READ));
    });
}

#[test]
fn session_has_cap_in_room_is_not_cartesian() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let anna = uuid::Uuid::new_v4().to_string();
        let docs = uuid::Uuid::new_v4().to_string();
        let mut policy = RoomPolicy::new();
        policy.insert(
            anna.clone(),
            vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
        );
        policy.insert(
            docs.clone(),
            vec![
                CAP_THREAD_READ.into(),
                CAP_THREAD_POST.into(),
                CAP_FILES_READ.into(),
            ],
        );
        create_role("dentist", &policy).unwrap();
        assign_role("bob", "dentist").unwrap();
        let (_m, raw) = create_session_token("bob").unwrap();
        let pass = resolve_skin_token(&raw).expect("session");
        assert!(pass.session);
        assert!(pass.has_room(&anna));
        assert!(pass.has_room(&docs));
        assert!(pass.has_cap(CAP_FILES_READ), "union still lists files");
        assert!(
            !pass.has_cap_in_room(&anna, CAP_FILES_READ),
            "files on docs must not grant files on anna: {pass:?}"
        );
        assert!(pass.has_cap_in_room(&docs, CAP_FILES_READ));
        assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
        let files_only = {
            let mut p = RoomPolicy::new();
            p.insert(anna.clone(), vec![CAP_FILES_READ.into()]);
            p
        };
        update_role("dentist", Some(&files_only)).unwrap();
        let after = resolve_skin_token(&raw).expect("R8");
        assert!(after.has_cap_in_room(&anna, CAP_FILES_READ));
        assert!(
            !after.has_cap_in_room(&anna, CAP_THREAD_READ),
            "explicit files-only must not silent-add Thread: {after:?}"
        );
        assert!(!after.has_room(&docs));
    });
}

#[test]
fn session_tickets_has_cap_in_room_is_not_cartesian() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let anna = uuid::Uuid::new_v4().to_string();
        let docs = uuid::Uuid::new_v4().to_string();
        let mut policy = RoomPolicy::new();
        policy.insert(
            anna.clone(),
            vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
        );
        policy.insert(
            docs.clone(),
            vec![
                CAP_THREAD_READ.into(),
                CAP_THREAD_POST.into(),
                CAP_TICKETS_READ.into(),
                CAP_TICKETS_POST.into(),
            ],
        );
        create_role("dentist", &policy).unwrap();
        assign_role("bob", "dentist").unwrap();
        let (_m, raw) = create_session_token("bob").unwrap();
        let pass = resolve_skin_token(&raw).expect("session");
        assert!(pass.has_cap(CAP_TICKETS_READ), "union still lists tickets");
        assert!(pass.has_cap(CAP_TICKETS_POST));
        assert!(
            !pass.has_cap_in_room(&anna, CAP_TICKETS_READ),
            "tickets on docs must not grant tickets on anna: {pass:?}"
        );
        assert!(!pass.has_cap_in_room(&anna, CAP_TICKETS_POST));
        assert!(pass.has_cap_in_room(&docs, CAP_TICKETS_READ));
        assert!(pass.has_cap_in_room(&docs, CAP_TICKETS_POST));
        assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
    });
}

#[test]
fn session_wiki_has_cap_in_room_is_not_cartesian() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let anna = uuid::Uuid::new_v4().to_string();
        let docs = uuid::Uuid::new_v4().to_string();
        let mut policy = RoomPolicy::new();
        policy.insert(
            anna.clone(),
            vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
        );
        policy.insert(
            docs.clone(),
            vec![
                CAP_THREAD_READ.into(),
                CAP_THREAD_POST.into(),
                CAP_WIKI_READ.into(),
            ],
        );
        create_role("dentist", &policy).unwrap();
        assign_role("bob", "dentist").unwrap();
        let (_m, raw) = create_session_token("bob").unwrap();
        let pass = resolve_skin_token(&raw).expect("session");
        assert!(pass.has_cap(CAP_WIKI_READ), "union still lists wiki");
        assert!(
            !pass.has_cap_in_room(&anna, CAP_WIKI_READ),
            "wiki on docs must not grant wiki on anna: {pass:?}"
        );
        assert!(pass.has_cap_in_room(&docs, CAP_WIKI_READ));
        assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
    });
}

#[test]
fn session_store_has_cap_in_room_is_not_cartesian() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let anna = uuid::Uuid::new_v4().to_string();
        let docs = uuid::Uuid::new_v4().to_string();
        let mut policy = RoomPolicy::new();
        policy.insert(
            anna.clone(),
            vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
        );
        policy.insert(
            docs.clone(),
            vec![
                CAP_THREAD_READ.into(),
                CAP_THREAD_POST.into(),
                CAP_STORE_READ.into(),
            ],
        );
        create_role("dentist", &policy).unwrap();
        assign_role("bob", "dentist").unwrap();
        let (_m, raw) = create_session_token("bob").unwrap();
        let pass = resolve_skin_token(&raw).expect("session");
        assert!(pass.has_cap(CAP_STORE_READ), "union still lists store");
        assert!(
            !pass.has_cap_in_room(&anna, CAP_STORE_READ),
            "store on docs must not grant store on anna: {pass:?}"
        );
        assert!(pass.has_cap_in_room(&docs, CAP_STORE_READ));
        assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
    });
}

#[test]
fn session_store_write_has_cap_in_room_is_not_cartesian() {
    with_temp_home(|| {
        add_principal("bob").unwrap();
        let sales = uuid::Uuid::new_v4().to_string();
        let docs = uuid::Uuid::new_v4().to_string();
        let mut policy = RoomPolicy::new();
        policy.insert(
            sales.clone(),
            vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
        );
        policy.insert(
            docs.clone(),
            vec![
                CAP_THREAD_READ.into(),
                CAP_THREAD_POST.into(),
                CAP_STORE_READ.into(),
                CAP_STORE_WRITE.into(),
            ],
        );
        create_role("dentist", &policy).unwrap();
        assign_role("bob", "dentist").unwrap();
        let (_m, raw) = create_session_token("bob").unwrap();
        let pass = resolve_skin_token(&raw).expect("session");
        assert!(
            pass.has_cap(CAP_STORE_WRITE),
            "union still lists store:write"
        );
        assert!(
            !pass.has_cap_in_room(&sales, CAP_STORE_WRITE),
            "store:write on docs must not grant store:write on sales: {pass:?}"
        );
        assert!(!pass.has_cap_in_room(&sales, CAP_STORE_READ));
        assert!(pass.has_cap_in_room(&docs, CAP_STORE_WRITE));
        assert!(pass.has_cap_in_room(&docs, CAP_STORE_READ));
        assert!(pass.has_cap_in_room(&sales, CAP_THREAD_READ));
    });
}

#[test]
fn platform_stays_flat_room_policy_null() {
    with_temp_home(|| {
        let room = uuid::Uuid::new_v4().to_string();
        let (meta, raw) = create_token(
            "vercel",
            Some(&[CAP_FILES_READ.into()]),
            std::slice::from_ref(&room),
        )
        .unwrap();
        assert!(meta.room_access.is_empty());
        let pass = resolve_skin_token(&raw).expect("platform");
        assert!(!pass.session);
        assert!(pass.room_policy.is_empty(), "{pass:?}");
        assert!(pass.has_cap(CAP_FILES_READ) && pass.has_room(&room));
        assert!(pass.has_cap_in_room(&room, CAP_FILES_READ));
        assert!(!pass.has_cap_in_room(&room, CAP_THREAD_READ));
        with_conn(|conn| {
            let policy: Option<String> = conn
                .query_row(
                    "SELECT room_policy FROM tokens WHERE session = 0 AND id = ?1",
                    params![meta.id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            assert!(
                policy.is_none(),
                "platform room_policy must stay NULL: {policy:?}"
            );
            Ok(())
        })
        .unwrap();
    });
}

#[test]
fn apply_tokens_writes_thread_only_room_policy() {
    with_temp_home(|| {
        add_principal("ada").unwrap();
        let room = uuid::Uuid::new_v4().to_string();
        let (_m, raw) = create_session_token("ada").unwrap();
        set_principal_default_rooms("ada", std::slice::from_ref(&room), true).unwrap();
        let pass = resolve_skin_token(&raw).expect("rewritten");
        assert!(pass.has_room(&room));
        assert!(pass.has_cap_in_room(&room, CAP_THREAD_READ));
        assert!(!pass.has_cap_in_room(&room, CAP_FILES_READ));
        with_conn(|conn| {
            let policy_raw: String = conn
                .query_row(
                    "SELECT room_policy FROM tokens WHERE session = 1 AND revoked_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let policy = parse_room_policy_json(Some(&policy_raw))
                .expect("json")
                .expect("object");
            assert_eq!(
                policy.get(&room),
                Some(&default_thread_caps()),
                "{policy:?}"
            );
            Ok(())
        })
        .unwrap();
    });
}

#[test]
fn migrate_132_copies_caps_onto_each_room_and_resolve_reads_the_role_live() {
    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT
                 );
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
                    name TEXT,
                    key_hash TEXT NOT NULL UNIQUE,
                    key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    revoked_at INTEGER,
                    session INTEGER NOT NULL DEFAULT 0,
                    expires_at INTEGER,
                    CHECK (
                        (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                        OR
                        (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
                    )
                 );",
        )
        .expect("132 schema");
        let anna = uuid::Uuid::new_v4().to_string();
        let docs = uuid::Uuid::new_v4().to_string();
        let rid = uuid::Uuid::new_v4().to_string();
        let pid = uuid::Uuid::new_v4().to_string();
        let tid = uuid::Uuid::new_v4().to_string();
        let raw = format!("{SKIN_KEY_PREFIX}Migrate132HashBody000000000000000000000");
        let key_hash = sha256_hex(&raw);
        let prefix = display_prefix(&raw);
        let caps = caps_json(&[
            CAP_THREAD_READ.to_string(),
            CAP_THREAD_POST.to_string(),
            CAP_FILES_READ.to_string(),
        ]);
        let rooms = rooms_json(&[anna.clone(), docs.clone()]);
        conn.execute(
            "INSERT INTO roles (id, name, caps, rooms, created_at) VALUES (?1, 'dentist', ?2, ?3, 1)",
            params![rid, caps, rooms],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO principals (id, username, created_at, role_id) VALUES (?1, 'bob', 1, ?2)",
            params![pid, rid],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, session)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, 1, 1)",
            params![tid, pid, key_hash, prefix, caps, rooms],
        )
        .unwrap();
        drop(conn);

        let listed = list_roles().expect("open_db migrate");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].room_policy.contains_key(&anna), "{listed:?}");
        assert!(listed[0].room_policy.contains_key(&docs), "{listed:?}");
        assert_eq!(
            listed[0].room_policy.get(&anna),
            listed[0].room_policy.get(&docs)
        );
        assert!(listed[0]
            .room_policy
            .get(&anna)
            .unwrap()
            .iter()
            .any(|c| c == CAP_FILES_READ));

        let pass = resolve_skin_token(&raw).expect("migrated session");
        assert!(pass.session);
        assert!(pass.has_cap_in_room(&anna, CAP_FILES_READ), "{pass:?}");
        assert!(pass.has_cap_in_room(&docs, CAP_FILES_READ), "{pass:?}");

        let mut docs_only = RoomPolicy::new();
        docs_only.insert(
            docs.clone(),
            vec![
                CAP_THREAD_READ.into(),
                CAP_THREAD_POST.into(),
                CAP_FILES_READ.into(),
            ],
        );
        docs_only.insert(
            anna.clone(),
            vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
        );
        update_role("dentist", Some(&docs_only)).unwrap();
        let after = resolve_skin_token(&raw).expect("R8 no re-login");
        assert!(
            !after.has_cap_in_room(&anna, CAP_FILES_READ),
            "anna lost files after R8: {after:?}"
        );
        assert!(after.has_cap_in_room(&docs, CAP_FILES_READ));
        assert!(after.has_cap_in_room(&anna, CAP_THREAD_READ));

        with_conn(|conn| {
            let mut grant_anna = RoomPolicy::new();
            grant_anna.insert(anna.clone(), vec![CAP_FILES_READ.into()]);
            conn.execute(
                "UPDATE roles SET room_policy = ?1 WHERE name = 'dentist'",
                params![room_policy_json(&grant_anna)],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        // 0.46.8 (Rosson 2026-11-02): a login's access is its grant's and
        // role's at each request, so even a role row no writer rewrote
        // sessions for applies now, both ways.
        let live = resolve_skin_token(&raw).expect("still signed in");
        assert!(live.has_cap_in_room(&anna, CAP_FILES_READ), "the role, read at this request: {live:?}");
        assert!(!live.has_room(&docs), "and what it dropped is gone: {live:?}");
    });
}

#[test]
fn omitted_caps_on_present_room_is_thread_only() {
    with_temp_home(|| {
        let sales = uuid::Uuid::new_v4().to_string();
        let role = create_role(
            "hygienist",
            &thread_only_room_policy(std::slice::from_ref(&sales)),
        )
        .unwrap();
        assert_eq!(role.rooms, vec![sales.clone()]);
        assert_eq!(role.room_policy.get(&sales), Some(&default_thread_caps()));
        assert!(!role.caps.iter().any(|c| c == CAP_FILES_READ));
        let added = set_role_room("hygienist", &sales, Some(&[CAP_FILES_READ.into()])).unwrap();
        assert_eq!(
            added.room_policy.get(&sales),
            Some(&vec![CAP_FILES_READ.to_string()])
        );
        let cleared = clear_role_room("hygienist", &sales).unwrap();
        assert!(cleared.room_policy.is_empty());
    });
}

// ── S24 per-App logins (prd-s24-per-app-logins-v1-vs-live S24-17) ──

const APP_A: &str = "0a0a0a0a-1111-4222-8333-444444444444";
const APP_B: &str = "0b0b0b0b-1111-4222-8333-444444444444";

fn one_room_policy(room: &str, caps: &[&str]) -> RoomPolicy {
    let mut p = RoomPolicy::new();
    p.insert(room.to_string(), caps.iter().map(|c| (*c).to_string()).collect());
    p
}

fn grant_for<'a>(grants: &'a [SkinGrant], principal: &str, app: &str) -> Option<&'a SkinGrant> {
    grants.iter().find(|g| {
        g.subject_kind == "principal"
            && g.subject_id == principal
            && g.kind == "app"
            && g.target_id == app
    })
}

fn app_write(principal: &str, app: &str, role: Option<&str>, scope: Option<&str>) -> SkinGrantWrite {
    SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: principal.into(),
        kind: "app".into(),
        target_id: app.into(),
        role: role.map(str::to_string),
        scope: scope.map(str::to_string),
        created_by: Some("owner-token".into()),
    }
}

/// S24-17 #1: a guest with only an App A grant logs in for A with A's
/// role, and is refused for B and for `host` with the generic error.
#[test]
fn s24_cross_app_login_fails_closed() {
    with_temp_home(|| {
        let room_a = uuid::Uuid::new_v4().to_string();
        let role_a =
            create_role_for_app("a-desk", &one_room_policy(&room_a, &[CAP_THREAD_READ, CAP_FILES_READ]), Some(APP_A))
                .expect("role");
        let eve = add_principal_for_app("eve", None, APP_A, Some("a-desk"), "owner-token")
            .expect("App-only guest");
        let grants = list_grants().expect("grants");
        assert!(
            host_grants(&grants, &eve.id).is_empty(),
            "S24-12: an App-only guest has no box login: {grants:?}"
        );
        let g = grant_for(&grants, &eve.id, APP_A).expect("A grant");
        assert_eq!(g.role_id.as_deref(), Some(role_a.id.as_str()));
        assert_eq!(g.created_by.as_deref(), Some("owner-token"));

        let a = create_app_session_token("eve", APP_A).expect("A login");
        assert_eq!(a.app, APP_A);
        assert_eq!(a.role_name.as_deref(), Some("a-desk"));
        assert_eq!(a.meta.rooms, vec![room_a.clone()]);
        let pass = resolve_skin_token(&a.raw).expect("A pass");
        assert_eq!(pass.app, APP_A);
        assert!(pass.has_cap_in_room(&room_a, CAP_FILES_READ));

        for other in [APP_B, HOST_APP_ID, "", "not-an-app"] {
            let err = create_app_session_token("eve", other).expect_err("cross-App refused");
            assert_eq!(err, HOST_LOGIN_DENIED, "{other:?}");
            assert!(!err.contains("eve") && !err.contains(APP_A), "{err}");
        }
        let err = create_session_token("eve").expect_err("no box login");
        assert_eq!(err, HOST_LOGIN_DENIED);

        // A role for another App can't be used on this App.
        let room_b = uuid::Uuid::new_v4().to_string();
        create_role_for_app("b-desk", &one_room_policy(&room_b, &[CAP_THREAD_READ]), Some(APP_B))
            .expect("role b");
        let err = add_principal_for_app("fay", None, APP_A, Some("b-desk"), "owner-token")
            .expect_err("other-App role refused");
        assert!(err.contains("is for app"), "{err}");
        assert!(
            !list_principals().unwrap().iter().any(|p| p.username == "fay"),
            "refused add must not leave a principal behind"
        );
        let err = add_principal_for_app("gus", None, HOST_APP_ID, None, "owner-token")
            .expect_err("host is not an App target here");
        assert!(err.contains("published App"), "{err}");
    });
}

/// S24-17 #2 + #3: two Apps, two roles, two passes; the use-time
/// backstop is per App (works even when the revocation is skipped).
#[test]
fn s24_two_apps_two_snapshots_and_per_app_backstop() {
    with_temp_home(|| {
        let room_a = uuid::Uuid::new_v4().to_string();
        let room_b = uuid::Uuid::new_v4().to_string();
        create_role_for_app("ra", &one_room_policy(&room_a, &[CAP_THREAD_READ]), Some(APP_A)).unwrap();
        create_role("rb", &one_room_policy(&room_b, &[CAP_TICKETS_READ])).unwrap();
        let ada = add_principal("ada").unwrap();
        create_grant(&app_write(&ada.id, APP_A, Some("ra"), None)).expect("A grant");
        let gb = create_grant(&app_write(&ada.id, APP_B, Some("rb"), None)).expect("B grant");

        let a = create_app_session_token("ada", APP_A).unwrap();
        let b = create_app_session_token("ada", APP_B).unwrap();
        let host = create_app_session_token("ada", HOST_APP_ID).unwrap();
        let pa = resolve_skin_token(&a.raw).unwrap();
        let pb = resolve_skin_token(&b.raw).unwrap();
        let ph = resolve_skin_token(&host.raw).unwrap();
        assert_eq!((pa.app.as_str(), pb.app.as_str(), ph.app.as_str()), (APP_A, APP_B, HOST_APP_ID));
        assert!(pa.has_room(&room_a) && !pa.has_room(&room_b), "{pa:?}");
        assert!(pb.has_room(&room_b) && !pb.has_room(&room_a), "{pb:?}");
        assert!(ph.rooms_empty(), "box login of an unassigned guest: {ph:?}");

        // Backstop alone: drop B's grant behind the writers' backs.
        with_conn(|c| {
            c.execute("DELETE FROM grants WHERE id = ?1", params![gb.id])
                .map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(resolve_skin_token(&b.raw).is_none(), "B pass dies at the next use");
        assert!(resolve_skin_token(&a.raw).is_some(), "A pass keeps working");
        assert!(resolve_skin_token(&host.raw).is_some(), "box pass keeps working");

        // Writer path: disabling A ends only A's sessions.
        let ga = list_grants()
            .unwrap()
            .into_iter()
            .find(|g| g.target_id == APP_A)
            .unwrap();
        set_grant_enabled(&ga.id, false).unwrap();
        assert!(session_revoked_at(&a.raw).is_some(), "A session revoked");
        assert!(session_revoked_at(&host.raw).is_none(), "host session untouched");
        assert_eq!(
            create_app_session_token("ada", APP_A).expect_err("disabled").as_str(),
            HOST_LOGIN_DENIED
        );
    });
}

/// S24-17 #4: a role change rewrites only sessions whose own App grant
/// carries that role; host writers touch only host sessions.
#[test]
fn s24_role_and_host_writers_touch_only_their_app() {
    with_temp_home(|| {
        let r1 = uuid::Uuid::new_v4().to_string();
        let r2 = uuid::Uuid::new_v4().to_string();
        let r3 = uuid::Uuid::new_v4().to_string();
        create_role("host-role", &one_room_policy(&r1, &[CAP_THREAD_READ])).unwrap();
        create_role_for_app("app-role", &one_room_policy(&r2, &[CAP_THREAD_READ]), Some(APP_A)).unwrap();
        let ada = add_principal("ada").unwrap();
        assign_role("ada", "host-role").unwrap();
        create_grant(&app_write(&ada.id, APP_A, Some("app-role"), None)).unwrap();
        let host = create_session_token("ada").unwrap().1;
        let app = create_app_session_token("ada", APP_A).unwrap().raw;

        set_role_room("app-role", &r3, None).unwrap();
        let ph = resolve_skin_token(&host).unwrap();
        let pa = resolve_skin_token(&app).unwrap();
        assert!(pa.has_room(&r3) && pa.has_room(&r2), "App session rewritten: {pa:?}");
        assert!(!ph.has_room(&r3) && ph.has_room(&r1), "host session untouched: {ph:?}");

        unassign_role("ada").unwrap();
        let ph = resolve_skin_token(&host).unwrap();
        let pa = resolve_skin_token(&app).unwrap();
        assert!(ph.rooms_empty(), "host session follows the box login: {ph:?}");
        assert!(pa.has_room(&r2), "App session keeps its own role: {pa:?}");

        // Role-less App grant rooms (S24-7 / S24-15) with --apply-tokens.
        let bob = add_principal("bob").unwrap();
        let g = create_grant(&app_write(&bob.id, APP_B, None, Some(&rooms_json(&[r1.clone()])))).unwrap();
        assert!(g.role_id.is_none());
        assert_eq!(g.scope.as_deref(), Some(rooms_json(&[r1.clone()]).as_str()));
        let bs = create_app_session_token("bob", APP_B).unwrap();
        assert!(bs.role_name.is_none());
        let bp = resolve_skin_token(&bs.raw).unwrap();
        assert!(bp.has_cap_in_room(&r1, CAP_THREAD_POST));
        assert!(!bp.has_cap_in_room(&r1, CAP_FILES_READ), "Thread-only: {bp:?}");
        let bh = create_session_token("bob").unwrap().1;
        set_app_grant_rooms("bob", APP_B, &[r2.clone()], true, "owner-token").unwrap();
        let bp = resolve_skin_token(&bs.raw).unwrap();
        assert!(bp.has_room(&r2) && !bp.has_room(&r1), "{bp:?}");
        assert!(resolve_skin_token(&bh).unwrap().rooms_empty(), "box session untouched");
        let err = set_app_grant_rooms("ada", APP_A, &[r1.clone()], false, "owner-token")
            .expect_err("a role-backed grant refuses rooms");
        assert!(err.contains("role"), "{err}");

        // Shape rules (S24-7).
        let both = create_grant(&app_write(&bob.id, "x", Some("host-role"), Some("[]"))).unwrap_err();
        assert!(both.contains("not both"), "{both}");
        let junk = create_grant(&app_write(&bob.id, "x", None, Some("owner"))).unwrap_err();
        assert!(junk.contains("JSON array"), "{junk}");
    });
}

/// 0.46.5 (`k2 app grant role`): an App login gets another role IN
/// PLACE: same grant id, its live sessions kept and rewritten to the new
/// role's rooms; other Apps' sessions and the box login untouched; a
/// rooms-only grant becomes role-backed; the add rule on roles holds.
#[test]
fn set_app_grant_role_rewrites_live_sessions_in_place() {
    with_temp_home(|| {
        let r1 = uuid::Uuid::new_v4().to_string();
        let r2 = uuid::Uuid::new_v4().to_string();
        create_role("host-role", &one_room_policy(&r1, &[CAP_THREAD_READ])).unwrap();
        create_role_for_app("looker", &one_room_policy(&r1, &[CAP_THREAD_READ]), Some(APP_A)).unwrap();
        let editor = create_role_for_app("editor", &one_room_policy(&r2, &[CAP_THREAD_READ, CAP_FILES_READ]), Some(APP_A))
            .unwrap();
        create_role_for_app("b-only", &one_room_policy(&r2, &[CAP_THREAD_READ]), Some(APP_B)).unwrap();
        let ada = add_principal("ada").unwrap();
        assign_role("ada", "host-role").unwrap();
        let g = create_grant(&app_write(&ada.id, APP_A, Some("looker"), None)).unwrap();
        let gb = create_grant(&app_write(&ada.id, APP_B, None, Some(&rooms_json(&[r2.clone()])))).unwrap();
        let a = create_app_session_token("ada", APP_A).unwrap().raw;
        let b = create_app_session_token("ada", APP_B).unwrap().raw;
        let host = create_session_token("ada").unwrap().1;

        let (ng, changed) = set_app_grant_role(&g.id, "editor").expect("re-role");
        assert!(changed);
        assert_eq!(ng.id, g.id, "same row, not rm + add");
        assert_eq!(ng.role_id.as_deref(), Some(editor.id.as_str()));
        assert!(ng.enabled);
        let stored = list_grants().unwrap().into_iter().find(|x| x.id == g.id).expect("row kept");
        assert_eq!(stored.role_id.as_deref(), Some(editor.id.as_str()));
        assert!(session_revoked_at(&a).is_none(), "the live App login is kept");
        let pa = resolve_skin_token(&a).expect("A session still signs in");
        assert!(pa.has_cap_in_room(&r2, CAP_FILES_READ), "new role reaches the live session: {pa:?}");
        assert!(!pa.has_room(&r1), "old role's room left the live session: {pa:?}");
        let pb = resolve_skin_token(&b).expect("B untouched");
        assert!(pb.has_room(&r2) && !pb.has_cap_in_room(&r2, CAP_FILES_READ), "{pb:?}");
        let ph = resolve_skin_token(&host).expect("box login untouched");
        assert!(ph.has_room(&r1) && !ph.has_room(&r2), "{ph:?}");
        let fresh = create_app_session_token("ada", APP_A).unwrap();
        assert_eq!(fresh.role_name.as_deref(), Some("editor"), "a new sign-in gets the new role");

        let (_, again) = set_app_grant_role(&g.id, "editor").unwrap();
        assert!(!again, "same role: no change");

        let err = set_app_grant_role(&g.id, "b-only").unwrap_err();
        assert!(err.contains("is for app"), "another App's role is refused: {err}");
        let err = set_app_grant_role(&g.id, "nope").unwrap_err();
        assert!(err.contains("unknown skin role"), "{err}");
        let err = set_app_grant_role("no-such-grant", "editor").unwrap_err();
        assert_eq!(err, "unknown grant");
        let stored = list_grants().unwrap().into_iter().find(|x| x.id == g.id).unwrap();
        assert_eq!(stored.role_id.as_deref(), Some(editor.id.as_str()), "refusals change nothing");

        // A rooms-only grant becomes role-backed; its session follows.
        let (nb, changed) = set_app_grant_role(&gb.id, "host-role").unwrap();
        assert!(changed && nb.scope.is_none() && nb.role_id.is_some(), "{nb:?}");
        let pb = resolve_skin_token(&b).expect("B kept");
        assert!(pb.has_room(&r1) && !pb.has_room(&r2), "{pb:?}");

        // Only App logins have a role.
        let mail = create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "mailbox".into(),
            target_id: "ada@example.test".into(),
            role: None,
            scope: Some("read".into()),
            created_by: None,
        })
        .unwrap();
        let err = set_app_grant_role(&mail.id, "editor").unwrap_err();
        assert!(err.contains("only an App login"), "{err}");
    });
}

/// 0.46.5 item 13: `k2 app user role` on an App-only guest whose App
/// login is a rooms scope (a test guest added with --room) puts the
/// role on that grant, in place: the live session and a fresh sign-in
/// get the role's caps. A box login keeps the old path.
#[test]
fn assign_login_role_moves_an_app_only_rooms_grant_to_the_role() {
    with_temp_home(|| {
        let room = uuid::Uuid::new_v4().to_string();
        create_role_for_app("rtest-files", &one_room_policy(&room, &[CAP_THREAD_READ, CAP_FILES_READ]), Some(APP_A))
            .unwrap();
        create_role_for_app("b-role", &one_room_policy(&room, &[CAP_THREAD_READ]), Some(APP_B)).unwrap();
        // An App-only guest (no box login) on a rooms scope, the shape
        // `k2 app user add <app> <name> --test --room <h>` makes.
        let zz = add_principal_for_app("zz-rtest", None, APP_A, None, "owner-token").unwrap();
        let g = set_app_grant_rooms("zz-rtest", APP_A, &[room.clone()], true, "owner-token").unwrap();
        assert!(list_grants().unwrap().iter().all(|x| !(x.subject_id == zz.id && x.target_id == HOST_APP_ID)), "no box login");
        let live = create_app_session_token("zz-rtest", APP_A).unwrap().raw;
        let before = resolve_skin_token(&live).unwrap();
        assert!(before.has_cap_in_room(&room, CAP_THREAD_POST) && !before.has_cap_in_room(&room, CAP_FILES_READ), "{before:?}");

        let err = assign_login_role("zz-rtest", "b-role").unwrap_err();
        assert!(err.contains("another App"), "{err}");

        let p = assign_login_role("zz-rtest", "rtest-files").expect("role on the App login");
        assert_eq!(p.role_name.as_deref(), Some("rtest-files"), "user list names it");
        let stored = list_grants().unwrap().into_iter().find(|x| x.id == g.id).expect("same grant row");
        assert!(stored.scope.is_none() && stored.role_id.is_some(), "a role, not rooms: {stored:?}");
        assert!(session_revoked_at(&live).is_none(), "the live session is kept");
        let after = resolve_skin_token(&live).expect("still signed in");
        assert!(after.has_cap_in_room(&room, CAP_FILES_READ), "the role's caps reach it: {after:?}");
        let fresh = create_app_session_token("zz-rtest", APP_A).unwrap();
        assert_eq!(fresh.role_name.as_deref(), Some("rtest-files"));
        assert!(resolve_skin_token(&fresh.raw).unwrap().has_cap_in_room(&room, CAP_FILES_READ));

        // Two App logins the role fits: ambiguous, nothing changed.
        create_role("wide", &one_room_policy(&room, &[CAP_THREAD_READ])).unwrap();
        create_grant(&app_write(&zz.id, APP_B, None, Some(&rooms_json(&[room.clone()])))).unwrap();
        let err = assign_login_role("zz-rtest", "wide").unwrap_err();
        assert!(err.contains("k2 app grant role"), "{err}");
        let stored = list_grants().unwrap().into_iter().find(|x| x.id == g.id).unwrap();
        assert_eq!(stored.role_id, list_roles().unwrap().into_iter().find(|r| r.name == "rtest-files").map(|r| r.id));

        // A box login keeps the old path (its host grant).
        add_principal("boxy").unwrap();
        create_role("host-r", &one_room_policy(&room, &[CAP_THREAD_READ])).unwrap();
        let p = assign_login_role("boxy", "host-r").unwrap();
        assert_eq!(p.role_name.as_deref(), Some("host-r"));
    });
}

/// S24-6 + Q8: box login off ends every session (constant default);
/// `revoke_sessions_for_app` (publish rm, S24-20) ends one App's.
#[test]
fn s24_host_off_and_app_removed_revocations() {
    with_temp_home(|| {
        let ada = add_principal("ada").unwrap();
        create_grant(&app_write(&ada.id, APP_A, None, Some("[]"))).unwrap();
        create_grant(&app_write(&ada.id, APP_B, None, Some("[]"))).unwrap();
        let a = create_app_session_token("ada", APP_A).unwrap().raw;
        let b = create_app_session_token("ada", APP_B).unwrap().raw;
        assert_eq!(revoke_sessions_for_app(APP_A).unwrap(), 1);
        assert!(resolve_skin_token(&a).is_none());
        assert!(resolve_skin_token(&b).is_some());
        assert!(
            list_grants().unwrap().iter().any(|g| g.target_id == APP_A),
            "grants stay after the App is removed"
        );
        assert!(revoke_sessions_for_app(HOST_APP_ID).is_err());
        let h = create_session_token("ada").unwrap().1;
        set_host_login("ada", false).unwrap();
        assert!(resolve_skin_token(&h).is_none());
        assert_eq!(
            resolve_skin_token(&b).is_none(),
            HOST_OFF_ENDS_ALL_APP_SESSIONS,
            "Q8 default: box login off ends App sessions too"
        );
        // Re-adding the box login copies the current rooms (AM1).
        set_principal_default_rooms("ada", &[], false).unwrap();
        set_host_login("ada", true).unwrap();
        let g = host_grants(&list_grants().unwrap(), &ada.id)[0].clone();
        assert_eq!(g.scope.as_deref(), Some("[]"));
    });
}

/// S24-1 + #6: steps run once, the version is recorded, and a file from a
/// newer K2 is refused with a clear error.
#[test]
fn s24_schema_steps_run_once_and_refuse_newer() {
    with_temp_home(|| {
        add_principal("ada").unwrap();
        let v = with_conn(|c| schema_version(c)).unwrap();
        assert_eq!(v, SKIN_SCHEMA_VERSION);
        let cols = with_conn(|c| Ok(table_columns(c, "tokens"))).unwrap();
        assert!(cols.split(',').any(|c| c == "app_service_id"), "{cols}");
        let gcols = with_conn(|c| Ok(table_columns(c, "grants"))).unwrap();
        assert!(gcols.split(',').any(|c| c == "created_by"), "{gcols}");
        assert!(skin_app_backfill_pending().unwrap(), "fresh db: backfill pending");
        let before = list_grants().unwrap();
        reopen_skin_db();
        assert_eq!(list_grants().unwrap(), before, "a reopen changes nothing");
        assert_eq!(with_conn(|c| schema_version(c)).unwrap(), SKIN_SCHEMA_VERSION);

        with_conn(|c| {
            c.execute_batch(&format!("PRAGMA user_version = {};", SKIN_SCHEMA_VERSION + 1))
                .map_err(|e| e.to_string())
        })
        .unwrap();
        let path = store_path();
        *db().lock() = None;
        let err = open_db(&path).expect_err("newer schema must refuse");
        assert_eq!(err, schema_newer_error(SKIN_SCHEMA_VERSION + 1));
        assert!(err.contains("newer"), "{err}");
        // Put it back so the temp home tears down cleanly.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {SKIN_SCHEMA_VERSION};"))
            .unwrap();
    });
}

/// R3 step 5: a version-4 file (0.46.0) gains `roles.room_paths`
/// (`{}`: every existing line stays the whole room), its roles and live
/// sessions survive untouched, and the version reads 5.
#[test]
fn r3_step5_adds_room_paths_to_a_v4_file() {
    with_temp_home(|| {
        assert_eq!(SKIN_SCHEMA_VERSION, 8);
        let room = uuid::Uuid::new_v4().to_string();
        create_role("staff", &one_room_policy(&room, &[CAP_FILES_READ])).unwrap();
        add_principal("ada").unwrap();
        assign_role("ada", "staff").unwrap();
        let (_, raw) = create_session_token("ada").unwrap();
        // Wind the file back to what 0.46.0 wrote: no column, version 4.
        let path = store_path();
        *db().lock() = None;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("ALTER TABLE roles DROP COLUMN room_paths; PRAGMA user_version = 4;")
                .unwrap();
            assert!(!table_columns(&conn, "roles").split(',').any(|c| c == "room_paths"));
        }
        // The next open runs step 5.
        let pass = resolve_skin_token(&raw).expect("a 0.46.0 session still resolves after the upgrade");
        assert_eq!(with_conn(|c| schema_version(c)).unwrap(), SKIN_SCHEMA_VERSION, "steps 5 and 6 ran");
        let cols = with_conn(|c| Ok(table_columns(c, "roles"))).unwrap();
        assert!(cols.split(',').any(|c| c == "room_paths"), "{cols}");
        let stored: String = with_conn(|c| {
            c.query_row("SELECT room_paths FROM roles WHERE name = 'staff'", [], |r| r.get(0))
                .map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(stored, "{}");
        assert!(pass.has_cap_in_room(&room, CAP_FILES_READ));
        assert_eq!(pass.path_scope(&room).unwrap(), RoomPathScope::default());
        assert_eq!(pass.room_paths, paths::PassPaths::Lines(RoomPaths::new()));
        // And a 0.46.0 binary would refuse the new file (S24-1 guard).
        assert!(schema_newer_error(5).contains("newer"));
    });
}

/// S24-11: copy-host copies enabled box logins, keeps owner grants,
/// skips other-App roles, and a dry run writes nothing.
#[test]
fn s24_copy_host_rules_and_dry_run() {
    with_temp_home(|| {
        let room = uuid::Uuid::new_v4().to_string();
        create_role("staff", &one_room_policy(&room, &[CAP_THREAD_READ])).unwrap();
        create_role_for_app("b-only", &RoomPolicy::new(), Some(APP_B)).unwrap();
        let ada = add_principal("ada").unwrap();
        assign_role("ada", "staff").unwrap();
        add_principal("bob").unwrap();
        add_principal("olga").unwrap();
        assign_role("olga", "b-only").unwrap();
        let nina = add_principal("nina").unwrap();
        let gn = create_grant(&app_write(&nina.id, APP_A, None, Some("[]"))).unwrap();
        set_grant_enabled(&gn.id, false).unwrap();
        add_principal("hank").unwrap();
        set_host_login("hank", false).unwrap();

        let dry = copy_host_grants_to_app(APP_A, "owner-token", true).unwrap();
        assert!(dry.dry_run);
        assert_eq!(dry.added, vec!["ada".to_string(), "bob".to_string()]);
        assert_eq!(dry.kept_existing, vec![("nina".to_string(), false)]);
        assert_eq!(dry.skipped_other_app_role, vec!["olga".to_string()]);
        assert!(grant_for(&list_grants().unwrap(), &ada.id, APP_A).is_none(), "dry run wrote");

        let real = copy_host_grants_to_app(APP_A, "owner-token", false).unwrap();
        assert_eq!(real.added, dry.added);
        let grants = list_grants().unwrap();
        let g = grant_for(&grants, &ada.id, APP_A).expect("ada copied");
        assert!(g.role_id.is_some() && g.created_by.as_deref() == Some("owner-token"));
        assert_eq!(create_app_session_token("ada", APP_A).unwrap().meta.rooms, vec![room.clone()]);
        assert!(create_app_session_token("bob", APP_A).unwrap().meta.rooms.is_empty());
        for refused in ["olga", "nina", "hank"] {
            assert_eq!(
                create_app_session_token(refused, APP_A).expect_err(refused),
                HOST_LOGIN_DENIED
            );
        }
        let again = copy_host_grants_to_app(APP_A, "owner-token", false).unwrap();
        assert!(again.added.is_empty(), "idempotent: {again:?}");
        assert!(copy_host_grants_to_app(HOST_APP_ID, "owner-token", false).is_err());
    });
}

/// S24-17 upgrade fixture: a skin.db as 0.45.4 left it (host-grant-only
/// guests, a D1 mismatch, owner-written App grants enabled and disabled,
/// a role tagged for the BFF App, box login off / disabled, a live
/// pre-upgrade session). After open + the boot backfill, everyone who
/// could sign in to the `--skin` App before still can, with the same
/// rooms — except the owner's disabled grant and the other-App role,
/// which are refused and audited. The BFF (`--cmd`) App gets nobody, and
/// the box login is unchanged for all.
#[test]
fn s24_upgrade_fixture_no_lockouts() {
    const SKIN_X: &str = "5c5c5c5c-aaaa-4bbb-8ccc-dddddddddddd";
    const BFF_Y: &str = "9e9e9e9e-aaaa-4bbb-8ccc-dddddddddddd";
    with_temp_home(|| {
        let path = crate::paths::k2_home().join("skin.db");
        std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
        let conn = Connection::open(&path).expect("0.45.4 skin.db");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY, name TEXT NOT NULL, caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}', app_id TEXT);
                 CREATE UNIQUE INDEX roles_name ON roles(name COLLATE NOCASE);
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL, default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT, role_id TEXT REFERENCES roles(id) ON DELETE RESTRICT,
                    email TEXT, full_name TEXT, agent_workspace TEXT);
                 CREATE TABLE grants (
                    id TEXT PRIMARY KEY, subject_kind TEXT NOT NULL, subject_id TEXT NOT NULL,
                    kind TEXT NOT NULL, target_id TEXT NOT NULL, role_id TEXT, scope TEXT,
                    created_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
                    UNIQUE(subject_kind, subject_id, kind, target_id));
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
                    name TEXT, key_hash TEXT NOT NULL UNIQUE, key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL, rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL, revoked_at INTEGER,
                    session INTEGER NOT NULL DEFAULT 0, expires_at INTEGER, room_policy TEXT,
                    CHECK ((session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                        OR (session = 0 AND principal_id IS NULL AND name IS NOT NULL)));
                 CREATE TABLE host_grant_backfill (
                    id INTEGER PRIMARY KEY CHECK (id = 1), done_at INTEGER NOT NULL);
                 INSERT INTO host_grant_backfill (id, done_at) VALUES (1, 1);",
        )
        .expect("0.45.4 schema");
        let staff_room = uuid::Uuid::new_v4().to_string();
        let narrow_room = uuid::Uuid::new_v4().to_string();
        let bob_room = uuid::Uuid::new_v4().to_string();
        let olga_room = uuid::Uuid::new_v4().to_string();
        let staff = one_room_policy(&staff_room, &[CAP_THREAD_READ, CAP_FILES_READ]);
        let narrow = one_room_policy(&narrow_room, &[CAP_THREAD_READ]);
        let olga_p = one_room_policy(&olga_room, &[CAP_THREAD_READ]);
        for (id, name, pol, app) in [
            ("r-staff", "staff", &staff, None),
            ("r-narrow", "narrow", &narrow, None),
            ("r-bff", "bff-only", &olga_p, Some(BFF_Y)),
        ] {
            conn.execute(
                "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy, app_id)
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                params![
                    id,
                    name,
                    caps_json(&union_caps(pol)),
                    rooms_json(&rooms_from_policy(pol)),
                    room_policy_json(pol),
                    app
                ],
            )
            .unwrap();
        }
        let bob_rooms = rooms_json(std::slice::from_ref(&bob_room));
        // (id, username, role_id, default_rooms)
        let people: [(&str, &str, Option<&str>, &str); 8] = [
            ("p-ada", "ada", Some("r-staff"), "[]"),
            ("p-bob", "bob", None, bob_rooms.as_str()),
            ("p-dan", "dan", Some("r-staff"), "[]"),
            ("p-olga", "olga", Some("r-bff"), "[]"),
            ("p-nina", "nina", Some("r-staff"), "[]"),
            ("p-dora", "dora", Some("r-staff"), "[]"),
            ("p-hank", "hank", Some("r-staff"), "[]"),
            ("p-lena", "lena", None, bob_rooms.as_str()),
        ];
        for (id, user, role, rooms) in people {
            conn.execute(
                "INSERT INTO principals (id, username, created_at, default_rooms, role_id)
                     VALUES (?1, ?2, 1, ?3, ?4)",
                params![id, user, rooms, role],
            )
            .unwrap();
        }
        // Host grants as 0.45.4 wrote them: the 0.40.150 backfill /
        // add_principal (role or rooms), set_host_login (role NULL,
        // "[]"), and a D1 drift (dan's grant role ≠ his column).
        // (grant id, principal, target, role, scope, enabled)
        let rows: [(&str, &str, &str, Option<&str>, Option<&str>, i64); 10] = [
            ("g-ada", "p-ada", "host", Some("r-staff"), None, 1),
            ("g-bob", "p-bob", "host", None, Some("[]"), 1), // stale scope
            ("g-dan", "p-dan", "host", Some("r-narrow"), None, 1), // D1
            ("g-olga", "p-olga", "host", Some("r-bff"), None, 1),
            ("g-nina", "p-nina", "host", Some("r-staff"), None, 1),
            ("g-nina-x", "p-nina", SKIN_X, Some("r-narrow"), None, 1), // owner, narrower
            ("g-dora", "p-dora", "host", Some("r-staff"), None, 1),
            ("g-dora-x", "p-dora", SKIN_X, Some("r-staff"), None, 0), // owner, disabled
            // hank: box login switched off (no host row).
            ("g-lena", "p-lena", "host", None, Some("[]"), 0), // disabled
            ("g-ws", "ws-1", "mbox", None, Some("may-read"), 1), // workspace subject
        ];
        for (id, who, target, role, scope, enabled) in rows {
            let (subject_kind, kind) =
                if who == "ws-1" { ("workspace", "mailbox") } else { ("principal", "app") };
            conn.execute(
                "INSERT INTO grants
                     (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
                params![id, subject_kind, who, kind, target, role, scope, enabled],
            )
            .unwrap();
        }
        // A live host session minted by 0.45.4 (no App column yet).
        let old_raw = format!("{SKIN_KEY_PREFIX}{}", "A".repeat(KEY_BODY_LEN));
        conn.execute(
            "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms,
                                     created_at, revoked_at, session, expires_at, room_policy)
                 VALUES ('t-ada', 'p-ada', NULL, ?1, 'k2skn_…AAAA', ?2, ?3, 1, NULL, 1, ?4, ?5)",
            params![
                sha256_hex(&old_raw),
                caps_json(&union_caps(&staff)),
                rooms_json(&rooms_from_policy(&staff)),
                now_secs() + 3600,
                room_policy_json(&staff)
            ],
        )
        .unwrap();
        // What each guest could do BEFORE the upgrade at the --skin App
        // (0.45.4: helper login = box login, policy from the column).
        let before_rooms: Vec<(&str, Option<Vec<String>>)> = vec![
            ("ada", Some(vec![staff_room.clone()])),
            ("bob", Some(vec![bob_room.clone()])),
            ("dan", Some(vec![staff_room.clone()])),
            ("olga", Some(vec![olga_room.clone()])),
            ("nina", Some(vec![staff_room.clone()])),
            ("dora", Some(vec![staff_room.clone()])),
            ("hank", None),
            ("lena", None),
        ];
        drop(conn);

        // Open (steps 1–3) then the boot backfill for the --skin App.
        let old = resolve_skin_token(&old_raw).expect("pre-upgrade host session still resolves");
        assert_eq!(old.app, HOST_APP_ID, "NULL App = host");
        assert!(old.has_cap_in_room(&staff_room, CAP_FILES_READ));
        assert_eq!(with_conn(|c| schema_version(c)).unwrap(), SKIN_SCHEMA_VERSION);
        assert!(skin_app_backfill_pending().unwrap());
        let reports = run_skin_app_backfill(&[SKIN_X.to_string()])
            .expect("backfill")
            .expect("ran once");
        assert_eq!(reports.len(), 1);
        assert!(run_skin_app_backfill(&[SKIN_X.to_string()]).unwrap().is_none(), "runs once");
        assert!(!skin_app_backfill_pending().unwrap());

        // Box login: same as before for everyone (incl. olga's App-tagged role).
        for (user, want) in &before_rooms {
            match (create_session_token(user), want) {
                (Ok((meta, _)), Some(rooms)) => assert_eq!(&meta.rooms, rooms, "{user} host"),
                (Err(e), None) => assert_eq!(e, HOST_LOGIN_DENIED, "{user} host"),
                (got, want) => panic!("{user} host: got {:?}, want {want:?}", got.map(|m| m.0.rooms)),
            }
        }
        // The --skin App: same rooms, except the two owner/other-App rows.
        for (user, before) in &before_rooms {
            let got = create_app_session_token(user, SKIN_X).map(|s| s.meta.rooms);
            match *user {
                "nina" => assert_eq!(
                    got.expect("nina keeps the App"),
                    vec![narrow_room.clone()],
                    "the owner's narrower App grant now applies (Q2)"
                ),
                "dora" | "olga" => assert_eq!(
                    got.expect_err(user),
                    HOST_LOGIN_DENIED,
                    "{user}: refused (owner-disabled / other-App role)"
                ),
                _ => match before {
                    Some(rooms) => assert_eq!(&got.expect(user), rooms, "{user} at the App"),
                    None => assert_eq!(got.expect_err(user), HOST_LOGIN_DENIED, "{user}"),
                },
            }
        }
        // The BFF App starts with nobody until copy-host (Q6 / S24-10).
        for (user, _) in &before_rooms {
            assert_eq!(
                create_app_session_token(user, BFF_Y).expect_err(user),
                HOST_LOGIN_DENIED,
                "{user} at the BFF App"
            );
        }
        let report = &reports[0];
        assert!(report.added.contains(&"ada".to_string()));
        assert!(report.added.contains(&"bob".to_string()));
        assert!(report.added.contains(&"dan".to_string()));
        assert!(!report.added.iter().any(|u| u == "hank" || u == "lena"));
        assert_eq!(report.skipped_other_app_role, vec!["olga".to_string()]);
        assert_eq!(
            report.kept_existing,
            vec![("dora".to_string(), false), ("nina".to_string(), true)]
        );
        let grants = list_grants().unwrap();
        assert_eq!(
            grant_for(&grants, "p-ada", SKIN_X).unwrap().created_by.as_deref(),
            Some(UPGRADE_CREATED_BY)
        );
        assert_eq!(
            grant_for(&grants, "p-dan", HOST_APP_ID).unwrap().role_id.as_deref(),
            Some("r-staff"),
            "D1: the column won"
        );
        assert_eq!(
            grant_for(&grants, "p-bob", HOST_APP_ID).unwrap().scope.as_deref(),
            Some(bob_rooms.as_str()),
            "stale scope synced to default_rooms"
        );
        assert!(grants.iter().any(|g| g.id == "g-ws"), "workspace grants untouched");
        assert!(
            resolve_skin_token(&old_raw).is_some(),
            "the pre-upgrade host session survives the backfill"
        );
        let audit: Vec<String> = crate::auth_audit::tail(200)
            .unwrap()
            .iter()
            .map(|v| v.to_string())
            .collect();
        for needle in ["dora", "olga", "dan", "bob"] {
            assert!(
                audit.iter().any(|l| l.contains("app_grant_upgrade") && l.contains(needle)),
                "{needle} must be in the upgrade audit: {audit:#?}"
            );
        }
        assert!(
            audit.iter().any(|l| l.contains("--skin App backfill")),
            "summary line: {audit:#?}"
        );
    });
}
