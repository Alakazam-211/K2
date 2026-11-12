use super::*;
use crate::tunnel::test_support::with_temp_home;

#[test]
fn hash_then_verify_round_trips() {
    let h = hash_password("correct horse battery staple").expect("hash");
    assert!(verify_hash("correct horse battery staple", &h));
}

#[test]
fn wrong_password_fails_verify() {
    let h = hash_password("hunter2").expect("hash");
    assert!(!verify_hash("hunter3", &h));
}

#[test]
fn normalize_username_lowercases_and_validates() {
    assert_eq!(normalize_username("Rosson").unwrap(), "rosson");
    assert_eq!(normalize_username("  a_b-1 ").unwrap(), "a_b-1");
    assert!(normalize_username("a").is_err(), "too short");
    assert!(normalize_username("has space").is_err());
    assert!(normalize_username("Bang!").is_err());
}

#[test]
fn add_user_then_verify_succeeds() {
    with_temp_home(|| {
        add_user("Alice", "s3cretpass").expect("add");
        // Stored lowercased; verify is case-insensitive on the name.
        assert!(verify("alice", "s3cretpass"));
        assert!(verify("ALICE", "s3cretpass"));
        assert!(!verify("alice", "wrongpass"));
    });
}

#[test]
fn verify_unknown_user_is_false_and_still_does_work() {
    with_temp_home(|| {
        // No users provisioned at all.
        assert!(!verify("ghost", "whatever"));
        // Add one, then check a different unknown user still false.
        add_user("real", "password").expect("add");
        assert!(!verify("nobody", "password"));
    });
}

#[test]
fn add_rejects_duplicate() {
    with_temp_home(|| {
        add_user("bob", "password1").expect("add");
        let err = add_user("BOB", "password2").expect_err("dup must reject");
        assert!(err.contains("already exists"), "got: {err}");
    });
}

#[test]
fn add_rejects_invalid_username() {
    with_temp_home(|| {
        assert!(add_user("x", "password").is_err(), "too short");
        assert!(add_user("bad name", "password").is_err(), "space");
        assert!(add_user("ok", "").is_err(), "empty password");
    });
}

#[test]
fn list_users_redacts_hash() {
    with_temp_home(|| {
        add_user("carol", "password").expect("add");
        let views = list_users().expect("list");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].username, "carol");
        assert!(!views[0].disabled);
        // ConnectUserView simply has no hash field — compile-time
        // guarantee. Serialize and confirm no hash leaks.
        let json = serde_json::to_string(&views[0]).unwrap();
        assert!(!json.contains("password"), "view leaked a hash: {json}");
    });
}

#[test]
fn set_password_changes_credential_and_revokes_sessions() {
    with_temp_home(|| {
        add_user("dave", "oldpassword").expect("add");
        let tok = create_session("dave");
        assert_eq!(validate_session(&tok), Some("dave".to_string()));
        set_password("dave", "newpassword").expect("set-password");
        assert!(!verify("dave", "oldpassword"));
        assert!(verify("dave", "newpassword"));
        assert_eq!(validate_session(&tok), None, "rotation must revoke session");
    });
}

/// No login can be called `owner` or `agent` (the daemon's names for the
/// owner token and for a request with no person). One made before 0.46.6
/// fails closed: no sign-in, no live session, and a doctor line.
#[test]
fn reserved_names_are_refused_and_old_ones_fail_closed() {
    with_temp_home(|| {
        for name in ["owner", "Owner", " OWNER ", "agent"] {
            let e = add_user(name, "password").expect_err(name);
            assert!(e.contains("server's owner and its agents"), "{name}: {e}");
        }
        assert!(list_users().expect("list").is_empty(), "nothing was created");
        assert!(is_reserved_username("Owner") && !is_reserved_username("owner2") && !is_reserved_username("rosson"));
        assert_eq!(reserved_name_doctor_line(&reserved_name_users().expect("users")), None);
        // One from before the rule.
        test_add_legacy_reserved_user("owner", "password").expect("legacy");
        add_user("gina", "password").expect("gina");
        assert!(!verify("owner", "password"), "a reserved name never signs in");
        assert!(verify("gina", "password"));
        let tok = create_session("owner");
        assert_eq!(validate_session(&tok), None, "its session is nobody");
        assert_eq!(role_for_session(&tok), None);
        assert_eq!(reserved_login_session(&tok).as_deref(), Some("owner"), "told apart from no session");
        let gina = create_session("gina");
        assert_eq!(validate_session(&gina).as_deref(), Some("gina"));
        assert_eq!(reserved_login_session(&gina), None);
        assert_eq!(reserved_login_session("not-a-token"), None);
        let line = reserved_name_doctor_line(&reserved_name_users().expect("users")).expect("doctor line");
        assert!(line.contains("owner") && line.contains("Admin Access"), "{line}");
    });
}

#[test]
fn set_disabled_blocks_login_and_revokes_sessions() {
    with_temp_home(|| {
        add_user("eve", "password").expect("add");
        let tok = create_session("eve");
        set_disabled("eve", true).expect("disable");
        assert!(!verify("eve", "password"), "disabled user must not verify");
        assert_eq!(validate_session(&tok), None, "disable must revoke session");
        set_disabled("eve", false).expect("re-enable");
        assert!(verify("eve", "password"), "re-enabled user verifies again");
    });
}

#[test]
fn set_role_revokes_sessions() {
    with_temp_home(|| {
        add_user("mallory", "password").expect("add");
        let tok = create_session("mallory");
        assert_eq!(
            validate_session(&tok),
            Some("mallory".to_string()),
            "fresh session is valid"
        );
        set_role("mallory", Role::Admin).expect("promote");
        assert_eq!(role_for_user("mallory"), Some(Role::Admin));
        assert_eq!(
            validate_session(&tok),
            None,
            "role change must revoke the old session so the new role takes effect on re-login"
        );
    });
}

#[test]
fn session_create_validate_expire() {
    with_temp_home(|| {
        // The epoch cross-check requires the user to EXIST, so provision
        // it before issuing a session.
        add_user("frank", "password").expect("add");
        let tok = create_session("frank");
        assert_eq!(validate_session(&tok), Some("frank".to_string()));

        // Inject an already-expired record directly into the persisted
        // store and confirm validate drops it.
        let expired_tok = new_token();
        update_sessions(|store| {
            store.sessions.push(SessionRecord {
                version: SESSION_RECORD_VERSION,
                username: "frank".to_string(),
                token_digest: token_digest(&expired_tok),
                created_at: Utc::now() - Duration::days(31),
                expires_at: Utc::now() - Duration::seconds(1),
                token_epoch: 0,
                created_ip: None,
                user_agent: None,
                // Legacy shape — records that predate session ids
                // deserialize to "" via #[serde(default)].
                session_id: String::new(),
                device: None,
            });
            Ok(())
        })
        .expect("inject expired session");
        assert_eq!(validate_session(&expired_tok), None, "expired -> None");
        // Lazily dropped from disk on validate.
        let store = load_sessions().expect("load sessions");
        assert!(
            !store
                .sessions
                .iter()
                .any(|s| s.token_digest == token_digest(&expired_tok)),
            "expired session must be evicted on validate"
        );
    });
}

#[test]
fn remove_user_revokes_sessions() {
    with_temp_home(|| {
        add_user("grace", "password").expect("add");
        let tok = create_session("grace");
        assert_eq!(validate_session(&tok), Some("grace".to_string()));
        remove_user("grace").expect("remove");
        assert_eq!(validate_session(&tok), None, "remove must revoke session");
        assert!(!verify("grace", "password"), "removed user must not verify");
        let err = remove_user("grace").expect_err("removing twice errors");
        assert!(err.contains("not found"), "got: {err}");
    });
}

#[test]
fn unknown_token_validates_to_none() {
    with_temp_home(|| {
        assert_eq!(validate_session("deadbeef"), None);
    });
}

#[test]
fn lockout_after_three_fails_rejects_even_correct_password() {
    with_temp_home(|| {
        add_user("lock1", "rightpass").expect("add");
        // Three wrong attempts → locked.
        assert_eq!(check_and_record("lock1", "x"), LoginOutcome::BadCreds);
        assert_eq!(check_and_record("lock1", "x"), LoginOutcome::BadCreds);
        assert_eq!(check_and_record("lock1", "x"), LoginOutcome::BadCreds);
        assert!(is_locked("lock1"), "3 fails must lock");
        // Now even the CORRECT password is rejected as LockedOut
        // (and the password is not even checked).
        assert_eq!(
            check_and_record("lock1", "rightpass"),
            LoginOutcome::LockedOut,
            "locked account rejects correct password without checking"
        );
    });
}

#[test]
fn success_before_threshold_clears_failed_count() {
    with_temp_home(|| {
        add_user("lock2", "rightpass").expect("add");
        assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
        assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
        // Success on the 3rd attempt clears the counter.
        assert_eq!(check_and_record("lock2", "rightpass"), LoginOutcome::Ok);
        assert!(!is_locked("lock2"));
        // Two more fails should NOT lock (counter was reset).
        assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
        assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
        assert!(!is_locked("lock2"), "counter must have reset on success");
    });
}

#[test]
fn locked_attempt_does_not_extend_lock_and_expiry_clears() {
    with_temp_home(|| {
        add_user("lock3", "rightpass").expect("add");
        for _ in 0..LOCKOUT_THRESHOLD {
            assert_eq!(check_and_record("lock3", "x"), LoginOutcome::BadCreds);
        }
        assert!(is_locked("lock3"));
        // Manually force the lock to have already expired, simulating
        // the 15-minute window elapsing.
        update_sessions(|store| {
            let e = store.locks.get_mut("lock3").expect("entry");
            e.locked_until = Some(Utc::now() - Duration::seconds(1));
            Ok(())
        })
        .expect("force-expire lock");
        assert!(!is_locked("lock3"), "expired lock reads as unlocked");
        // After expiry the correct password works again and clears
        // the entry.
        assert_eq!(check_and_record("lock3", "rightpass"), LoginOutcome::Ok);
    });
}

#[test]
fn change_password_happy_path_revokes_sessions() {
    with_temp_home(|| {
        add_user("chp1", "oldpassword").expect("add");
        let tok = create_session("chp1");
        assert_eq!(validate_session(&tok), Some("chp1".to_string()));
        assert_eq!(
            change_password("chp1", "oldpassword", "newpassword"),
            ChangePasswordOutcome::Ok
        );
        assert!(verify("chp1", "newpassword"));
        assert!(!verify("chp1", "oldpassword"));
        assert_eq!(
            validate_session(&tok),
            None,
            "change-password must revoke existing sessions"
        );
    });
}

#[test]
fn change_password_wrong_current_is_bad_current() {
    with_temp_home(|| {
        add_user("chp2", "oldpassword").expect("add");
        assert_eq!(
            change_password("chp2", "WRONG", "newpassword"),
            ChangePasswordOutcome::BadCurrent
        );
        // Password unchanged.
        assert!(verify("chp2", "oldpassword"));
    });
}

