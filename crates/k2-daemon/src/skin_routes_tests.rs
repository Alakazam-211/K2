use super::*;

/// Security review (0.46.4): through an App's link the handler itself
/// refuses a login whose target (read its own way: any-case `app`, the
/// first match; empty or none = the box) isn't that App, before any
/// password work. The link's rewrite is not the only check.
#[test]
fn a_link_login_naming_another_target_is_refused_by_the_handler() {
    const FORM: &str = "application/x-www-form-urlencoded";
    for (body, ct) in [
        (&b"username=a&password=x&APP=host&app=svc-a"[..], FORM),
        (b"username=a&password=x&APP=&app=svc-a", FORM),
        (b"username=a&password=x&App=svc-b&app=svc-a", FORM),
        (b"username=a&password=x", FORM),
        (b"not json", "application/json"),
        (br#"{"username":"a","password":"x","app":"host"}"#, "application/json"),
        (br#"{"username":"a","password":"x","appId":"svc-b"}"#, "application/json"),
        (br#"{"username":"a","password":"x"}"#, "application/json"),
    ] {
        let r = handle_login_on(body, ct, Some("svc-a"), None);
        assert!(r.response.status.starts_with("403"), "{}: {} {}", String::from_utf8_lossy(body), r.response.status, r.response.body);
        let v: serde_json::Value = serde_json::from_str(&r.response.body).expect("json");
        assert_eq!(v["error"]["code"], "app_mismatch", "{v}");
        assert!(r.token.is_none());
    }
    // The throttle's username is the handler's.
    assert_eq!(login_username(b"Username=victim&username=decoy", FORM), "victim");
    assert_eq!(login_username(br#"{"name":"victim"}"#, "application/json"), "victim");
    assert_eq!(login_username(br#"{"username":"u","name":"n"}"#, "application/json"), "u");
}

/// agent-app-users S2: an agent actor that reaches the add without the
/// scoped gate (no workspace) never names an App and never gets a box
/// login; through the gate, an add without an App is refused with the
/// teaching text. Refused before any store is touched.
#[test]
fn agent_user_add_needs_the_gate_and_an_app() {
    assert!(actor_is_agent("agent:sales"));
    assert!(!actor_is_agent("owner-token"));
    assert!(!actor_is_agent("user:rosson"));
    for body in [&br#"{"username":"eve","app":"0f5e7c1a-3b2d-4e6f-8a9b-1c2d3e4f5a6b"}"#[..], br#"{"username":"eve"}"#] {
        let r = handle_users_post(body, "agent:sales");
        assert!(r.status.starts_with("403"), "{} {}", r.status, r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only", "{v}");
    }
    let r = handle_users_post_as(Some("ws-sales"), br#"{"username":"eve"}"#, "agent:sales");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["error"]["code"], "owner_only", "{v}");
    assert_eq!(v["error"]["hint"], AGENT_ADD_NEEDS_APP_HINT, "{v}");
    let r = handle_users_post(br#"{"username":"eve","role":"x"}"#, "owner-token");
    assert!(r.status.starts_with("400"), "role needs app: {}", r.body);
}

/// 0.46.6 (App users list Unlock): the users list says whose login is
/// locked right now (`lockedUntil`, epoch seconds) and nobody else's;
/// the owner's unlock clears it.
#[test]
fn users_list_says_who_is_locked_and_unlock_clears_it() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    skin::add_principal("lena").expect("lena");
    skin::set_principal_password("lena", Some("right-password")).expect("lena pw");
    skin::add_principal("otto").expect("otto");
    for _ in 0..3 {
        assert!(matches!(skin::check_and_record_login("lena", "wrong"), skin::SkinLoginOutcome::BadCreds));
    }
    let list = |who: &str| -> serde_json::Value {
        let r = handle_users_get();
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        v["users"].as_array().expect("users").iter().find(|u| u["username"] == who).cloned().unwrap_or_else(|| panic!("no {who}: {v}"))
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_secs() as i64;
    let lena = list("lena");
    assert!(lena["lockedUntil"].as_i64().expect("lena is locked") > now, "{lena}");
    assert!(list("otto")["lockedUntil"].is_null(), "otto is not locked");

    let r = handle_users_unlock(br#"{"username":"lena"}"#, "owner-token");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["cleared"], true, "{v}");
    assert!(list("lena")["lockedUntil"].is_null(), "unlock clears the lock");
}

/// 0.46.6: the users list says who has a box login (`boxLogin`), by
/// the same rule [`owner_or_admin_for`] keeps the owner's, so
/// Settings can tell an Admin before they try.
#[test]
fn users_list_says_who_has_a_box_login() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    skin::add_principal("bea").expect("bea");
    skin::add_principal("cal").expect("cal");
    // add_principal gives a box login (an unassigned host grant); cal
    // loses hers, so she only uses Apps.
    assert!(skin::set_host_login("cal", false).expect("host login off"), "cal had a box login to remove");
    let r = handle_users_get();
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    let users = v["users"].as_array().expect("users");
    let row = |who: &str| users.iter().find(|u| u["username"] == who).cloned().unwrap_or_else(|| panic!("no {who}: {v}"));
    assert_eq!(row("bea")["boxLogin"], serde_json::json!(true), "{v}");
    assert_eq!(row("cal")["boxLogin"], serde_json::json!(false), "{v}");
    let grants = skin::list_grants().expect("grants");
    for who in ["bea", "cal"] {
        let id = row(who)["id"].as_str().expect("id").to_string();
        assert_eq!(row(who)["boxLogin"], serde_json::json!(has_box_login(&id, &grants)), "the list and the reset rule agree on {who}");
    }
}

/// Settings → User Access: one part of the users list that can't load
/// (here the login locks table is gone) doesn't take the list down. The
/// users come back with every other part's fields, the failed part's
/// field is left off, and `errors.locks` says why.
#[test]
fn users_list_part_that_fails_leaves_the_rest_with_its_error() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    skin::add_principal("bea").expect("bea");
    skin::add_principal("cal").expect("cal");
    let r = handle_users_get();
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert!(v.get("errors").is_none(), "nothing failed yet: {v}");

    let store = rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db")).expect("skin store");
    store.execute_batch("DROP TABLE login_lockouts").expect("drop the locks table");
    drop(store);

    let r = handle_users_get();
    assert_eq!(r.status, "200 OK", "one failed part is not a failed list: {}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    let errors = v["errors"].as_object().unwrap_or_else(|| panic!("errors names the failed part: {v}"));
    assert_eq!(errors.keys().map(String::as_str).collect::<Vec<_>>(), vec!["locks"], "{v}");
    let why = errors["locks"].as_str().unwrap_or_else(|| panic!("a reason: {v}"));
    assert!(why.contains("login_lockouts"), "{why}");
    let users = v["users"].as_array().unwrap_or_else(|| panic!("users: {v}"));
    let names: Vec<&str> = users.iter().map(|u| u["username"].as_str().expect("username")).collect();
    assert_eq!(names, vec!["bea", "cal"], "{v}");
    for u in users {
        assert!(u.get("lockedUntil").is_none(), "the failed part's field is left off: {u}");
        assert_eq!(u["boxLogin"], serde_json::json!(true), "{u}");
        assert!(u["passwordMustChange"].is_boolean(), "{u}");
        assert!(u["appGrants"].is_array(), "{u}");
        assert!(u["nameSetBySelf"].is_boolean(), "{u}");
        assert!(u.get("pendingReset").is_some() && u.get("storedEmailRefused").is_some(), "{u}");
    }
}

/// The users list reads the daemon DB's App ids and room handles in
/// one go, not once per App grant or test guest: with 1 and with 6 App
/// users (each a test guest, all on one room), the list reads `apps`
/// and `projects` the same number of times.
#[test]
fn users_list_reads_app_names_once_however_many_grants_and_guests() {
    use rusqlite::ffi;
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::sync::{Arc, Mutex};
    // rusqlite sets an authorizer only through `&mut Connection`, and the
    // scoped test DB is shared (`ReentrantMutex`, `&` only), so this one
    // goes through SQLite's C API: the table name of every column read.
    unsafe extern "C" fn record_read(
        seen: *mut c_void,
        code: c_int,
        table: *const c_char,
        _column: *const c_char,
        _db: *const c_char,
        _trigger: *const c_char,
    ) -> c_int {
        if code == ffi::SQLITE_READ && !table.is_null() {
            // SAFETY: `seen` is the Arc'd Mutex below, alive until the
            // authorizer is cleared; SQLite passes a NUL-terminated name.
            let seen = unsafe { &*(seen as *const Mutex<Vec<String>>) };
            let name = unsafe { CStr::from_ptr(table) }.to_string_lossy().into_owned();
            seen.lock().expect("seen").push(name);
        }
        ffi::SQLITE_OK
    }
    let _home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (app, room) = ("svc-roster-batch", "room-roster-batch");
    let add = |name: &str| {
        let p = skin::add_principal_for_app(name, None, app, None, "test").expect("guest");
        skin::set_app_grant_rooms(name, app, &[room.to_string()], false, "test").expect("rooms");
        skin::test_guests::record(&p.id, app, "ws-roster-batch", "agent:x", 4_102_444_800).expect("test guest");
    };
    // Reads of (`apps`, `projects`) by one users list, from the daemon
    // DB's authorizer (asked once per column per prepared statement).
    let reads = || -> (usize, usize) {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        {
            let handle = db.handle();
            let conn = handle.lock();
            conn.flush_prepared_statement_cache();
            // SAFETY: the handle is the open scoped connection; `seen`
            // outlives the authorizer (cleared below before it drops).
            let rc = unsafe {
                ffi::sqlite3_set_authorizer(
                    conn.handle(),
                    Some(record_read),
                    Arc::as_ptr(&seen) as *mut c_void,
                )
            };
            assert_eq!(rc, ffi::SQLITE_OK, "set authorizer");
        }
        let r = handle_users_get();
        {
            let handle = db.handle();
            let conn = handle.lock();
            // SAFETY: as above; clearing it takes no pointer.
            let rc = unsafe { ffi::sqlite3_set_authorizer(conn.handle(), None, std::ptr::null_mut()) };
            assert_eq!(rc, ffi::SQLITE_OK, "clear authorizer");
        }
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert!(v.get("errors").is_none(), "{v}");
        let tests = v["users"].as_array().expect("users").iter().filter(|u| u["test"]["appServiceId"] == app).count();
        assert!(tests >= 1, "the test guests are listed: {v}");
        let seen = seen.lock().expect("seen");
        (seen.iter().filter(|t| *t == "apps").count(), seen.iter().filter(|t| *t == "projects").count())
    };
    add("tg0");
    let one = reads();
    assert!(one.0 > 0 && one.1 > 0, "the list looks up App ids and room handles: {one:?}");
    for i in 1..6 {
        add(&format!("tg{i}"));
    }
    let six = reads();
    assert_eq!(six, one, "(apps, projects) reads with 6 App users vs 1: the lookups must not run per grant or per guest");
}

/// 0.46.6: `generate: true` on a password set and on a user add — the
/// daemon makes the password, returns it once, and it signs in. Never
/// with a typed
/// password; a non-boolean is refused before anyone is added.
#[test]
fn generate_sets_a_daemon_made_password_and_returns_it_once() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let ok = |r: CliResponse| -> serde_json::Value {
        assert_eq!(r.status, "200 OK", "{}", r.body);
        serde_json::from_str(&r.body).expect("json")
    };
    // Add with generate.
    let v = ok(handle_users_post(br#"{"username":"gina","generate":true}"#, "owner-token"));
    let first = v["generatedPassword"].as_str().expect("generatedPassword").to_string();
    assert_eq!(first.len(), skin::GENERATED_PASSWORD_CHARS + 3, "{first}");
    assert_eq!(v["hasPassword"], true, "{v}");
    // 0.46.6: K2 chose it, so the person chooses their own at sign-in.
    assert_eq!(v["passwordMustChange"], true, "{v}");
    assert!(matches!(skin::check_and_record_login("gina", &first), skin::SkinLoginOutcome::MustChange(_)));
    // A plain add and a typed set never carry one.
    let v = ok(handle_users_post(br#"{"username":"hal","password":"typed-one-here"}"#, "owner-token"));
    assert!(v.get("generatedPassword").is_none(), "{v}");
    assert_eq!(v["passwordMustChange"], true, "the owner typed it for someone else: {v}");
    let v = ok(handle_users_password(br#"{"username":"hal","password":"typed-two-here"}"#, "owner-token"));
    assert!(v.get("generatedPassword").is_none(), "{v}");
    assert_eq!(v["passwordMustChange"], true, "{v}");
    // Set with generate: a new one, and the old one stops working (the
    // set is the same `set_principal_password_tied` that ends sessions).
    let v = ok(handle_users_password(br#"{"username":"gina","generate":true}"#, "owner-token"));
    let second = v["generatedPassword"].as_str().expect("generatedPassword").to_string();
    assert_ne!(first, second);
    assert!(matches!(skin::check_and_record_login("gina", &second), skin::SkinLoginOutcome::MustChange(_)));
    assert!(matches!(skin::check_and_record_login("gina", &first), skin::SkinLoginOutcome::BadCreds));
    // One or the other, and only a boolean.
    for (body, why) in [
        (&br#"{"username":"gina","generate":true,"password":"typed-one-here"}"#[..], "both"),
        (br#"{"username":"gina","generate":"yes"}"#, "not a boolean"),
    ] {
        let r = handle_users_password(body, "owner-token");
        assert!(r.status.starts_with("400"), "{why}: {} {}", r.status, r.body);
    }
    let r = handle_users_post(br#"{"username":"ivan","generate":1}"#, "owner-token");
    assert!(r.status.starts_with("400"), "{}", r.body);
    assert!(
        skin::list_principals().expect("list").iter().all(|p| p.username != "ivan"),
        "a refused generate adds nobody"
    );
}

/// GH#72: an agent can't mint the `.k2/` write opt-in on a platform
/// pass. Refused before any store is touched.
#[test]
fn agent_cannot_mint_the_agent_config_opt_in() {
    let r = handle_tokens_post(
        br#"{"name":"agent-tok","caps":["files:write","files:agentconfig"],"rooms":["sales"]}"#,
        "agent:sales",
    );
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["error"]["code"], "owner_only", "{v}");
    assert!(
        v["error"]["hint"].as_str().expect("hint").contains("files:agentconfig"),
        "{v}"
    );
}

#[test]
fn full_name_sanitizer_caps_at_64_and_strips_controls() {
    let dirty = format!("A\u{1b}\n{}", "x".repeat(80));
    let clean = cleaned_full_name(Some(&dirty)).expect("usable name remains");
    assert!(!clean.contains('\u{1b}'), "ESC must not survive: {clean:?}");
    assert!(!clean.contains('\n'), "newline must not survive: {clean:?}");
    assert!(
        !clean.chars().any(|c| c.is_control()),
        "controls must not survive: {clean:?}"
    );
    assert_eq!(clean.chars().count(), 64, "{clean:?}");
    assert!(clean.starts_with("Ax"), "{clean:?}");
    assert_eq!(
        cleaned_full_name(Some("Ada/Lovelace: MD")).as_deref(),
        Some("Ada/Lovelace: MD"),
        "slash and colon stay (not validate_display_name)"
    );
    assert_eq!(cleaned_full_name(Some("  Ada  ")).as_deref(), Some("Ada"));
    assert!(cleaned_full_name(Some("\u{1b}\n\t")).is_none());
    assert!(cleaned_full_name(Some("   ")).is_none());
    assert!(cleaned_full_name(None).is_none());
}

#[test]
fn safe_next_rejects_open_redirects() {
    assert_eq!(safe_next(Some("https://evil.example")), "/");
    assert_eq!(safe_next(Some("//evil")), "/");
    assert_eq!(safe_next(Some("/app")), "/app");
    // Browsers resolve these to another host.
    assert_eq!(safe_next(Some("/\\evil.example")), "/");
    assert_eq!(safe_next(Some("/\t/evil.example")), "/");
    assert_eq!(safe_next(Some("/a\nb")), "/");
    assert_eq!(safe_next(Some("/app?x=1#y")), "/app?x=1#y");
}

#[test]
fn login_failed_bodies_are_generic() {
    let a = login_failed_json();
    let b = login_failed_json();
    assert_eq!(a.body, b.body);
    assert_eq!(a.status, "401 Unauthorized");
    assert!(!a.body.contains("guest"));
    assert_eq!(a.body, LOGIN_FAILED_JSON);
}

#[test]
fn reset_and_change_failures_are_generic() {
    let a = reset_failed();
    let b = reset_failed();
    assert_eq!(a.status, "401 Unauthorized");
    assert_eq!(a.body, b.body);
    assert_eq!(a.body, RESET_FAILED_JSON);
    assert!(!a.body.contains("expired") || a.body.contains("invalid or expired"));
    let c = change_failed();
    assert_eq!(c.status, "401 Unauthorized");
    assert_eq!(c.body, CHANGE_FAILED_JSON);
    assert!(!c.body.contains("username"));
    assert!(!c.body.contains("k2skn_"));
}

#[test]
fn owner_only_response_has_stable_code_and_403() {
    let r = owner_only_response();
    assert_eq!(r.status, "403 Forbidden");
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "owner_only");
    let hint = v["error"]["hint"].as_str().unwrap_or("");
    assert!(
        hint.contains("owner") || hint.contains("human"),
        "hint should teach owner/human: {hint}"
    );
    assert!(
        !hint.contains("To let this workspace's agent manage Apps")
            && !hint.contains("Allow this agent to manage Skin Access"),
        "leftover/hydra keep today's hint without the Agent-tab sentence: {hint}"
    );
    assert!(
        !r.body.contains("Invalid or missing auth token"),
        "must not look like a broken passport: {}",
        r.body
    );
}

#[test]
fn owner_only_manage_response_adds_agent_tab_sentence() {
    let r = owner_only_manage_response();
    assert_eq!(r.status, "403 Forbidden");
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["error"]["code"], "owner_only");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(
        hint.contains("Build and run Apps"),
        "manage OFF must teach the Agent-tab toggle: {hint}"
    );
    assert!(
        !r.body.contains("gated"),
        "must stay owner_only, not gated: {}",
        r.body
    );
}

#[test]
fn skin_manage_actor_label_is_agent_handle_never_owner_token() {
    k2_core::db::init_for_tests();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("sales{}", &id[..8]);
    let path = format!("/tmp/k2-skin-actor-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, path],
    )
    .expect("seed");
    drop(conn);
    let principal = crate::session_token::HookPrincipal {
        workspace_uuid: id,
        agent_address: "sidecar".to_string(),
    };
    let actor = skin_manage_actor_label(&principal);
    assert_eq!(actor, format!("agent:{handle}"));
    assert_ne!(actor, "owner");
    assert_ne!(actor, "owner-token");
}

#[test]
fn stamp_actor_skips_non_200_and_sets_field_on_ok() {
    let denied = owner_only_manage_response();
    let stamped = stamp_actor(denied, "agent:sales");
    assert!(!stamped.body.contains("agent:sales"), "{}", stamped.body);
    let ok = CliResponse::ok_json(r#"{"username":"bob"}"#.to_string());
    let stamped = stamp_actor(ok, "agent:sales");
    let v: serde_json::Value = serde_json::from_str(&stamped.body).expect("json");
    assert_eq!(v["actor"], "agent:sales");
    assert_eq!(v["username"], "bob");
}

#[test]
fn hydra_get_unsupported_on_non_linux() {
    let _g = crate::skin_hydra::hydra_test_lock();
    k2_core::db::init_for_tests();
    {
        let db = k2_core::db::shared();
        let _ = db.lock().execute("DELETE FROM skin_hydra", []);
    }
    crate::skin_hydra::stop();
    if cfg!(target_os = "linux") {
        return;
    }
    let r = handle_hydra_get();
    assert_eq!(r.status, "200 OK");
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["supported"], false, "{}", r.body);
    assert_eq!(v["enabled"], false, "{}", r.body);
    assert_eq!(v["running"], false, "{}", r.body);
    assert!(
        v["hint"].as_str().unwrap_or("").contains("LINUX"),
        "{}",
        r.body
    );
}

#[test]
fn hydra_post_off_is_not_running() {
    let _g = crate::skin_hydra::hydra_test_lock();
    k2_core::db::init_for_tests();
    {
        let db = k2_core::db::shared();
        let _ = db.lock().execute("DELETE FROM skin_hydra", []);
    }
    let r = handle_hydra_post(br#"{"enabled":false,"apply":true}"#, "owner");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["enabled"], false, "{}", r.body);
    assert_eq!(v["running"], false, "{}", r.body);
}

#[test]
fn hydra_post_requires_enabled() {
    let r = handle_hydra_post(br#"{}"#, "owner");
    assert_eq!(r.status, "400 Bad Request");
    assert!(r.body.contains("enabled"), "{}", r.body);
}
