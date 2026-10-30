use super::*;
use crate::db::{run_migrations, run_migrations_through};

fn fresh() -> Connection {
    let conn = Connection::open(":memory:").unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    run_migrations(&conn).unwrap();
    conn
}

/// A database as 0.46.6 left it, with `services` as `(name, expose, cmd)` and
/// a workspace, then upgraded.
fn upgraded(services: &[(&str, &str, &str)]) -> Connection {
    let conn = Connection::open(":memory:").unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    run_migrations_through(&conn, Some("0248_email_notify_queue")).unwrap();
    assert!(
        conn.query_row("SELECT 1 FROM sqlite_master WHERE name = 'site_visibility'", [], |_| Ok(())).is_err(),
        "0.46.6 has no visibility table"
    );
    conn.execute("INSERT INTO projects (id, name, path) VALUES ('p1', 'one', '/tmp/p1')", []).unwrap();
    for (i, (name, expose, cmd)) in services.iter().enumerate() {
        conn.execute(
            "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES (?1, 'p1', ?2, ?3, '/tmp/p1', ?4, ?5, 'running')",
            params![format!("svc-{i}"), name, cmd, 4000 + i as i64, expose],
        )
        .unwrap();
    }
    run_migrations(&conn).unwrap();
    conn
}

fn pin(p: &str) -> PinHash {
    hash_pin(p).unwrap()
}

#[test]
fn a_label_with_no_row_is_private_and_not_explicit() {
    let conn = fresh();
    let v = effective(&conn, "New-Site").unwrap();
    assert_eq!(v.label, "new-site");
    assert_eq!(v.mode, Mode::Private);
    assert!(!v.explicit);
    assert!(!v.pin_set);
    assert_eq!(effective_mode(&conn, "new-site").unwrap(), Mode::Private);
    assert!(get(&conn, "new-site").unwrap().is_none());
}

#[test]
fn a_new_database_owes_no_grandfather_pass() {
    let conn = fresh();
    assert!(!grandfather_pending(&conn).unwrap());
    let n = grandfather_map(&conn, "box", ["staging"], true).unwrap();
    assert_eq!(n, 0);
    assert_eq!(effective_mode(&conn, "staging").unwrap(), Mode::Private, "a fresh install's labels start private");
}

#[test]
fn upgrade_keeps_every_tunnelled_service_public_with_the_reason() {
    let conn = upgraded(&[("docs", "tunnel", "npm start"), ("Shop", "tunnel", "(skin)"), ("inner", "local", "x"), ("meet", "tunnel", "x")]);
    let docs = get(&conn, "docs").unwrap().unwrap();
    assert_eq!(docs.mode, Mode::Public);
    assert_eq!(docs.public_reason.as_deref(), Some(REASON_BEFORE));
    assert_eq!(docs.set_by, SET_BY_UPGRADE);
    assert_eq!(effective_mode(&conn, "shop").unwrap(), Mode::Public, "the label is lowercased");
    assert!(get(&conn, "inner").unwrap().is_none(), "a --no-tunnel service has no address to keep public");
    assert!(get(&conn, "meet").unwrap().is_none(), "a reserved label is not this table's");
    assert_eq!(grandfathered_public(&conn).unwrap().len(), 2);
}

#[test]
fn the_migrations_reserved_list_matches_the_code() {
    let sql = include_str!("../drizzle_sql/0253_site_visibility.sql");
    let quoted: Vec<String> = crate::skin::RESERVED_NESTED_LABELS.iter().map(|r| format!("'{}'", r.label)).collect();
    assert!(sql.contains(&format!("NOT IN ({})", quoted.join(", "))), "0253's reserved list drifted from RESERVED_NESTED_LABELS: {quoted:?}");
}