#[test]
fn change_password_short_new_is_weak() {
    with_temp_home(|| {
        add_user("chp3", "oldpassword").expect("add");
        assert_eq!(
            change_password("chp3", "oldpassword", "short"),
            ChangePasswordOutcome::WeakNew(
                "Password must be at least 8 characters.".to_string()
            )
        );
        assert!(verify("chp3", "oldpassword"), "weak-new must not change pw");
    });
}

#[test]
fn change_password_through_lockout_locks_after_three_wrong_current() {
    with_temp_home(|| {
        add_user("chp4", "oldpassword").expect("add");
        for _ in 0..LOCKOUT_THRESHOLD {
            assert_eq!(
                change_password("chp4", "WRONG", "newpassword"),
                ChangePasswordOutcome::BadCurrent
            );
        }
        assert!(is_locked("chp4"), "self-service form is subject to lockout");
        // Even the correct current password is now rejected.
        assert_eq!(
            change_password("chp4", "oldpassword", "newpassword"),
            ChangePasswordOutcome::BadCurrent
        );
    });
}

#[test]
fn token_is_64_hex_chars() {
    let t = new_token();
    assert_eq!(t.len(), 64, "32 bytes -> 64 hex chars");
    assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
}

// ── K2SO #620 — password policy ─────────────────────────────────────

#[test]
fn default_policy_preserves_legacy_behavior() {
    let p = PasswordPolicy::default();
    assert_eq!(p.min_length, 8);
    assert!(!p.require_special);
    assert!(!p.require_number);
    assert!(!p.require_uppercase);
}

#[test]
fn policy_round_trips_in_store() {
    with_temp_home(|| {
        // Absent file → default.
        assert_eq!(get_policy(), PasswordPolicy::default());
        let want = PasswordPolicy {
            min_length: 12,
            require_special: true,
            require_number: true,
            require_uppercase: true,
        };
        set_policy(want.clone()).expect("set policy");
        assert_eq!(get_policy(), want, "policy must round-trip on disk");
        // Adding a user must NOT disturb the stored policy.
        add_user("pol1", "Abcdef1!ghij").expect("add under policy");
        assert_eq!(get_policy(), want, "policy survives a user mutation");
    });
}

#[test]
fn set_policy_clamps_min_length() {
    with_temp_home(|| {
        set_policy(PasswordPolicy {
            min_length: 1,
            ..Default::default()
        })
        .expect("set");
        assert_eq!(get_policy().min_length, POLICY_MIN_LENGTH_FLOOR);
        set_policy(PasswordPolicy {
            min_length: 9999,
            ..Default::default()
        })
        .expect("set");
        assert_eq!(get_policy().min_length, POLICY_MIN_LENGTH_CEIL);
    });
}

#[test]
fn validate_password_enforces_each_rule_with_message() {
    // Length.
    let p = PasswordPolicy {
        min_length: 10,
        ..Default::default()
    };
    assert_eq!(
        validate_password("short", &p),
        Err("Password must be at least 10 characters.".to_string())
    );
    assert!(validate_password("longenough!", &p).is_ok());

    // Special.
    let p = PasswordPolicy {
        min_length: 4,
        require_special: true,
        ..Default::default()
    };
    assert_eq!(
        validate_password("abcd1234", &p),
        Err("Password must include a special character.".to_string())
    );
    assert!(validate_password("abcd!", &p).is_ok());

    // Number.
    let p = PasswordPolicy {
        min_length: 4,
        require_number: true,
        ..Default::default()
    };
    assert_eq!(
        validate_password("abcdef", &p),
        Err("Password must include a number.".to_string())
    );
    assert!(validate_password("abc1", &p).is_ok());

    // Uppercase.
    let p = PasswordPolicy {
        min_length: 4,
        require_uppercase: true,
        ..Default::default()
    };
    assert_eq!(
        validate_password("abcdef", &p),
        Err("Password must include an uppercase letter.".to_string())
    );
    assert!(validate_password("abcD", &p).is_ok());
}

#[test]
fn validate_password_reports_first_unmet_rule() {
    // All rules on; a too-short pw must complain about LENGTH first.
    let p = PasswordPolicy {
        min_length: 12,
        require_special: true,
        require_number: true,
        require_uppercase: true,
    };
    assert_eq!(
        validate_password("aB1!", &p),
        Err("Password must be at least 12 characters.".to_string())
    );
}

#[test]
fn add_user_rejects_password_failing_policy() {
    with_temp_home(|| {
        set_policy(PasswordPolicy {
            min_length: 10,
            require_special: true,
            ..Default::default()
        })
        .expect("set");
        // Too short → length message.
        let err = add_user("polA", "abc").expect_err("too short rejected");
        assert_eq!(err, "Password must be at least 10 characters.");
        // Long enough but no special char → special message.
        let err = add_user("polA", "abcdefghij").expect_err("missing special rejected");
        assert_eq!(err, "Password must include a special character.");
        // Satisfies the policy → accepted.
        add_user("polA", "abcdefghi!").expect("compliant add");
    });
}

#[test]
fn change_password_rejects_password_failing_policy() {
    with_temp_home(|| {
        // Add under the default (permissive) policy, then tighten it.
        add_user("polB", "oldpassword").expect("add");
        set_policy(PasswordPolicy {
            min_length: 8,
            require_special: true,
            ..Default::default()
        })
        .expect("set");
        assert_eq!(
            change_password("polB", "oldpassword", "newpassword"),
            ChangePasswordOutcome::WeakNew(
                "Password must include a special character.".to_string()
            )
        );
        assert!(verify("polB", "oldpassword"), "weak-new must not change pw");
        // Compliant new password succeeds.
        assert_eq!(
            change_password("polB", "oldpassword", "newpass!"),
            ChangePasswordOutcome::Ok
        );
        assert!(verify("polB", "newpass!"));
    });
}

// ── K2SO #629 — role model ──────────────────────────────────────────

#[test]
fn role_ordering_is_member_lt_admin_lt_owner() {
    // VARIANT ORDER IS LOAD-BEARING for derive(Ord).
    assert!(Role::VoiceChat < Role::Member);
    assert!(Role::Member < Role::Admin);
    assert!(Role::Admin < Role::Owner);
    assert!(Role::Member < Role::Owner);
    // Legacy rows (no `role` field) deserialize as Member.
    assert_eq!(Role::default(), Role::Member);
}

// ── Voice & Chat only (prd-voice-chat-only-members-v1) ────────────

#[test]
fn voice_chat_parses_both_spellings_and_serializes_snake_case() {
    assert_eq!(Role::parse_wire("voice_chat"), Ok(Role::VoiceChat));
    assert_eq!(Role::parse_wire(" Voice-Chat "), Ok(Role::VoiceChat));
    assert_eq!(Role::VoiceChat.as_wire(), "voice_chat");
    assert_eq!(serde_json::to_string(&Role::VoiceChat).unwrap(), "\"voice_chat\"");
    assert!(VIEWER_ROLE_REMOVED.contains("voice_chat"));
}

#[test]
fn admin_may_only_move_people_between_member_and_voice_chat() {
    use Role::*;
    assert!(can_set_role(Admin, Member, VoiceChat));
    assert!(can_set_role(Admin, VoiceChat, Member));
    assert!(!can_set_role(Admin, Member, Admin), "an Admin never makes an Admin");
    assert!(!can_set_role(Admin, VoiceChat, Admin));
    assert!(!can_set_role(Admin, Admin, VoiceChat), "an Admin can't demote an Admin");
    assert!(!can_set_role(Admin, Owner, VoiceChat));
    assert!(!can_set_role(Member, VoiceChat, Member));
    assert!(!can_set_role(VoiceChat, VoiceChat, Member));
    assert!(can_set_role(Owner, Admin, VoiceChat));
    assert!(!can_act_on(VoiceChat, VoiceChat));
    assert!(can_act_on(Admin, VoiceChat));
}