#[test]
fn upgrade_grandfathers_labels_only_the_control_plane_knows() {
    let conn = upgraded(&[("docs", "tunnel", "x")]);
    settle_grandfather_at_boot(&conn, "Box").unwrap();
    assert!(grandfather_pending(&conn).unwrap());

    // Boot: the persisted map, before it routes anything. Owed still.
    assert_eq!(grandfather_map(&conn, "box", ["staging", "skin", "docs"], false).unwrap(), 1);
    assert_eq!(effective_mode(&conn, "staging").unwrap(), Mode::Public);
    assert!(get(&conn, "skin").unwrap().is_none(), "reserved labels never get a row");
    assert!(grandfather_pending(&conn).unwrap(), "the copy on disk doesn't settle it");

    // The first fetched map: one more old label, and the pass is settled.
    let n = grandfather_map(&conn, "box", ["staging", "cp-only"], true).unwrap();
    assert_eq!(n, 1);
    assert_eq!(get(&conn, "cp-only").unwrap().unwrap().public_reason.as_deref(), Some(REASON_BEFORE));
    assert!(!grandfather_pending(&conn).unwrap());

    // Afterwards nothing is grandfathered: a new label is private.
    assert_eq!(grandfather_map(&conn, "box", ["brand-new"], true).unwrap(), 0);
    assert_eq!(effective_mode(&conn, "brand-new").unwrap(), Mode::Private);
}

#[test]
fn a_label_made_while_the_pass_is_owed_stays_private() {
    let conn = upgraded(&[]);
    settle_grandfather_at_boot(&conn, "box").unwrap();
    note_new_label(&conn, "made-after", "agent:x").unwrap();
    assert_eq!(grandfather_map(&conn, "box", ["made-after", "older"], true).unwrap(), 1);
    assert_eq!(effective_mode(&conn, "made-after").unwrap(), Mode::Private);
    assert_eq!(effective_mode(&conn, "older").unwrap(), Mode::Public);
}

#[test]
fn the_pass_is_owed_for_one_subdomain_and_a_box_with_no_tunnel_owes_nothing() {
    let conn = upgraded(&[]);
    settle_grandfather_at_boot(&conn, "box").unwrap();
    assert_eq!(grandfather_map(&conn, "other", ["theirs"], true).unwrap(), 0);
    assert!(grandfather_pending(&conn).unwrap(), "another subdomain's map settles nothing");
    assert_eq!(effective_mode(&conn, "theirs").unwrap(), Mode::Private);

    let conn = upgraded(&[]);
    settle_grandfather_at_boot(&conn, "  ").unwrap();
    assert!(!grandfather_pending(&conn).unwrap());
    assert_eq!(grandfather_map(&conn, "paired-later", ["old"], true).unwrap(), 0);
    assert_eq!(effective_mode(&conn, "old").unwrap(), Mode::Private);
}

#[test]
fn a_public_row_survives_note_new_label_and_forget_starts_over() {
    let conn = upgraded(&[("docs", "tunnel", "x")]);
    note_new_label(&conn, "docs", "run").unwrap();
    assert_eq!(effective_mode(&conn, "docs").unwrap(), Mode::Public, "a hostname that comes back stays public");
    forget_label(&conn, "docs").unwrap();
    assert!(get(&conn, "docs").unwrap().is_none());
    note_new_label(&conn, "docs", "run").unwrap();
    assert_eq!(effective_mode(&conn, "docs").unwrap(), Mode::Private, "a new label of the same name is new");
}

#[test]
fn public_needs_a_reason_and_records_it() {
    let conn = fresh();
    let e = set_public(&conn, "site", "   ", "owner").unwrap_err();
    assert!(e.starts_with("reason_required:"), "{e}");
    assert_eq!(effective_mode(&conn, "site").unwrap(), Mode::Private, "the refusal changed nothing");
    let v = set_public(&conn, "site", "launch", "user:rosson").unwrap();
    assert_eq!(v.mode, Mode::Public);
    assert_eq!(v.public_reason.as_deref(), Some("launch"));
    assert_eq!(v.set_by, "user:rosson");
    let v = set_private(&conn, "site", "owner").unwrap();
    assert_eq!(v.mode, Mode::Private);
    assert_eq!(v.public_reason, None);
}

#[test]
fn pin_rules_refuse_without_quoting_it() {
    for bad in ["short", "", &"x".repeat(65)] {
        let e = hash_pin(bad).unwrap_err();
        assert!(e.starts_with("pin_length:"), "{e}");
    }
    let e = hash_pin("abc\ndef").unwrap_err();
    assert!(e.starts_with("pin_characters:") && !e.contains("abc"), "{e}");
    assert!(pin("123456").short, "digits only is allowed, with the warning");
    assert!(!pin("12345678").short);
}

#[test]
fn a_pin_is_stored_hashed_and_verifies() {
    let conn = fresh();
    let v = set_pin(&conn, "site", &pin("hunter22"), None, "owner").unwrap();
    assert_eq!(v.mode, Mode::Pin);
    assert!(v.pin_set);
    let stored: String = conn.query_row("SELECT pin_hash FROM site_visibility WHERE label = 'site'", [], |r| r.get(0)).unwrap();
    assert!(stored.starts_with("$argon2id$"), "argon2id, like Connect users");
    assert!(!stored.contains("hunter22"));
    assert!(v.verify_pin("hunter22"));
    assert!(!v.verify_pin("hunter23"));
    assert!(!format!("{v:?}").contains("argon2"), "Debug never prints the hash");
    assert!(!serde_json::to_string(&v).unwrap().contains("argon2"), "the wire never carries the hash");
    assert!(!effective(&conn, "no-row").unwrap().verify_pin("hunter22"), "no PIN set: nothing verifies");
}

#[test]
fn pin_days_are_bounded_and_kept() {
    let conn = fresh();
    for bad in [0, 31] {
        assert!(set_pin(&conn, "site", &pin("hunter22"), Some(bad), "o").unwrap_err().starts_with("pin_days:"));
    }
    let v = set_pin(&conn, "site", &pin("hunter22"), Some(3), "o").unwrap();
    assert_eq!(v.pin_days, 3);
    let v = set_pin(&conn, "site", &pin("hunter23"), None, "o").unwrap();
    assert_eq!(v.pin_days, 3, "no --pin-days keeps the stored life");
    let s = create_session(&conn, "site", SessionKind::Pin, USER_PIN).unwrap();
    assert_eq!(s.max_age, 3 * 86_400);
}

#[test]
fn a_pin_change_ends_pin_sessions_and_keeps_private_ones() {
    let conn = fresh();
    set_pin(&conn, "site", &pin("hunter22"), None, "o").unwrap();
    let friend = create_session(&conn, "site", SessionKind::Pin, USER_PIN).unwrap();
    let owner = create_session(&conn, "site", SessionKind::Open, USER_OWNER_TOKEN).unwrap();
    assert_eq!(live_session(&conn, "site", &friend.token).unwrap().unwrap().kind, SessionKind::Pin);

    set_pin(&conn, "site", &pin("hunter23"), None, "o").unwrap();
    assert!(live_session(&conn, "site", &friend.token).unwrap().is_none());
    assert_eq!(live_session(&conn, "site", &owner.token).unwrap().unwrap().kind, SessionKind::Open);
}

#[test]
fn an_older_generations_pin_session_is_dead_even_if_its_row_remained() {
    let conn = fresh();
    set_pin(&conn, "site", &pin("hunter22"), None, "o").unwrap();
    let friend = create_session(&conn, "site", SessionKind::Pin, USER_PIN).unwrap();
    conn.execute("UPDATE site_visibility SET pin_generation = pin_generation + 1 WHERE label = 'site'", []).unwrap();
    assert!(live_session(&conn, "site", &friend.token).unwrap().is_none());
}

#[test]
fn moving_off_pin_ends_the_friend_and_the_pin() {
    let conn = fresh();
    set_pin(&conn, "site", &pin("hunter22"), None, "o").unwrap();
    let friend = create_session(&conn, "site", SessionKind::Pin, USER_PIN).unwrap();
    let v = set_private(&conn, "site", "o").unwrap();
    assert!(!v.pin_set);
    assert!(!v.verify_pin("hunter22"));
    assert!(live_session(&conn, "site", &friend.token).unwrap().is_none());
}

#[test]
fn going_public_ends_every_session() {
    let conn = fresh();
    let owner = create_session(&conn, "site", SessionKind::Open, USER_OWNER_TOKEN).unwrap();
    set_public(&conn, "site", "launch", "o").unwrap();
    assert!(live_session(&conn, "site", &owner.token).unwrap().is_none());
}