/// Re-review M2: accepted risks are kept only for agents still shared,
/// sorted, and survive a store round trip.
#[test]
fn accepted_risks_follow_the_shared_agents() {
    let mut accepted = std::collections::BTreeMap::new();
    accepted.insert("p-1".to_string(), vec!["vault grants".to_string(), "a mailbox".to_string(), "a mailbox".to_string()]);
    accepted.insert("gone".to_string(), vec!["a mailbox".to_string()]);
    accepted.insert("p-2".to_string(), vec![]);
    let s = Shares { agents: vec!["p-1".into(), "p-2".into()], accepted_risks: accepted, ..Default::default() }.normalized().expect("ok");
    assert_eq!(s.accepted_risks.len(), 1, "{:?}", s.accepted_risks);
    assert_eq!(s.accepted_risks["p-1"], vec!["a mailbox".to_string(), "vault grants".to_string()]);
    let back: Shares = serde_json::from_str(&serde_json::to_string(&s).expect("ser")).expect("de");
    assert_eq!(back, s);
    let plain: Shares = serde_json::from_str(r#"{"agents":["p-1"]}"#).expect("an old row");
    assert!(plain.accepted_risks.is_empty());
    assert!(!serde_json::to_string(&plain).expect("ser").contains("accepted_risks"), "not written when empty");
}

#[test]
fn add_user_with_role_writes_the_role_and_shares_in_one_write() {
    with_temp_home(|| {
        let new = NewLogin {
            role: Some(Role::VoiceChat),
            must_change_password: false,
            shares: Shares { agents: vec![" p-1 ".into(), "p-1".into()], gardens: vec!["g-1".into()], sources: vec![], ..Default::default() },
            fixed_password: false,
        };
        add_user_with_role("rev", "password", None, None, &new).expect("add");
        // Read the file itself: the one row on disk is already voice_chat.
        let raw = std::fs::read_to_string(store_path()).expect("store");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("json");
        let row = v["users"].as_array().expect("users").iter().find(|u| u["username"] == "rev").expect("row").clone();
        assert_eq!(row["role"], "voice_chat");
        assert_eq!(row["shares"]["agents"], serde_json::json!(["p-1"]), "trimmed + deduped");
        assert_eq!(row["shares"]["gardens"], serde_json::json!(["g-1"]));
        assert_eq!(row["must_change_password"], false);
        assert_eq!(role_for_user("rev"), Some(Role::VoiceChat));
        assert!(!can_clone("rev"), "V11: never canClone");
        assert_eq!(voice_chat_shares("rev").expect("shares").agents, vec!["p-1".to_string()]);
        // Shares on any other role are refused, nothing written.
        let bad = NewLogin { role: None, must_change_password: false, fixed_password: false, shares: Shares { agents: vec!["p".into()], gardens: vec![], sources: vec![], ..Default::default() } };
        assert!(add_user_with_role("mem", "password", None, None, &bad).is_err());
        assert_eq!(role_for_user("mem"), None);
    });
}

#[test]
fn a_voice_chat_store_row_loads() {
    with_temp_home(|| {
        add_user("xv", "password").expect("seed the file");
        let raw = std::fs::read_to_string(store_path()).expect("store");
        let mut v: serde_json::Value = serde_json::from_str(&raw).expect("json");
        v["users"][0]["role"] = serde_json::json!("voice_chat");
        v["users"][0]["shares"] = serde_json::json!({"gardens": ["g-9"]});
        std::fs::write(store_path(), v.to_string()).expect("write");
        assert_eq!(role_for_user("xv"), Some(Role::VoiceChat));
        assert_eq!(voice_chat_shares("xv").expect("shares").gardens, vec!["g-9".to_string()]);
    });
}

#[test]
fn voice_chat_role_changes_are_live_and_others_still_sign_out() {
    with_temp_home(|| {
        add_user("dana", "password").expect("add");
        let tok = create_session("dana");
        let epoch0 = live_user_epoch("dana").expect("epoch");
        assert_eq!(set_role_from("dana", Role::VoiceChat).expect("down"), Role::Member);
        assert_eq!(live_user_epoch("dana"), Some(epoch0), "a downgrade keeps the epoch");
        assert_eq!(validate_session(&tok), Some("dana".to_string()), "no sign-out on a downgrade");
        set_role("dana", Role::Member).expect("up");
        assert_eq!(validate_session(&tok), Some("dana".to_string()), "no sign-out on an upgrade");
        set_role("dana", Role::Admin).expect("to admin");
        assert_ne!(live_user_epoch("dana"), Some(epoch0), "Member -> Admin bumps the epoch");
        assert_eq!(validate_session(&tok), None, "Member -> Admin still signs out");
    });
}

#[test]
fn voice_chat_logins_are_not_people_agents_see() {
    with_temp_home(|| {
        add_user("pat", "password").expect("add");
        add_user_with_role("rev", "password", None, None, &NewLogin { role: Some(Role::VoiceChat), ..Default::default() }).expect("add");
        let rows = list_people_for_agents("owner").expect("rows");
        assert!(rows.iter().any(|r| r.username == "pat"));
        assert!(!rows.iter().any(|r| r.username == "rev"), "voice_chat left out: {rows:?}");
        assert!(is_voice_chat_only("rev"));
        assert!(!is_voice_chat_only("pat"));
    });
}

/// Security review M3: a fixed password can't be changed by the person
/// and is never temporary; an Admin/owner reset still works.
#[test]
fn a_fixed_password_is_never_changed_by_its_person_nor_temporary() {
    with_temp_home(|| {
        let new = NewLogin { role: Some(Role::VoiceChat), must_change_password: true, fixed_password: true, ..Default::default() };
        add_user_with_role("fixedrev", "password1", None, None, &new).expect("add");
        assert!(is_fixed_password("fixedrev"));
        assert!(!must_change_password("fixedrev"), "never temporary");
        set_must_change_password("fixedrev", true).expect("set");
        assert!(!must_change_password("fixedrev"), "a set can't make it temporary");
        assert_eq!(change_password("fixedrev", "password1", "password2"), ChangePasswordOutcome::Fixed);
        assert!(verify("fixedrev", "password1"), "unchanged");
        set_password("fixedrev", "password3").expect("an owner reset");
        assert!(verify("fixedrev", "password3"));
        set_fixed_password("fixedrev", false).expect("unfix");
        assert_eq!(change_password("fixedrev", "password3", "password4"), ChangePasswordOutcome::Ok);
    });
}

/// M3: a Voice & Chat only login's lockout is per (username, address); a
/// Member's stays per username.
#[test]
fn a_voice_chat_lockout_is_keyed_on_the_address_too() {
    with_temp_home(|| {
        add_user_with_role("lockrev", "password1", None, None, &NewLogin { role: Some(Role::VoiceChat), ..Default::default() }).expect("add");
        for _ in 0..LOCKOUT_THRESHOLD {
            assert_eq!(check_and_record_from("lockrev", "wrong", Some("198.51.100.7")), LoginOutcome::BadCreds);
        }
        assert_eq!(check_and_record_from("lockrev", "password1", Some("198.51.100.7")), LoginOutcome::LockedOut, "that address is locked");
        assert_eq!(check_and_record_from("lockrev", "password1", Some("203.0.113.9")), LoginOutcome::Ok, "another address isn't");
        clear_lockout("lockrev").expect("clear");
        assert_eq!(check_and_record_from("lockrev", "password1", Some("198.51.100.7")), LoginOutcome::Ok, "the per-address lock clears too");
        add_user("lockmem", "password1").expect("member");
        for _ in 0..LOCKOUT_THRESHOLD {
            assert_eq!(check_and_record_from("lockmem", "wrong", Some("198.51.100.7")), LoginOutcome::BadCreds);
        }
        assert_eq!(check_and_record_from("lockmem", "password1", Some("203.0.113.9")), LoginOutcome::LockedOut, "a Member's lock is per username");
    });
}

/// L9: the role rule is checked against the role in the same write.
#[test]
fn set_role_checked_refuses_inside_the_write() {
    with_temp_home(|| {
        add_user("chkadm", "password1").expect("add");
        set_role("chkadm", Role::Admin).expect("admin");
        let e = set_role_checked(Role::Admin, "chkadm", Role::VoiceChat).expect_err("an Admin can't demote an Admin");
        assert!(e.starts_with("role_change_not_allowed"), "{e}");
        assert_eq!(role_for_user("chkadm"), Some(Role::Admin));
        add_user("chkmem", "password1").expect("add");
        assert_eq!(set_role_checked(Role::Admin, "chkmem", Role::VoiceChat), Ok(Role::Member));
        assert_eq!(role_for_user("chkmem"), Some(Role::VoiceChat));
    });
}

/// L6: one read gives the session's login and role.
#[test]
fn validate_session_role_reads_both_at_once() {
    with_temp_home(|| {
        add_user_with_role("sessrole", "password1", None, None, &NewLogin { role: Some(Role::VoiceChat), ..Default::default() }).expect("add");
        let tok = create_session("sessrole");
        assert_eq!(validate_session_role(&tok), Some(("sessrole".to_string(), Role::VoiceChat)));
        assert!(!store_unreadable());
        std::fs::write(store_path(), "{ not json").expect("corrupt");
        assert!(store_unreadable());
        assert_eq!(validate_session_role(&tok), None, "fails closed");
    });
}

#[test]
fn shares_are_only_for_voice_chat_logins() {
    with_temp_home(|| {
        add_user("pat", "password").expect("add");
        let e = set_shares("pat", Shares { agents: vec!["p".into()], gardens: vec![], sources: vec![], ..Default::default() }).expect_err("member");
        assert!(e.starts_with("not_voice_chat"), "{e}");
        set_role("pat", Role::VoiceChat).expect("down");
        set_shares("pat", Shares { agents: vec!["p".into()], gardens: vec!["g".into()], sources: vec![], ..Default::default() }).expect("set");
        assert_eq!(voice_chat_shares("pat").expect("s").gardens, vec!["g".to_string()]);
        // A Member's stored shares never act.
        set_role("pat", Role::Member).expect("up");
        assert_eq!(voice_chat_shares("pat"), None);
        // Disabled: nothing shared.
        set_role("pat", Role::VoiceChat).expect("down");
        set_disabled("pat", true).expect("disable");
        assert_eq!(voice_chat_shares("pat"), None);
        let too_many = Shares { agents: (0..=MAX_SHARES).map(|i| format!("p{i}")).collect(), gardens: vec![], sources: vec![], ..Default::default() };
        assert!(too_many.normalized().is_err());
    });
}

#[test]
fn role_wire_round_trips() {
    for r in [Role::Owner, Role::Admin, Role::Member, Role::VoiceChat] {
        assert_eq!(Role::from_wire(r.as_wire()), Some(r));
    }
    // Case-insensitive + trimmed.
    assert_eq!(Role::from_wire("  OWNER "), Some(Role::Owner));
    assert_eq!(Role::from_wire("Admin"), Some(Role::Admin));
    assert_eq!(Role::from_wire("nonsense"), None);
    // serde uses the same snake_case strings.
    assert_eq!(serde_json::to_string(&Role::Owner).unwrap(), "\"owner\"");
    assert_eq!(serde_json::to_string(&Role::Member).unwrap(), "\"member\"");
}

#[test]
fn viewer_wire_role_is_refused_with_the_removal_message() {
    assert_eq!(Role::from_wire("viewer"), None);
    assert_eq!(Role::from_wire(" Viewer "), None);
    assert_eq!(
        Role::parse_wire("viewer").expect_err("viewer must be refused"),
        VIEWER_ROLE_REMOVED
    );
    assert_eq!(
        Role::parse_wire("VIEWER").expect_err("viewer must be refused"),
        VIEWER_ROLE_REMOVED
    );
    let other = Role::parse_wire("guest").expect_err("unknown must be refused");
    assert_eq!(other, "invalid role 'guest' (expected owner|admin|member|voice_chat)");
    // The bare Role type no longer deserializes "viewer" — only the
    // ConnectUser store row tolerates it (and disables the account).
    let bare: Result<Role, _> = serde_json::from_str("\"viewer\"");
    assert!(bare.is_err(), "Role must not deserialize viewer: {bare:?}");
}

#[test]
fn can_manage_users_admin_and_owner_only() {
    assert!(can_manage_users(Role::Owner));
    assert!(can_manage_users(Role::Admin));
    assert!(!can_manage_users(Role::Member));
}

#[test]
fn can_change_roles_owner_only() {
    assert!(can_change_roles(Role::Owner));
    assert!(!can_change_roles(Role::Admin));
    assert!(!can_change_roles(Role::Member));
}

#[test]
fn can_act_on_enforces_admin_cannot_touch_owner() {
    // Owner can act on anyone.
    assert!(can_act_on(Role::Owner, Role::Owner));
    assert!(can_act_on(Role::Owner, Role::Admin));
    assert!(can_act_on(Role::Owner, Role::Member));
    // Admin can act on Admin/Member but NOT Owner.
    assert!(!can_act_on(Role::Admin, Role::Owner));
    assert!(can_act_on(Role::Admin, Role::Admin));
    assert!(can_act_on(Role::Admin, Role::Member));
    // Member can act on no one.
    assert!(!can_act_on(Role::Member, Role::Owner));
    assert!(!can_act_on(Role::Member, Role::Admin));
    assert!(!can_act_on(Role::Member, Role::Member));
}

/// Write a store the way a pre-removal daemon did: `"role":"viewer"`.
fn write_legacy_store_with_viewer(viewer: &str, member: &str) {
    let hash = hash_password("password123").expect("hash");
    let body = serde_json::json!({
        "users": [
            {
                "username": viewer,
                "password_hash": hash,
                "created_at": "2026-01-01T00:00:00Z",
                "disabled": false,
                "role": "viewer",
                "token_epoch": 4
            },
            {
                "username": member,
                "password_hash": hash,
                "created_at": "2026-01-01T00:00:00Z",
                "disabled": false,
                "role": "member",
                "token_epoch": 0
            }
        ]
    });
    let dir = config_dir();
    fs::create_dir_all(&dir).expect("mkdir store dir");
    fs::write(store_path(), body.to_string()).expect("write legacy store");
}

#[test]
fn stored_viewer_loads_disabled_was_viewer_and_cannot_log_in() {
    with_temp_home(|| {
        write_legacy_store_with_viewer("vera", "mel");
        // Fail closed on LOAD — before any migration has run.
        let views = list_users().expect("legacy store must still load");
        let vera = views.iter().find(|v| v.username == "vera").expect("vera row");
        assert_eq!(vera.role, Role::Member);
        assert!(vera.disabled, "a stored viewer must load disabled");
        assert!(vera.was_viewer, "a stored viewer must carry was_viewer");
        let mel = views.iter().find(|v| v.username == "mel").expect("mel row");
        assert!(!mel.disabled);
        assert!(!mel.was_viewer);
        assert!(!verify("vera", "password123"), "a former viewer must not log in");
        assert!(verify("mel", "password123"), "members are untouched");
        assert_eq!(
            check_and_record("vera", "password123"),
            LoginOutcome::BadCreds
        );
    });
}

#[test]
fn migrate_removed_viewers_persists_audits_and_revokes_once() {
    with_temp_home(|| {
        write_legacy_store_with_viewer("vera", "mel");
        let migrated = migrate_removed_viewers().expect("migrate");
        assert_eq!(migrated, vec!["vera".to_string()]);

        // Persisted: the file no longer says viewer; the row is a
        // disabled member with was_viewer and a bumped epoch.
        let raw = fs::read_to_string(store_path()).expect("read store");
        assert!(!raw.contains("\"viewer\""), "store still holds viewer: {raw}");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("store json");
        let users = v["users"].as_array().expect("users array");
        let vera = users
            .iter()
            .find(|u| u["username"] == "vera")
            .expect("vera persisted");
        assert_eq!(vera["role"], "member");
        assert_eq!(vera["disabled"], true);
        assert_eq!(vera["was_viewer"], true);
        assert_eq!(vera["token_epoch"], 5);
        let mel = users.iter().find(|u| u["username"] == "mel").expect("mel");
        assert!(mel.get("was_viewer").is_none(), "false was_viewer is omitted: {mel}");

        // One audit line for vera.
        let events = crate::auth_audit::tail(50).expect("audit tail");
        let lines: Vec<&serde_json::Value> = events
            .iter()
            .filter(|e| e["event"] == "role-removed")
            .collect();
        assert_eq!(lines.len(), 1, "audit: {events:?}");
        assert_eq!(lines[0]["user"], "vera");
        assert_eq!(lines[0]["outcome"], "viewer-disabled");
        assert_eq!(lines[0]["ingress"], "boot");
        assert_eq!(lines[0]["client"], "daemon");

        // Second boot: nothing left to migrate, no second audit line.
        let again = migrate_removed_viewers().expect("migrate again");
        assert!(again.is_empty(), "second run migrated {again:?}");
        let events = crate::auth_audit::tail(50).expect("audit tail");
        assert_eq!(
            events.iter().filter(|e| e["event"] == "role-removed").count(),
            1
        );
    });
}

#[test]
fn enabling_a_former_viewer_makes_a_plain_member() {
    with_temp_home(|| {
        write_legacy_store_with_viewer("vera", "mel");
        migrate_removed_viewers().expect("migrate");
        set_disabled("vera", false).expect("enable as member");
        let views = list_users().expect("list");
        let vera = views.iter().find(|v| v.username == "vera").expect("vera");
        assert!(!vera.disabled);
        assert!(!vera.was_viewer, "enable must clear was_viewer");
        assert_eq!(vera.role, Role::Member);
        assert!(verify("vera", "password123"));
        let tok = create_session("vera");
        assert_eq!(role_for_session(&tok), Some(Role::Member));
    });
}

#[test]
fn migrate_removed_viewers_is_a_no_op_without_viewers() {
    with_temp_home(|| {
        add_user("plain", "password").expect("add");
        let before = fs::read_to_string(store_path()).expect("read");
        let migrated = migrate_removed_viewers().expect("migrate");
        assert!(migrated.is_empty());
        let after = fs::read_to_string(store_path()).expect("read");
        assert_eq!(before, after, "no-op migration must not rewrite the store");
    });
}

#[test]
fn new_user_defaults_to_member_and_set_role_promotes() {
    with_temp_home(|| {
        add_user("rolea", "password").expect("add");
        assert_eq!(role_for_user("rolea"), Some(Role::Member));
        set_role("rolea", Role::Admin).expect("promote");
        assert_eq!(role_for_user("rolea"), Some(Role::Admin));
        set_role("rolea", Role::Owner).expect("promote owner");
        assert_eq!(role_for_user("rolea"), Some(Role::Owner));
        // Unknown user errors.
        assert!(set_role("ghost", Role::Admin).is_err());
        assert_eq!(role_for_user("ghost"), None);
    });
}

fn view(name: &str) -> ConnectUserView {
    list_users().expect("list").into_iter().find(|u| u.username == name).expect("user row")
}

/// 0.46.0 person names: set, partial update, clear, on-disk shape.
#[test]
fn set_names_round_trips_and_clears() {
    with_temp_home(|| {
        add_user("namea", "password").expect("add");
        let v = view("namea");
        assert_eq!((v.first_name.as_deref(), v.last_name.as_deref()), (None, None));
        let raw = std::fs::read_to_string(store_path()).expect("store");
        assert!(!raw.contains("first_name"), "unset names are omitted on disk: {raw}");

        set_names("namea", Some("  Anna "), Some("Smith")).expect("set");
        let v = view("namea");
        assert_eq!(v.first_name.as_deref(), Some("Anna"), "trimmed");
        assert_eq!(v.last_name.as_deref(), Some("Smith"));
        let wire = serde_json::to_value(&v).expect("json");
        assert_eq!(wire["firstName"], "Anna");
        assert_eq!(wire["lastName"], "Smith");
        assert_eq!(wire["displayName"], "Anna Smith");

        // None leaves a part unchanged.
        set_names("namea", None, Some("Jones")).expect("last only");
        let v = view("namea");
        assert_eq!(v.first_name.as_deref(), Some("Anna"));
        assert_eq!(v.last_name.as_deref(), Some("Jones"));

        // "" clears.
        set_names("namea", Some(""), None).expect("clear first");
        let v = view("namea");
        assert_eq!(v.first_name, None);
        assert_eq!(v.last_name.as_deref(), Some("Jones"));
        let wire = serde_json::to_value(&v).expect("json");
        assert_eq!(wire["firstName"], serde_json::Value::Null, "unset is null on the wire");
        assert_eq!(wire["displayName"], "Jones");
        set_names("namea", None, Some("")).expect("clear last");
        assert_eq!(view("namea").display_name, "namea", "no name → the username");
        set_names("namea", None, Some("Jones")).expect("restore last");

        // A bad value writes neither part.
        let err = set_names("namea", Some("Ok"), Some(&"x".repeat(65))).expect_err("over the cap");
        assert!(err.contains("lastName"), "{err}");
        let v = view("namea");
        assert_eq!(v.first_name, None);
        assert_eq!(v.last_name.as_deref(), Some("Jones"));
        assert!(set_names("namea", Some("<b>"), None).is_err());
        assert!(set_names("ghost", Some("Ghost"), None).is_err(), "unknown user");
    });
}

#[test]
fn add_user_with_names_validates_before_writing() {
    with_temp_home(|| {
        add_user_with_names("nameb", "password", Some("Bob"), Some("")).expect("add");
        let v = view("nameb");
        assert_eq!(v.first_name.as_deref(), Some("Bob"));
        assert_eq!(v.last_name, None, "empty is unset");
        let err = add_user_with_names("namec", "password", Some("Line\nBreak"), None).expect_err("bad name");
        assert!(err.contains("firstName"), "{err}");
        assert!(list_users().expect("list").iter().all(|u| u.username != "namec"), "no user created");
    });
}

#[test]
fn role_for_session_resolves_stored_role() {
    with_temp_home(|| {
        add_user("rsess", "password").expect("add");
        set_role("rsess", Role::Admin).expect("promote");
        let tok = create_session("rsess");
        assert_eq!(role_for_session(&tok), Some(Role::Admin));
        // Unknown token → None.
        assert_eq!(role_for_session("deadbeef"), None);
    });
}

#[test]
fn list_users_includes_role() {
    with_temp_home(|| {
        add_user("rl1", "password").expect("add");
        set_role("rl1", Role::Admin).expect("promote");
        let views = list_users().expect("list");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].role, Role::Admin);
        // Wire shape carries the role string.
        let json = serde_json::to_string(&views[0]).unwrap();
        assert!(json.contains("\"role\":\"admin\""), "got: {json}");
    });
}

fn assert_people_row_keys_only(row: &PeopleRow) {
    let json = serde_json::to_value(row).expect("people row json");
    let obj = json.as_object().expect("object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["disabled", "role", "username"],
        "people row must be username/role/disabled only: {json}"
    );
    let dumped = json.to_string();
    assert!(
        !dumped.contains("password")
            && !dumped.contains("hash")
            && !dumped.contains("created")
            && !dumped.contains("token_epoch")
            && !dumped.contains("tokenEpoch"),
        "people row leaked a secret field: {dumped}"
    );
}

#[test]
fn list_people_for_agents_empty_store_still_has_owner() {
    with_temp_home(|| {
        let rows = list_people_for_agents("Rosson").expect("people");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].username, "Rosson");
        assert_eq!(rows[0].role, Role::Owner);
        assert!(!rows[0].disabled);
        assert_people_row_keys_only(&rows[0]);
    });
}

#[test]
fn list_people_for_agents_includes_member_admin_and_disabled() {
    with_temp_home(|| {
        add_user("julie", "password1").expect("add");
        set_role("julie", Role::Admin).expect("promote");
        add_user("member1", "password1").expect("add");
        add_user("viewy", "password1").expect("add");
        add_user("ghost", "password1").expect("add");
        set_disabled("ghost", true).expect("disable");
        let rows = list_people_for_agents("Rosson").expect("people");
        let names: Vec<&str> = rows.iter().map(|r| r.username.as_str()).collect();
        assert_eq!(names, vec!["Rosson", "ghost", "julie", "member1", "viewy"]);
        let find = |n: &str| rows.iter().find(|r| r.username == n).expect(n);
        assert_eq!(find("Rosson").role, Role::Owner);
        assert!(!find("Rosson").disabled);
        assert_eq!(find("julie").role, Role::Admin);
        assert_eq!(find("member1").role, Role::Member);
        assert_eq!(find("viewy").role, Role::Member);
        assert_eq!(find("ghost").role, Role::Member);
        assert!(find("ghost").disabled, "disabled connect-user must appear");
        for r in &rows {
            assert_people_row_keys_only(r);
        }
    });
}