#[test]
fn sessions_are_bound_to_their_label_and_stored_hashed() {
    let conn = fresh();
    let s = create_session(&conn, "one", SessionKind::Open, "rosson").unwrap();
    assert_eq!(s.max_age, OPEN_SESSION_SECS);
    assert!(live_session(&conn, "two", &s.token).unwrap().is_none(), "another label's cookie never works here");
    assert!(live_session(&conn, "one", "not-the-token").unwrap().is_none());
    let stored: String = conn.query_row("SELECT id_hash FROM site_sessions", [], |r| r.get(0)).unwrap();
    assert_ne!(stored, s.token);
    assert!(!format!("{s:?}").contains(&s.token), "Debug never prints the cookie");
}

#[test]
fn an_expired_session_is_dead_and_swept() {
    let conn = fresh();
    let s = create_session(&conn, "site", SessionKind::Open, "rosson").unwrap();
    conn.execute("UPDATE site_sessions SET expires_at = unixepoch() - 1", []).unwrap();
    assert!(live_session(&conn, "site", &s.token).unwrap().is_none());
    assert!(list_sessions(&conn, "site").unwrap().is_empty());
    assert_eq!(sweep_expired(&conn).unwrap(), 1);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM site_sessions", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn list_and_revoke_all() {
    let conn = fresh();
    create_session(&conn, "site", SessionKind::Open, "rosson").unwrap();
    set_pin(&conn, "site", &pin("hunter22"), None, "o").unwrap();
    create_session(&conn, "site", SessionKind::Pin, USER_PIN).unwrap();
    create_session(&conn, "other", SessionKind::Open, "rosson").unwrap();
    let l = list_sessions(&conn, "site").unwrap();
    assert_eq!(l.len(), 2);
    assert!(l.iter().any(|s| s.kind == SessionKind::Open && s.user == "rosson"));
    assert!(l.iter().any(|s| s.kind == SessionKind::Pin));
    assert_eq!(revoke_sessions(&conn, "site").unwrap(), 2);
    assert!(list_sessions(&conn, "site").unwrap().is_empty());
    assert_eq!(list_sessions(&conn, "other").unwrap().len(), 1, "another label's sessions stay");
}

#[test]
fn a_code_is_single_use_label_bound_and_expires() {
    let conn = fresh();
    let c = mint_open_code(&conn, "site", "/docs?x=1", "rosson").unwrap();
    assert!(c.expires_at > 0);
    assert!(!format!("{c:?}").contains(&c.code), "Debug never prints the code");

    assert_eq!(redeem_open_code(&conn, "other", &c.code).unwrap(), None, "bound to its label");
    let r = redeem_open_code(&conn, "site", &c.code).unwrap().unwrap();
    assert_eq!(r, Redeemed { user: "rosson".into(), path: "/docs?x=1".into() });
    assert_eq!(redeem_open_code(&conn, "site", &c.code).unwrap(), None, "a reused code fails");

    let c = mint_open_code(&conn, "site", "/", USER_OWNER_TOKEN).unwrap();
    conn.execute("UPDATE site_open_codes SET expires_at = unixepoch() - 1", []).unwrap();
    assert_eq!(redeem_open_code(&conn, "site", &c.code).unwrap(), None, "an expired code fails");
    assert_eq!(sweep_expired(&conn).unwrap(), 1);
}

#[test]
fn a_code_for_a_reserved_label_or_a_foreign_path_is_refused() {
    let conn = fresh();
    assert!(mint_open_code(&conn, "skin", "/", "o").unwrap_err().starts_with("reserved_label:"));
    for bad in ["//evil.example", "https://evil.example", "/\\evil.example", "docs", "/a b", "/a\r\nSet-Cookie: x", "/_k2/site/open"] {
        assert!(mint_open_code(&conn, "site", bad, "o").unwrap_err().starts_with("bad_path:"), "{bad:?}");
    }
    assert_eq!(safe_path("").as_deref(), Some("/"), "no path = the site's root");
    assert_eq!(safe_path("/a/b?c=d#e").as_deref(), Some("/a/b?c=d#e"));
}