#[test]
fn list_people_for_agents_dedups_owner_case_insensitive_keeps_stored() {
    with_temp_home(|| {
        add_user("rosson", "password1").expect("add");
        set_role("rosson", Role::Admin).expect("promote");
        set_disabled("rosson", true).expect("disable");
        let rows = list_people_for_agents("Rosson").expect("people");
        assert_eq!(rows.len(), 1, "owner display matching a connect-user is one row");
        assert_eq!(rows[0].username, "rosson");
        assert_eq!(rows[0].role, Role::Admin);
        assert!(rows[0].disabled);
    });
}

#[test]
fn list_people_for_agents_blank_owner_falls_back() {
    with_temp_home(|| {
        let rows = list_people_for_agents("   ").expect("people");
        assert_eq!(rows[0].username, "owner");
        assert_eq!(rows[0].role, Role::Owner);
        assert!(!rows[0].disabled);
    });
}

#[test]
fn legacy_json_without_role_migrates_to_member() {
    with_temp_home(|| {
        // Hand-write a pre-#629 store: a user row with NO `role` field.
        let dir = config_dir();
        fs::create_dir_all(&dir).unwrap();
        let legacy = r#"{
                "users": [
                    {
                        "username": "legacy",
                        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "created_at": "2024-01-01T00:00:00Z",
                        "disabled": false
                    }
                ]
            }"#;
        fs::write(store_path(), legacy).unwrap();
        // Deserializes cleanly; missing role → Member.
        let views = list_users().expect("legacy store loads");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].username, "legacy");
        assert_eq!(views[0].role, Role::Member, "missing role defaults to Member");
        assert_eq!(role_for_user("legacy"), Some(Role::Member));
    });
}

#[test]
fn owner_set_password_clears_lockout() {
    with_temp_home(|| {
        add_user("locked", "rightpass").expect("add");
        // Drive the account into a lockout via the lockout path.
        for _ in 0..LOCKOUT_THRESHOLD {
            assert_eq!(check_and_record("locked", "x"), LoginOutcome::BadCreds);
        }
        assert!(is_locked("locked"), "must be locked after threshold");
        // Even the correct password is rejected while locked.
        assert_eq!(
            check_and_record("locked", "rightpass"),
            LoginOutcome::LockedOut
        );
        // Owner resets the password → lockout cleared immediately.
        set_password("locked", "brandnewpw").expect("owner reset");
        assert!(!is_locked("locked"), "owner reset must clear lockout");
        // A correct login with the NEW password now succeeds right away.
        assert_eq!(
            check_and_record("locked", "brandnewpw"),
            LoginOutcome::Ok,
            "unlocked user logs in immediately after owner reset"
        );
    });
}

#[test]
fn clear_lockout_reports_entry_and_does_not_create_a_user() {
    with_temp_home(|| {
        let (key, cleared) = clear_lockout("Ghost").expect("missing entry");
        assert_eq!(key, "ghost");
        assert!(!cleared);
        assert_eq!(check_and_record("Ghost", "x"), LoginOutcome::BadCreds);
        assert!(!is_locked("ghost"), "one failure is not a live lock");
        assert_eq!(lockout_failed_count("ghost"), 1);
        let (key, cleared) = clear_lockout("ghost").expect("entry existed");
        assert_eq!(key, "ghost");
        assert!(cleared);
        assert_eq!(lockout_failed_count("ghost"), 0);
        let (key, cleared) = clear_lockout("ghost").expect("second clear");
        assert_eq!(key, "ghost");
        assert!(!cleared);
        assert!(
            list_users()
                .expect("list")
                .iter()
                .all(|u| u.username != "ghost"),
            "clear must not create a connect-user"
        );
    });
}

#[test]
fn set_password_still_clears_a_partial_lock_entry() {
    with_temp_home(|| {
        add_user("baden", "oldpassword1").expect("add");
        let tok = create_session("baden");
        assert_eq!(check_and_record("baden", "nope"), LoginOutcome::BadCreds);
        assert_eq!(lockout_failed_count("baden"), 1);
        assert!(!is_locked("baden"));
        let before = fs::read_to_string(store_path()).expect("users file");
        set_password("baden", "newpassword1").expect("set");
        assert_eq!(lockout_failed_count("baden"), 0, "set_password still clears");
        let (_key, cleared) = clear_lockout("baden").expect("after set_password");
        assert!(!cleared, "entry already removed");
        assert_eq!(validate_session(&tok), None, "set_password still revokes");
        let after = fs::read_to_string(store_path()).expect("users file after");
        assert_ne!(before, after, "set_password still writes the credential");
        assert!(verify("baden", "newpassword1"));
    });
}

#[test]
fn clear_lockout_write_failure_keeps_the_entry() {
    with_temp_home(|| {
        assert_eq!(check_and_record("stuck", "x"), LoginOutcome::BadCreds);
        assert_eq!(check_and_record("stuck", "x"), LoginOutcome::BadCreds);
        assert_eq!(check_and_record("stuck", "x"), LoginOutcome::BadCreds);
        assert!(is_locked("stuck"));
        // Inject the write failure structurally: the store's temp path
        // is a non-empty directory, so `fs::write` fails with EISDIR
        // for every user (root ignores chmod, so 0555 would not).
        let blocker = sessions_store_tmp_path();
        fs::create_dir_all(blocker.join("block")).expect("create blocker dir");
        let err = clear_lockout("stuck").expect_err("unwritable session store");
        assert!(!err.is_empty(), "store error must be non-empty");
        assert!(
            err.contains(&blocker.display().to_string()),
            "error names the temp path: {err}"
        );
        fs::remove_dir_all(&blocker).expect("remove blocker dir");
        assert!(is_locked("stuck"), "failed write must leave the lock");
        assert_eq!(lockout_failed_count("stuck"), 0, "live lock stores failed_count 0");
        assert!(
            list_users()
                .expect("list")
                .iter()
                .all(|u| u.username != "stuck"),
            "failed clear must not create a user"
        );
    });
}

// ── K2SO #631 (P1) — connect-users store security ───────────────────

/// The credential store holds argon2 password hashes; it MUST land on
/// disk owner-only (0600). Locks the `restrict_mode` chmod on both the
/// initial write (add_user) AND a subsequent rewrite (set_policy), since
/// the tmp+rename path re-chmods the final file each time.
#[cfg(unix)]
#[test]
fn store_file_is_written_0600() {
    use std::os::unix::fs::PermissionsExt;
    with_temp_home(|| {
        add_user("permu", "password").expect("add");
        let mode = fs::metadata(store_path())
            .expect("stat store")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "credential store must be owner-only 0600, got {mode:o}"
        );
        // A second write (different mutation, same tmp+rename path) must
        // also leave it 0600 — rename replaces the inode, so perms are
        // re-applied, not inherited.
        set_policy(PasswordPolicy {
            min_length: 10,
            ..Default::default()
        })
        .expect("set policy");
        let mode2 = fs::metadata(store_path())
            .expect("stat store after rewrite")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode2, 0o600,
            "store must stay 0600 after a rewrite, got {mode2:o}"
        );
    });
}

/// Windows: the same store is owner-only (DACL) after a write and a
/// rewrite.
#[cfg(windows)]
#[test]
fn store_file_is_written_owner_only() {
    use crate::private_fs::is_private;
    with_temp_home(|| {
        add_user("permu", "password").expect("add");
        let p = store_path();
        assert!(is_private(&p, &fs::metadata(&p).expect("stat")), "owner-only after the first write");
        set_policy(PasswordPolicy { min_length: 10, ..Default::default() }).expect("set policy");
        assert!(is_private(&p, &fs::metadata(&p).expect("stat")), "owner-only after a rewrite");
    });
}

/// The lockout counter is keyed by username and shared by BOTH credential
/// surfaces — interactive login (`check_and_record`) and self-service
/// change-password (`change_password`, which routes its current-password
/// check through the SAME gate). An attacker must not be able to dodge the
/// 3-strike lockout by alternating between the two endpoints.
#[test]
fn lockout_counter_shared_across_login_and_change_password() {
    with_temp_home(|| {
        add_user("mixlock", "rightpass").expect("add");
        // Two failed LOGINs — counter at 2, not yet locked.
        assert_eq!(check_and_record("mixlock", "x"), LoginOutcome::BadCreds);
        assert_eq!(check_and_record("mixlock", "x"), LoginOutcome::BadCreds);
        assert!(!is_locked("mixlock"), "two fails must not lock yet");
        // A failed CHANGE-PASSWORD (wrong current) trips the SAME counter
        // to the threshold → locked. Proves one shared lockout key.
        assert_eq!(
            change_password("mixlock", "WRONG", "newpassword"),
            ChangePasswordOutcome::BadCurrent
        );
        assert!(
            is_locked("mixlock"),
            "3rd fail (via change-password) must lock the shared counter"
        );
        // And the lock applies back on the LOGIN surface: the correct
        // password is rejected without being checked.
        assert_eq!(
            check_and_record("mixlock", "rightpass"),
            LoginOutcome::LockedOut,
            "a lockout earned via change-password also blocks login"
        );
    });
}

/// Pre-#620 stores have no top-level `policy` field. They must load
/// (the `#[serde(default)]` on `Store::policy` yields the permissive
/// default) and remain fully usable — the serde-migration sibling of
/// `legacy_json_without_role_migrates_to_member`.
#[test]
fn legacy_json_without_policy_loads_default_policy() {
    with_temp_home(|| {
        let dir = config_dir();
        fs::create_dir_all(&dir).unwrap();
        // A store with a user row but NO `policy` field and NO `role`.
        let legacy = r#"{
                "users": [
                    {
                        "username": "oldtimer",
                        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "created_at": "2024-01-01T00:00:00Z",
                        "disabled": false
                    }
                ]
            }"#;
        fs::write(store_path(), legacy).unwrap();
        // Missing policy → permissive default.
        assert_eq!(
            get_policy(),
            PasswordPolicy::default(),
            "absent policy field must yield the default policy"
        );
        // And the store is still usable: the legacy user lists, and a new
        // add succeeds under the default (length-8) policy.
        let views = list_users().expect("legacy store loads");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].username, "oldtimer");
        add_user("newcomer", "password").expect("add under default policy");
        assert_eq!(list_users().unwrap().len(), 2);
    });
}

// ── K2 Connect #4 — secure persistent sessions ──────────────────────

/// MUST #1: the on-disk session value is the SHA-256 DIGEST of the
/// token, never the live token itself. A stolen store yields only
/// non-replayable digests.
#[test]
fn stored_session_value_is_digest_not_raw_token() {
    with_temp_home(|| {
        add_user("digestu", "password").expect("add");
        let tok = create_session("digestu");
        let raw = fs::read_to_string(sessions_store_path()).expect("read sessions store");
        assert!(
            !raw.contains(&tok),
            "raw token must NOT appear in the on-disk session store"
        );
        assert!(
            raw.contains(&token_digest(&tok)),
            "the SHA-256 digest of the token MUST be persisted"
        );
        // And the digest is the 64-hex SHA-256, not the token.
        assert_eq!(token_digest(&tok).len(), 64);
        assert_ne!(token_digest(&tok), tok);
    });
}

/// MUST #2 (each of the 5 revoke sites bumps the epoch → old token
/// rejected). disable / set_password / set_role / change_password all
/// bump the durable epoch; remove deletes the user (epoch moot, the
/// record is gone). Each must reject a previously-valid token.
#[test]
fn each_revoke_site_invalidates_old_token() {
    with_temp_home(|| {
        // set_disabled
        add_user("rv_dis", "password").expect("add");
        let t = create_session("rv_dis");
        assert_eq!(validate_session(&t), Some("rv_dis".to_string()));
        set_disabled("rv_dis", true).expect("disable");
        assert_eq!(validate_session(&t), None, "disable must reject old token");

        // set_password
        add_user("rv_pw", "password").expect("add");
        let t = create_session("rv_pw");
        assert_eq!(validate_session(&t), Some("rv_pw".to_string()));
        set_password("rv_pw", "newpassword").expect("set-password");
        assert_eq!(validate_session(&t), None, "set_password must reject old token");

        // set_role
        add_user("rv_role", "password").expect("add");
        let t = create_session("rv_role");
        assert_eq!(validate_session(&t), Some("rv_role".to_string()));
        set_role("rv_role", Role::Admin).expect("set-role");
        assert_eq!(validate_session(&t), None, "set_role must reject old token");

        // change_password
        add_user("rv_chp", "oldpassword").expect("add");
        let t = create_session("rv_chp");
        assert_eq!(validate_session(&t), Some("rv_chp".to_string()));
        assert_eq!(
            change_password("rv_chp", "oldpassword", "newpassword"),
            ChangePasswordOutcome::Ok
        );
        assert_eq!(validate_session(&t), None, "change_password must reject old token");

        // remove_user
        add_user("rv_rm", "password").expect("add");
        let t = create_session("rv_rm");
        assert_eq!(validate_session(&t), Some("rv_rm".to_string()));
        remove_user("rv_rm").expect("remove");
        assert_eq!(validate_session(&t), None, "remove must reject old token");
    });
}

/// Direct epoch-bump assertion: the durable per-user epoch advances on
/// each of the four mutate-in-place revoke sites.
#[test]
fn revoke_sites_bump_token_epoch() {
    with_temp_home(|| {
        add_user("epu", "password").expect("add");
        assert_eq!(user_auth_state("epu"), Some((false, 0)));
        set_disabled("epu", true).expect("disable");
        assert_eq!(user_auth_state("epu"), Some((true, 1)));
        set_disabled("epu", false).expect("re-enable"); // no bump on enable
        assert_eq!(user_auth_state("epu"), Some((false, 1)));
        set_password("epu", "newpassword").expect("set-pw");
        assert_eq!(user_auth_state("epu"), Some((false, 2)));
        set_role("epu", Role::Admin).expect("set-role");
        assert_eq!(user_auth_state("epu"), Some((false, 3)));
    });
}

/// MUST #2 / the BOOT-RACE: a persisted session SURVIVES a simulated
/// restart (reload from disk), BUT a session whose user was disabled
/// while the daemon was "down" is rejected on the very first validate.
#[test]
fn session_survives_restart_but_disabled_while_down_is_rejected() {
    with_temp_home(|| {
        add_user("survivor", "password").expect("add");
        let live = create_session("survivor");
        add_user("victim", "password").expect("add");
        let stale = create_session("victim");

        // Simulate a restart: the persisted store is the ONLY state
        // (there is no in-memory map anymore). Reload from disk and
        // confirm the live session is still valid.
        let reloaded = load_sessions().expect("reload from disk");
        assert_eq!(reloaded.sessions.len(), 2, "both records persisted");
        assert_eq!(
            validate_session(&live),
            Some("survivor".to_string()),
            "a persisted session must survive a restart"
        );

        // Now disable the victim's account DIRECTLY in the user store —
        // simulating a change made while the daemon was down (e.g. an
        // owner edit synced in, #47). The first validate after "boot"
        // must reject the stale session (epoch bump + !disabled check).
        set_disabled("victim", true).expect("disable while down");
        assert_eq!(
            validate_session(&stale),
            None,
            "boot-race: a user disabled while down must be rejected on validate"
        );
        // And the stale record was deleted from disk by that validate.
        let after = load_sessions().expect("reload after reject");
        assert!(
            after.sessions.iter().all(|s| s.username != "victim"),
            "rejected stale record must be deleted from disk"
        );
        // The survivor is untouched.
        assert_eq!(validate_session(&live), Some("survivor".to_string()));
    });
}

/// MUST: logout deletes ONLY the caller's record (per-device), leaving
/// the user's other sessions intact.
#[test]
fn logout_deletes_only_the_callers_record() {
    with_temp_home(|| {
        add_user("multi", "password").expect("add");
        let device_a = create_session("multi");
        let device_b = create_session("multi");
        assert_eq!(validate_session(&device_a), Some("multi".to_string()));
        assert_eq!(validate_session(&device_b), Some("multi".to_string()));

        assert_eq!(logout_session(&device_a), Some("multi".to_string()));
        assert_eq!(validate_session(&device_a), None, "logged-out session rejected");
        assert_eq!(
            validate_session(&device_b),
            Some("multi".to_string()),
            "the other device stays logged in"
        );
        // Logging out an unknown token is a no-op None.
        assert_eq!(logout_session("not-a-token"), None);
    });
}

/// Presence S3 groundwork: every minted session carries a uuid-v4
/// `session_id`, and revoking reports how many records it deleted.
#[test]
fn create_session_mints_session_id_and_revoke_counts() {
    with_temp_home(|| {
        add_user("sid_user", "password").expect("add");
        let _t1 = create_session("sid_user");
        let _t2 = create_session("sid_user");
        let store = load_sessions().expect("load sessions");
        assert_eq!(store.sessions.len(), 2);
        let ids: Vec<&String> =
            store.sessions.iter().map(|s| &s.session_id).collect();
        for id in &ids {
            assert_eq!(id.len(), 36, "uuid v4 string, got {id:?}");
            assert!(
                uuid::Uuid::parse_str(id).is_ok(),
                "session_id must parse as a uuid: {id:?}"
            );
        }
        assert_ne!(ids[0], ids[1], "ids are per-session, not per-user");
        assert_eq!(revoke_user_sessions("sid_user"), 2);
        assert_eq!(revoke_user_sessions("sid_user"), 0, "idempotent");
    });
}

/// PRD connect-login-edge-only T1: sessions live 7 days (was 30). The
/// persisted `expires_at` must be exactly `created_at + 7d`, and the
/// canonical TTL the route layer reads must agree.
#[test]
fn create_session_expires_in_seven_days() {
    with_temp_home(|| {
        assert_eq!(session_ttl_days(), 7, "SESSION_TTL_DAYS must be 7");
        add_user("ttl_user", "password").expect("add");
        let _t = create_session("ttl_user");
        let store = load_sessions().expect("load sessions");
        assert_eq!(store.sessions.len(), 1);
        let rec = &store.sessions[0];
        assert_eq!(
            rec.expires_at - rec.created_at,
            Duration::days(7),
            "expires_at must be created_at + 7 days"
        );
    });
}

/// Home M2 (MS44 b / MS82): validating a live session — every authed
/// request and every socket's 5 s re-check — must NOT rewrite
/// `connect-sessions.json`. A real change still does. Each save is a
/// tmp-file + rename, so the file's inode changes on every rewrite and
/// stays the same when nothing was written.
#[cfg(unix)]
#[test]
fn validate_session_does_not_rewrite_the_file_unless_a_record_changes() {
    use std::os::unix::fs::MetadataExt;
    with_temp_home(|| {
        add_user("steady", "password").expect("add");
        let token = create_session("steady");
        let path = sessions_store_path();
        let ino = |p: &std::path::Path| fs::metadata(p).expect("sessions file exists").ino();
        let bytes_before = fs::read(&path).expect("read sessions file");
        let ino_before = ino(&path);
        for _ in 0..25 {
            assert_eq!(validate_session(&token).as_deref(), Some("steady"));
        }
        assert_eq!(validate_session("not-a-token"), None);
        assert_eq!(ino(&path), ino_before, "a steady validate must not rewrite the file");
        assert_eq!(fs::read(&path).expect("read sessions file"), bytes_before);

        // A new session is a real change: rewritten.
        let other = create_session("steady");
        let ino_after_create = ino(&path);
        assert_ne!(ino_after_create, ino_before, "a new session must be saved");

        // Revoking is a real change too, and the revoked token is gone.
        assert_eq!(revoke_user_sessions("steady"), 2);
        assert_ne!(ino(&path), ino_after_create, "a revoke must be saved");
        assert_eq!(validate_session(&token), None);
        assert_eq!(validate_session(&other), None);
        assert!(load_sessions().expect("load").sessions.is_empty());
    });
}

/// Home M2 (MS82): an expired record found by validate is still reaped
/// from disk (the one validate path that changes a record).
#[cfg(unix)]
#[test]
fn validate_session_still_saves_when_it_reaps_an_expired_record() {
    use std::os::unix::fs::MetadataExt;
    with_temp_home(|| {
        add_user("expiring", "password").expect("add");
        let tok = new_token();
        update_sessions(|store| {
            store.sessions.push(SessionRecord {
                version: SESSION_RECORD_VERSION,
                username: "expiring".to_string(),
                token_digest: token_digest(&tok),
                created_at: Utc::now() - Duration::days(8),
                expires_at: Utc::now() - Duration::seconds(1),
                token_epoch: 0,
                created_ip: None,
                user_agent: None,
                session_id: uuid::Uuid::new_v4().to_string(),
                device: None,
            });
            Ok(())
        })
        .expect("inject expired record");
        let path = sessions_store_path();
        let ino_before = fs::metadata(&path).expect("file").ino();
        assert_eq!(validate_session(&tok), None);
        assert_ne!(fs::metadata(&path).expect("file").ino(), ino_before, "the reap must be saved");
        assert!(load_sessions().expect("load").sessions.is_empty());
    });
}