#[test]
fn reserved_labels_refuse_every_mode_change() {
    let conn = fresh();
    for r in crate::skin::RESERVED_NESTED_LABELS {
        assert!(!is_covered(r.label));
        assert!(set_private(&conn, r.label, "o").unwrap_err().starts_with("reserved_label:"));
        assert!(set_public(&conn, r.label, "why", "o").unwrap_err().starts_with("reserved_label:"));
    }
    assert!(is_covered("staging"));
}

#[test]
fn dev_keeps_the_mode_and_bounds_the_idle_stop() {
    let conn = fresh();
    let v = set_dev(&conn, "web", true, None, "o").unwrap();
    assert!(v.dev);
    assert_eq!(v.mode, Mode::Private, "a dev site starts private");
    assert_eq!(v.idle_secs, None);
    for bad in [1, 299, 86_401] {
        assert!(set_dev(&conn, "web", true, Some(bad), "o").unwrap_err().starts_with("idle_range:"), "{bad}");
    }
    assert_eq!(set_dev(&conn, "web", true, Some(0), "o").unwrap().idle_secs, Some(0));
    set_public(&conn, "web", "demo", "o").unwrap();
    let v = set_dev(&conn, "web", false, None, "o").unwrap();
    assert!(!v.dev);
    assert_eq!(v.mode, Mode::Public, "dev off doesn't change who may see it");
}

#[test]
fn a_bad_label_is_refused() {
    let conn = fresh();
    assert!(set_private(&conn, "a.b", "o").unwrap_err().starts_with("bad_label:"));
    assert!(mint_open_code(&conn, "-x", "/", "o").unwrap_err().starts_with("bad_label:"));
}

#[test]
fn a_change_wakes_subscribers() {
    let conn = fresh();
    let mut rx = subscribe();
    rx.borrow_and_update();
    set_private(&conn, "site", "o").unwrap();
    assert!(rx.has_changed().unwrap(), "the gate re-checks open connections on a change");
}

#[test]
fn the_address_limit_counts_first_and_locks_for_a_while() {
    let conn = fresh();
    let t = 1_000_000;
    for i in 0..PIN_ADDR_TRIES {
        assert_eq!(pin_attempt_begin(&conn, "site", Some("203.0.113.7"), t + i).unwrap(), PinAttempt::Go { site_tripped: false });
    }
    let w = pin_attempt_begin(&conn, "site", Some("203.0.113.7"), t + 10).unwrap();
    assert_eq!(w, PinAttempt::Wait { retry_after: PIN_ADDR_LOCK_SECS - 6, site: false });
    assert!(matches!(pin_attempt_begin(&conn, "site", Some("198.51.100.3"), t + 10).unwrap(), PinAttempt::Go { .. }), "another address goes on");
    assert!(matches!(pin_attempt_begin(&conn, "other", Some("203.0.113.7"), t + 10).unwrap(), PinAttempt::Go { .. }), "another site goes on");
    assert!(matches!(pin_attempt_begin(&conn, "site", Some("203.0.113.7"), t + 4 + PIN_ADDR_LOCK_SECS).unwrap(), PinAttempt::Go { .. }), "the wait ends");
}

#[test]
fn a_right_pin_gives_its_try_back() {
    let conn = fresh();
    let t = 1_000_000;
    for _ in 0..4 {
        pin_attempt_begin(&conn, "site", Some("203.0.113.7"), t).unwrap();
    }
    pin_attempt_right(&conn, "site", Some("203.0.113.7")).unwrap();
    for _ in 0..PIN_ADDR_TRIES {
        assert!(matches!(pin_attempt_begin(&conn, "site", Some("203.0.113.7"), t).unwrap(), PinAttempt::Go { .. }), "the address started over");
    }
    let fails: i64 = conn.query_row("SELECT fails FROM site_pin_limits WHERE scope = 'site:site'", [], |r| r.get(0)).unwrap();
    assert_eq!(fails, 3 + PIN_ADDR_TRIES);
}