/// MUST: the daemon-canonical reaper evicts an expired record.
#[test]
fn reaper_evicts_expired_record() {
    with_temp_home(|| {
        add_user("reapu", "password").expect("add");
        let live = create_session("reapu");
        // Inject an expired record for the same user.
        let expired_tok = new_token();
        update_sessions(|store| {
            store.sessions.push(SessionRecord {
                version: SESSION_RECORD_VERSION,
                username: "reapu".to_string(),
                token_digest: token_digest(&expired_tok),
                created_at: Utc::now() - Duration::days(40),
                expires_at: Utc::now() - Duration::seconds(5),
                token_epoch: 0,
                created_ip: None,
                user_agent: None,
                session_id: String::new(),
                device: None,
            });
            Ok(())
        })
        .expect("inject expired");
        assert_eq!(load_sessions().unwrap().sessions.len(), 2);
        let reaped = reap_expired_sessions();
        assert_eq!(reaped, 1, "exactly the expired record is reaped");
        let after = load_sessions().unwrap();
        assert_eq!(after.sessions.len(), 1, "only the live record remains");
        // The live session still validates after a reap.
        assert_eq!(validate_session(&live), Some("reapu".to_string()));
    });
}

/// MUST: a record/envelope with an UNKNOWN version fails CLOSED (the
/// store is treated as empty → forced re-login), and a hand-rolled
/// legacy RAW-TOKEN row (no `token_digest`) is dropped on load.
#[test]
fn unknown_version_and_raw_token_row_fail_closed() {
    with_temp_home(|| {
        let dir = config_dir();
        fs::create_dir_all(&dir).unwrap();

        // (a) Unknown ENVELOPE version → empty store.
        let future = r#"{ "version": 9999, "sessions": [
                { "version": 9999, "username": "u", "token_digest": "abc",
                  "created_at": "2024-01-01T00:00:00Z",
                  "expires_at": "2999-01-01T00:00:00Z", "token_epoch": 0 } ] }"#;
        fs::write(sessions_store_path(), future).unwrap();
        let s = load_sessions().expect("loads (fail closed) on unknown envelope version");
        assert!(s.sessions.is_empty(), "unknown envelope version fails closed (empty)");

        // (b) Current envelope, but a record with a WRONG record version
        // and a legacy RAW-TOKEN row (no token_digest field). Both must
        // be dropped on load (fail closed).
        let mixed = r#"{ "version": 1, "sessions": [
                { "version": 2, "username": "wrongver", "token_digest": "deadbeef",
                  "created_at": "2024-01-01T00:00:00Z",
                  "expires_at": "2999-01-01T00:00:00Z", "token_epoch": 0 },
                { "version": 1, "username": "rawtok", "token": "PLAINTEXTTOKEN",
                  "token_digest": "",
                  "created_at": "2024-01-01T00:00:00Z",
                  "expires_at": "2999-01-01T00:00:00Z", "token_epoch": 0 } ] }"#;
        fs::write(sessions_store_path(), mixed).unwrap();
        let s = load_sessions().expect("loads, dropping bad rows");
        assert!(
            s.sessions.is_empty(),
            "wrong-version + raw-token rows must both be dropped (fail closed)"
        );
    });
}

/// MUST #1 (perms): the session store lands on disk owner-only (0600),
/// on the initial write AND a subsequent rewrite (tmp+rename re-chmods).
#[cfg(unix)]
#[test]
fn session_store_file_is_written_0600() {
    use std::os::unix::fs::PermissionsExt;
    with_temp_home(|| {
        add_user("permsess", "password").expect("add");
        create_session("permsess"); // first write
        let mode = fs::metadata(sessions_store_path())
            .expect("stat sessions store")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "session store must be 0600, got {mode:o}");
        // A second write (a logout) re-chmods the new inode to 0600.
        create_session("permsess");
        let mode2 = fs::metadata(sessions_store_path())
            .expect("stat after rewrite")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode2, 0o600, "session store stays 0600 after rewrite, got {mode2:o}");
    });
}

/// The brute-force lockout counter is PERSISTED — it survives a
/// simulated restart so a self-update reboot can't reset it.
#[test]
fn lockout_counter_persists_across_restart() {
    with_temp_home(|| {
        add_user("plock", "rightpass").expect("add");
        for _ in 0..LOCKOUT_THRESHOLD {
            assert_eq!(check_and_record("plock", "x"), LoginOutcome::BadCreds);
        }
        assert!(is_locked("plock"), "locked after the threshold");
        // Simulate a restart: state is on disk only. Reload + confirm
        // the lock is still in force (correct password rejected).
        let reloaded = load_sessions().expect("reload locks from disk");
        assert!(
            reloaded.locks.get("plock").and_then(|e| e.locked_until).is_some(),
            "lockout must be persisted"
        );
        assert_eq!(
            check_and_record("plock", "rightpass"),
            LoginOutcome::LockedOut,
            "persisted lockout must survive a restart"
        );
    });
}

// ── K2 Cloud S1 — must_change_password ──────────────────────────

/// BACK-COMPAT (required by the S1 spec): a LEGACY connect-users.json
/// blob — written before `must_change_password` existed (and, for the
/// oldest rows, before `role`/`token_epoch`) — must deserialize with
/// the flag defaulting to `false`, and the store must be fully usable.
#[test]
fn legacy_store_json_without_flag_deserializes_false() {
    with_temp_home(|| {
        let dir = config_dir();
        fs::create_dir_all(&dir).unwrap();
        // Two vintages of legacy row: pre-#629 (no role/token_epoch)
        // and pre-S1 (role+epoch present, no must_change_password).
        let legacy = r#"{
                "users": [
                    {
                        "username": "oldest",
                        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$abcdefgh$ijklmnop",
                        "created_at": "2025-01-01T00:00:00Z",
                        "disabled": false
                    },
                    {
                        "username": "pres1",
                        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$qrstuvwx$yzabcdef",
                        "created_at": "2026-01-01T00:00:00Z",
                        "disabled": false,
                        "role": "admin",
                        "token_epoch": 3
                    }
                ]
            }"#;
        fs::write(store_path(), legacy).unwrap();

        let store = load_store().expect("legacy blob must parse");
        assert_eq!(store.users.len(), 2, "both legacy rows load");
        for u in &store.users {
            assert!(
                !u.must_change_password,
                "missing field must default false for '{}'",
                u.username
            );
        }
        assert_eq!(store.users[1].role, Role::Admin);
        assert_eq!(store.users[1].token_epoch, 3);
        // The public accessor agrees.
        assert!(!must_change_password("oldest"));
        assert!(!must_change_password("pres1"));
    });
}

#[test]
fn set_must_change_password_flags_and_owner_reset_clears() {
    with_temp_home(|| {
        add_user("rotator", "password1").expect("add");
        assert!(!must_change_password("rotator"), "defaults false on add");
        set_must_change_password("rotator", true).expect("flag");
        assert!(must_change_password("rotator"));
        // The flag does NOT revoke an existing session by itself —
        // restriction is the route layer's job.
        let tok = create_session("rotator");
        assert_eq!(validate_session(&tok), Some("rotator".to_string()));
        // Owner reset (set_password) clears the flag.
        set_password("rotator", "password2").expect("owner reset");
        assert!(
            !must_change_password("rotator"),
            "set_password must clear the flag"
        );
    });
}

#[test]
fn change_password_clears_flag_and_revokes_sessions() {
    with_temp_home(|| {
        add_user("selfrot", "password1").expect("add");
        set_must_change_password("selfrot", true).expect("flag");
        let tok = create_session("selfrot");
        assert_eq!(
            change_password("selfrot", "password1", "password2"),
            ChangePasswordOutcome::Ok
        );
        assert!(
            !must_change_password("selfrot"),
            "change_password must clear the flag"
        );
        assert_eq!(
            validate_session(&tok),
            None,
            "change_password revokes every session (forced fresh login)"
        );
        assert!(verify("selfrot", "password2"));
    });
}

#[test]
fn set_must_change_password_unknown_user_errors() {
    with_temp_home(|| {
        let err = set_must_change_password("ghostrot", true).expect_err("must error");
        assert!(err.contains("not found"), "got: {err}");
    });
}

// ── 0.46.3: a phone push registration ends with its login ────────

/// Register a phone the way the push route does: linked to the
/// caller's session id (`cu_…`), never the token.
fn push_reg(device: &str, user: &str, token: &str) {
    crate::push_devices::upsert_device_for_session(
        device,
        "apns",
        &format!("apns-{device}"),
        user,
        None,
        Some(&session_sid(token)),
    )
    .expect("register device");
}

/// 0.46.9: the `[push]` device records of one login with one reason
/// (`action:device`, sorted). Logins are unique per test, so parallel
/// tests never mix.
fn push_events(login: &str, reason: &str) -> Vec<String> {
    let mut v: Vec<String> = crate::push::device_log::recent(None)
        .into_iter()
        .filter(|e| e.login == login && e.reason == reason)
        .map(|e| format!("{}:{}", e.action, e.device))
        .collect();
    v.sort();
    v
}

fn push_ids() -> Vec<String> {
    let mut ids: Vec<String> = crate::push_devices::list_devices()
        .expect("list devices")
        .into_iter()
        .map(|d| d.device_id)
        .collect();
    ids.sort();
    ids
}

/// Logout drops ONLY the logged-out session's phone: the same user's
/// other session keeps its phone, and the owner's phone is untouched.
#[test]
fn push_logout_drops_only_that_sessions_device() {
    let _db = crate::db::scoped_for_test();
    with_temp_home(|| {
        add_user("pushmulti", "password").expect("add");
        let a = create_session("pushmulti");
        let b = create_session("pushmulti");
        push_reg("phone-a", "pushmulti", &a);
        push_reg("phone-b", "pushmulti", &b);
        crate::push_devices::upsert_device("phone-owner", "apns", "apns-owner", "owner").expect("owner");
        assert_eq!(push_ids(), vec!["phone-a", "phone-b", "phone-owner"]);

        assert_eq!(push_events("pushmulti", "register"), vec!["add:phone-a", "add:phone-b"]);
        assert_eq!(logout_session(&a), Some("pushmulti".to_string()));
        assert_eq!(push_ids(), vec!["phone-b", "phone-owner"], "only the logged-out phone goes");
        assert_eq!(push_events("pushmulti", "logout"), vec!["remove:phone-a"], "the logout is logged");
        // An unknown-token logout ends nothing.
        assert_eq!(logout_session("not-a-token"), None);
        assert_eq!(push_ids(), vec!["phone-b", "phone-owner"]);
    });
}

/// Every revoke path ends the phone's registration: kick
/// (`revoke_user_sessions`), password reset, role change, disable,
/// self-service password change, and removal. The owner's phone and
/// another user's phone stay throughout.
#[test]
fn push_revoke_paths_drop_the_users_devices() {
    let _db = crate::db::scoped_for_test();
    with_temp_home(|| {
        add_user("pushrev", "password").expect("add");
        add_user("pushother", "password").expect("add other");
        let other = create_session("pushother");
        push_reg("phone-other", "pushother", &other);
        crate::push_devices::upsert_device("phone-owner", "apns", "apns-owner", "owner").expect("owner");
        let keep = vec!["phone-other".to_string(), "phone-owner".to_string()];
        let with_rev = |ids: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = keep.iter().cloned().chain(ids.iter().map(|s| s.to_string())).collect();
            v.sort();
            v
        };

        let s = create_session("pushrev");
        push_reg("phone-rev", "pushrev", &s);
        assert_eq!(push_ids(), with_rev(&["phone-rev"]));
        assert_eq!(revoke_user_sessions("pushrev"), 1);
        assert_eq!(push_ids(), keep, "kick / revoke");
        assert_eq!(push_events("pushrev", "revoked"), vec!["remove:phone-re"], "the revoke is logged");

        let s = create_session("pushrev");
        push_reg("phone-rev", "pushrev", &s);
        set_password("pushrev", "password2").expect("reset");
        assert_eq!(push_ids(), keep, "password reset");

        let s = create_session("pushrev");
        push_reg("phone-rev", "pushrev", &s);
        set_role("pushrev", Role::Admin).expect("role");
        assert_eq!(push_ids(), keep, "role change");

        let s = create_session("pushrev");
        push_reg("phone-rev", "pushrev", &s);
        set_disabled("pushrev", true).expect("disable");
        assert_eq!(push_ids(), keep, "disable");
        set_disabled("pushrev", false).expect("enable");

        let s = create_session("pushrev");
        push_reg("phone-rev", "pushrev", &s);
        assert_eq!(
            change_password("pushrev", "password2", "password3"),
            ChangePasswordOutcome::Ok
        );
        assert_eq!(push_ids(), keep, "self-service password change");

        let s = create_session("pushrev");
        push_reg("phone-rev", "pushrev", &s);
        remove_user("pushrev").expect("remove");
        assert_eq!(push_ids(), keep, "removal");
        assert_eq!(validate_session(&other), Some("pushother".to_string()));
    });
}

fn push_inject_expired(user: &str) -> String {
    let tok = new_token();
    update_sessions(|store| {
        store.sessions.push(SessionRecord {
            version: SESSION_RECORD_VERSION,
            username: user.to_string(),
            token_digest: token_digest(&tok),
            created_at: Utc::now() - Duration::days(8),
            expires_at: Utc::now() - Duration::seconds(1),
            token_epoch: 0,
            created_ip: None,
            user_agent: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            device: None,
        });
        Ok(())
    })
    .expect("inject expired record");
    tok
}

/// Expiry ends the phone's registration both ways an expired session
/// is found: presented (validate reaps it) and the background reaper.
#[test]
fn push_expiry_drops_the_device() {
    let _db = crate::db::scoped_for_test();
    with_temp_home(|| {
        add_user("pushexp", "password").expect("add");
        let live = create_session("pushexp");
        push_reg("phone-live", "pushexp", &live);

        let presented = push_inject_expired("pushexp");
        push_reg("phone-presented", "pushexp", &presented);
        let swept = push_inject_expired("pushexp");
        push_reg("phone-swept", "pushexp", &swept);
        assert_eq!(push_ids(), vec!["phone-live", "phone-presented", "phone-swept"]);

        assert_eq!(validate_session(&presented), None, "expired");
        assert_eq!(push_ids(), vec!["phone-live", "phone-swept"], "validate reaped it");
        assert_eq!(push_events("pushexp", "session_expired"), vec!["remove:phone-pr"]);
        assert_eq!(reap_expired_sessions(), 1);
        assert_eq!(push_ids(), vec!["phone-live"], "the reaper reaped it");
        assert_eq!(push_events("pushexp", "session_expired"), vec!["remove:phone-pr", "remove:phone-sw"]);
        assert_eq!(validate_session(&live), Some("pushexp".to_string()));
    });
}

/// Sessions never refresh; a new login on the same phone re-registers
/// with the same deviceId, which re-links the row. Ending the OLD login
/// then keeps the phone; ending the new one drops it.
#[test]
fn push_new_login_relinks_and_the_old_logout_keeps_the_device() {
    let _db = crate::db::scoped_for_test();
    with_temp_home(|| {
        add_user("pushrelink", "password").expect("add");
        let old = create_session("pushrelink");
        push_reg("phone-r", "pushrelink", &old);
        let new = create_session("pushrelink");
        push_reg("phone-r", "pushrelink", &new);
        assert_eq!(push_events("pushrelink", "moved"), vec!["update:phone-r"], "a new session is a move");
        let moved = crate::push::device_log::recent(Some("phone-r"))
            .into_iter()
            .rfind(|e| e.login == "pushrelink" && e.reason == "moved")
            .expect("moved record");
        assert_eq!(
            moved.detail.as_deref(),
            Some(format!("from login=pushrelink session={}", crate::push::device_log::short_session(&session_sid(&old))).as_str())
        );
        push_reg("phone-r", "pushrelink", &new);
        assert_eq!(push_events("pushrelink", "refresh"), vec!["update:phone-r"], "the same again is a refresh");
        assert_eq!(
            crate::push_devices::device_session_sid("phone-r").expect("sid"),
            Some(session_sid(&new))
        );
        assert_eq!(logout_session(&old), Some("pushrelink".to_string()));
        assert_eq!(push_ids(), vec!["phone-r"], "the old login no longer owns it");
        assert_eq!(logout_session(&new), Some("pushrelink".to_string()));
        assert!(push_ids().is_empty(), "the current login's logout drops it");
    });
}

/// The sweep backstop: logins that ended with no end event (an epoch
/// bump while the session was never presented again; a session file
/// lost) lose their phones; a live login keeps its phone; legacy rows
/// (no link, from before 0195) go only when their user has no live
/// login or they are older than the session lifetime; owner rows stay.
#[test]
fn push_sweep_drops_devices_whose_login_is_gone() {
    let _db = crate::db::scoped_for_test();
    with_temp_home(|| {
        add_user("pushlive", "password").expect("add");
        add_user("pushbumped", "password").expect("add");
        add_user("pushnone", "password").expect("add");
        let live = create_session("pushlive");
        let bumped = create_session("pushbumped");
        push_reg("phone-live", "pushlive", &live);
        push_reg("phone-bumped", "pushbumped", &bumped);
        crate::push_devices::upsert_device("phone-owner", "apns", "apns-owner", "owner").expect("owner");
        // Legacy (pre-0195) rows: no session link.
        crate::push_devices::upsert_device("legacy-live-user", "apns", "l1", "pushlive").expect("legacy");
        crate::push_devices::upsert_device("legacy-old", "apns", "l2", "pushlive").expect("legacy");
        crate::push_devices::upsert_device("legacy-no-login", "apns", "l3", "pushnone").expect("legacy");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            // Everything registered a minute ago (before the sweep's
            // snapshot); legacy-old long before the session lifetime.
            conn.execute("UPDATE push_devices SET updated_at = updated_at - 60, last_seen_at = last_seen_at - 60", [])
                .expect("backdate");
            conn.execute(
                "UPDATE push_devices SET last_seen_at = last_seen_at - 8 * 86400 WHERE device_id = 'legacy-old'",
                [],
            )
            .expect("backdate legacy-old");
        }
        // The epoch moves on disk while nothing presents the session
        // (e.g. changed while the daemon was down): no end event.
        update_store(|store| {
            let u = store.users.iter_mut().find(|u| u.username == "pushbumped").expect("user");
            u.token_epoch = u.token_epoch.wrapping_add(1);
            Ok(())
        })
        .expect("bump epoch");
        assert_eq!(push_ids().len(), 6);

        assert_eq!(prune_push_devices_without_live_session().expect("sweep"), 3);
        assert_eq!(push_ids(), vec!["legacy-live-user", "phone-live", "phone-owner"]);
        assert_eq!(push_events("pushbumped", "no_live_login"), vec!["remove:phone-bu"]);
        assert_eq!(push_events("pushnone", "no_live_login"), vec!["remove:legacy-n"]);
        assert_eq!(push_events("pushlive", "no_live_login"), vec!["remove:legacy-o"]);

        // The session file is lost: every linked phone goes, the
        // legacy row of a now login-less user too; the owner stays.
        fs::remove_file(sessions_store_path()).expect("remove sessions file");
        assert_eq!(prune_push_devices_without_live_session().expect("sweep"), 2);
        assert_eq!(push_ids(), vec!["phone-owner"]);
    });
}

/// A phone registered at or after the sweep's snapshot is left alone
/// (its login may be newer than the snapshot).
#[test]
fn push_sweep_skips_a_device_registered_after_its_snapshot() {
    let _db = crate::db::scoped_for_test();
    with_temp_home(|| {
        add_user("pushrace", "password").expect("add");
        let gone = create_session("pushrace");
        push_reg("phone-race", "pushrace", &gone);
        // Its session is gone from the file with no end event, but the
        // row was written this very second: not swept yet.
        fs::remove_file(sessions_store_path()).expect("remove sessions file");
        assert_eq!(prune_push_devices_without_live_session().expect("sweep"), 0);
        assert_eq!(push_ids(), vec!["phone-race"]);
    });
}

/// 0.46.9 send-time re-check: a phone whose login is no longer live
/// (here: its user's epoch moved, so no end event dropped the row yet)
/// is not sent to, while a live login's phone and the owner's are.
#[test]
fn push_send_recheck_skips_a_phone_whose_login_is_gone() {
    let _db = crate::db::scoped_for_test();
    with_temp_home(|| {
        add_user("pushchklive", "password").expect("add");
        add_user("pushchkgone", "password").expect("add");
        let live = create_session("pushchklive");
        let gone = create_session("pushchkgone");
        push_reg("phone-chk-live", "pushchklive", &live);
        push_reg("phone-chk-gone", "pushchkgone", &gone);
        crate::push_devices::upsert_device("phone-chk-owner", "apns", "apns-chk-owner", "owner").expect("owner");
        let read = crate::push_devices::list_devices().expect("list");
        update_store(|store| {
            let u = store.users.iter_mut().find(|u| u.username == "pushchkgone").expect("user");
            u.token_epoch = u.token_epoch.wrapping_add(1);
            Ok(())
        })
        .expect("bump epoch");
        let (send, skipped) = crate::push_devices::recheck_for_send(&read);
        let mut ids: Vec<&str> = send.iter().map(|d| d.device_id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["phone-chk-live", "phone-chk-owner"]);
        let why: Vec<(&str, &str)> = skipped.iter().map(|(d, w)| (d.device_id.as_str(), w.as_wire())).collect();
        assert_eq!(why, [("phone-chk-gone", "no_live_login")]);
        assert!(push_ids().contains(&"phone-chk-gone".to_string()), "the re-check never deletes; the sweep does");
    });
}