#[test]
fn the_site_limit_applies_with_no_address_and_trips_once() {
    let conn = fresh();
    let t = 1_000_000;
    for i in 0..PIN_SITE_TRIES - 1 {
        assert_eq!(pin_attempt_begin(&conn, "site", None, t + i).unwrap(), PinAttempt::Go { site_tripped: false }, "{i}");
    }
    assert_eq!(pin_attempt_begin(&conn, "site", None, t + 100).unwrap(), PinAttempt::Go { site_tripped: true }, "the 30th");
    let w = pin_attempt_begin(&conn, "site", None, t + 101).unwrap();
    assert_eq!(w, PinAttempt::Wait { retry_after: PIN_SITE_LOCK_SECS - 1, site: true });
    assert!(matches!(pin_attempt_begin(&conn, "site", Some("203.0.113.7"), t + 101).unwrap(), PinAttempt::Wait { site: true, .. }), "every address waits");

    // A person who knew the PIN on the 30th try doesn't leave a lock behind.
    let conn = fresh();
    for _ in 0..PIN_SITE_TRIES {
        pin_attempt_begin(&conn, "site", None, t).unwrap();
    }
    pin_attempt_right(&conn, "site", None).unwrap();
    assert!(matches!(pin_attempt_begin(&conn, "site", None, t + 1).unwrap(), PinAttempt::Go { .. }));
}

#[test]
fn limits_survive_a_new_connection_and_a_new_pin_starts_them_over() {
    let path = std::env::temp_dir().join(format!("k2-site-limits-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    {
        let conn = Connection::open(&path).unwrap();
        run_migrations(&conn).unwrap();
        for _ in 0..PIN_ADDR_TRIES {
            pin_attempt_begin(&conn, "site", Some("203.0.113.7"), now_unix()).unwrap();
        }
    }
    let conn = Connection::open(&path).unwrap();
    assert!(matches!(pin_attempt_begin(&conn, "site", Some("203.0.113.7"), now_unix()).unwrap(), PinAttempt::Wait { .. }), "a restart doesn't reset a lock");
    set_pin(&conn, "site", &pin("hunter22"), None, "o").unwrap();
    assert!(matches!(pin_attempt_begin(&conn, "site", Some("203.0.113.7"), now_unix()).unwrap(), PinAttempt::Go { .. }));
    drop(conn);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn sweep_keeps_a_live_lock_and_drops_an_old_window() {
    let conn = fresh();
    let now = now_unix();
    conn.execute("INSERT INTO site_pin_limits (scope, fails, window_start, locked_until) VALUES ('site:a', 30, ?1, ?2)", params![now - 2 * PIN_SITE_WINDOW_SECS, now + 60]).unwrap();
    conn.execute("INSERT INTO site_pin_limits (scope, fails, window_start, locked_until) VALUES ('site:b', 3, ?1, 0)", params![now - 2 * PIN_SITE_WINDOW_SECS]).unwrap();
    conn.execute("INSERT INTO site_pin_limits (scope, fails, window_start, locked_until) VALUES ('site:c', 3, ?1, 0)", params![now - 10]).unwrap();
    assert_eq!(sweep_expired(&conn).unwrap(), 1);
    let left: Vec<String> = conn.prepare("SELECT scope FROM site_pin_limits ORDER BY scope").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
    assert_eq!(left, vec!["site:a".to_string(), "site:c".to_string()]);
}

#[test]
fn door_refusal_names_e2e_off() {
    let _home = crate::test_env::TempHome::new();
    let _e2e = crate::test_env::EnvVar::set("K2_E2E", "0");
    let why = door_refusal().expect("E2E off: the gate can't see the traffic");
    assert!(why.starts_with("visibility_door_unsupported:") && why.contains("E2E"), "{why}");
    let _on = crate::test_env::EnvVar::set("K2_E2E", "1");
    assert_eq!(door_refusal(), None);
}

#[test]
fn a_map_hook_failure_mode_a_grandfather_pass_on_a_tx_is_atomic() {
    let conn = upgraded(&[]);
    settle_grandfather_at_boot(&conn, "box").unwrap();
    conn.execute("DROP TABLE site_visibility", []).unwrap();
    let e = grandfather_map(&conn, "box", ["old"], true).unwrap_err();
    assert!(e.starts_with("site_visibility_db:"), "{e}");
    assert!(grandfather_pending(&conn).unwrap(), "a pass that failed is still owed");
}
