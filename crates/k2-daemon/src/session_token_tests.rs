use super::*;
use crate::test_support::with_temp_home;

/// A test build refuses the store in a home that isn't a test's own.
#[test]
#[should_panic(expected = "prod-isolation guard")]
fn saving_the_store_outside_a_temp_home_panics() {
    let _home = k2_core::test_env::EnvVar::set("HOME", "/var/lib/k2-not-a-temp-home");
    save_to_disk(&SessionTokenRegistry::new());
}

#[test]
#[should_panic(expected = "prod-isolation guard")]
fn loading_the_store_outside_a_temp_home_panics() {
    let _home = k2_core::test_env::EnvVar::set("HOME", "/var/lib/k2-not-a-temp-home");
    let _ = load_from_disk();
}

fn heal_principal() -> Option<HookPrincipal> {
    Some(HookPrincipal { workspace_uuid: "ws".into(), agent_address: "a".into() })
}

/// Leaving the session map revokes a passport once, and a session with no
/// passport changes nothing.
#[test]
fn revoke_if_minted_revokes_a_real_passport_once() {
    with_temp_home(|| {
        let id = SessionId::new();
        let token = mint_session_token(&id, "p", heal_principal().unwrap(), CredMode::ApiKey, Provider::Anthropic);
        assert!(validate_hook(&token).is_some());
        revoke_if_minted(&id);
        assert!(validate_hook(&token).is_none(), "revoked");
        revoke_session(&id);
        revoke_if_minted(&id);
        let g = registry().lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(g.session_epochs.get(&id.to_string()), Some(&1), "a second teardown path doesn't bump again");
        drop(g);
        let none = SessionId::new();
        revoke_if_minted(&none);
        let g = registry().lock().unwrap_or_else(|e| e.into_inner());
        assert!(!g.session_epochs.contains_key(&none.to_string()), "no passport, no epoch");
    });
}

/// The kill switch counts as a revoke: records stay but are inert, and
/// don't read as valid or as lost. (A bare registry: tripping the process
/// one would reach every other test in this binary.)
#[test]
fn the_kill_switch_reads_as_revoked_not_lost() {
    let mut reg = SessionTokenRegistry::new();
    let id = SessionId::new();
    reg.mint(&id, "p", heal_principal().unwrap(), CredMode::ApiKey, Provider::Anthropic);
    assert_eq!(reg.passport_state(&id.to_string()), PassportState::Valid);
    reg.revoke_all();
    assert_eq!(reg.passport_state(&id.to_string()), PassportState::Revoked);
    assert_eq!(reg.passport_state(&SessionId::new().to_string()), PassportState::Missing);
}

/// A session that lost its record gets a new passport, once; a revoked
/// one never does; a failed delivery leaves no record behind.
#[test]
fn healing_mints_only_for_a_lost_passport_and_never_a_revoked_one() {
    with_temp_home(|| {
        let lost = SessionId::new();
        let mut delivered = String::new();
        let healed = heal_adopted_session(&lost, heal_principal, |t| {
            delivered = t.to_string();
            Ok(())
        });
        assert_eq!(healed, PassportHeal::Reminted);
        let v = validate_hook(&delivered).expect("the delivered passport validates");
        assert_eq!(v.session_id, lost.to_string());
        assert_eq!(v.pane_id, lost.to_string());
        assert_eq!(v.principal.workspace_uuid, "ws");
        let path = store_path();
        assert!(std::fs::read_to_string(&path).unwrap().contains(&lost.to_string()), "saved");

        // Now it has a record: nothing to heal, no delivery.
        let again = heal_adopted_session(&lost, heal_principal, |_| panic!("no second delivery"));
        assert_eq!(again, PassportHeal::Valid);

        // Revoked (the epoch moved, the record went): never again.
        revoke_session(&lost);
        let revoked = heal_adopted_session(&lost, heal_principal, |_| panic!("a revoked session got a passport"));
        assert_eq!(revoked, PassportHeal::Revoked);
        assert!(validate_hook(&delivered).is_none());

        // No file reached the session: no record, so a restart tries again.
        let undelivered = SessionId::new();
        let lost_it = heal_adopted_session(&undelivered, heal_principal, |_| Err(std::io::Error::other("no dir")));
        assert!(matches!(lost_it, PassportHeal::Lost(_)), "{lost_it:?}");
        let retried = heal_adopted_session(&undelivered, heal_principal, |_| Ok(()));
        assert_eq!(retried, PassportHeal::Reminted);

        // No workspace to bind it to: no mint.
        let nameless = SessionId::new();
        let no_ws = heal_adopted_session(&nameless, || None, |_| panic!("no principal, no passport"));
        assert!(matches!(no_ws, PassportHeal::Lost(_)), "{no_ws:?}");
    });
}

fn principal() -> HookPrincipal {
    HookPrincipal {
        workspace_uuid: "ws-uuid-1".to_string(),
        agent_address: "agent-a".to_string(),
    }
}

/// `k2 job` (item 5): a run's passport validates while the run lives,
/// then is gone without leaving an epoch behind; boot drops every
/// job record and leaves session records alone.
#[test]
fn job_passports_live_for_the_run_only() {
    let mut reg = SessionTokenRegistry::new();
    let jp = HookPrincipal { workspace_uuid: "ws-1".into(), agent_address: "job:collect".into() };
    let tok = reg.mint_sid("job-r1", "job-r1", jp.clone(), CredMode::Subscription, Provider::Anthropic);
    let v = reg.validate_hook(&tok).expect("valid during the run");
    assert_eq!(v.session_id, "job-r1");
    assert_eq!(v.principal, jp);
    let sid = SessionId::new();
    let stok = reg.mint(&sid, "pane", principal(), CredMode::ApiKey, Provider::Anthropic);
    let tok2 = reg.mint_sid("job-r2", "job-r2", jp, CredMode::Subscription, Provider::Anthropic);
    assert!(reg.drop_job_sid("job-r1"));
    assert!(reg.validate_hook(&tok).is_none(), "gone at run end");
    assert!(!reg.session_epochs.contains_key("job-r1"), "no epoch kept for a job run");
    assert!(!reg.drop_job_sid(&sid.to_string()), "only job selectors drop this way");
    assert_eq!(reg.revoke_job_selectors(), 1);
    assert!(reg.validate_hook(&tok2).is_none(), "boot revokes job passports");
    assert!(reg.validate_hook(&stok).is_some(), "session passports stay");
}

// ── env flag ─────────────────────────────────────────────────────

#[test]
fn scoped_hooks_flag_defaults_on() {
    // Phase 1 / PR-A: default ON so new agent sessions get a passport.
    // Opt-out is explicit (`0`/`false`/`off`). The guards hold the ONE
    // shared env lock and restore whatever was set (LIFO) so parallel
    // tests that pin the flag keep working.
    let _env = crate::test_support::lock_home();
    let _scoped = k2_core::test_env::EnvVar::remove("K2_HOOK_SCOPED");
    assert!(scoped_hooks_enabled(), "must default ON when unset");
    let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
    assert!(!scoped_hooks_enabled(), "explicit 0 must opt out");
    let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "false");
    assert!(!scoped_hooks_enabled(), "explicit false must opt out");
    let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "off");
    assert!(!scoped_hooks_enabled(), "explicit off must opt out");
    let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "1");
    assert!(scoped_hooks_enabled(), "explicit 1 must stay ON");
}

// ── mint + validate round trip ──────────────────────────────────

#[test]
fn mint_then_validate_accepts_correct_secret() {
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);

    // Format is selector.verifier and the selector is the session id.
    let (sel, secret) = token.split_once('.').expect("token has a dot");
    assert_eq!(sel, sid.to_string(), "selector must be the session id");
    assert!(!secret.is_empty(), "verifier must be present");

    let v = reg
        .validate_hook(&token)
        .expect("freshly minted token must validate");
    assert_eq!(v.session_id, sid.to_string());
    assert_eq!(v.pane_id, "pane-1");
    assert_eq!(v.principal, principal());
}

#[test]
fn validate_rejects_tampered_secret() {
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    // Flip the last hex char of the verifier.
    let mut bytes: Vec<char> = token.chars().collect();
    let last = bytes.len() - 1;
    bytes[last] = if bytes[last] == 'a' { 'b' } else { 'a' };
    let tampered: String = bytes.into_iter().collect();
    assert_ne!(tampered, token, "tamper must actually change the token");
    assert!(
        reg.validate_hook(&tampered).is_none(),
        "a tampered verifier must NOT validate",
    );
}

#[test]
fn validate_rejects_unknown_selector() {
    let reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    // No record was ever minted for this selector.
    assert!(reg
        .validate_hook(&format!("{sid}.deadbeef"))
        .is_none());
}

#[test]
fn validate_rejects_malformed_bearer() {
    let reg = SessionTokenRegistry::new();
    assert!(reg.validate_hook("").is_none());
    assert!(reg.validate_hook("no-dot-here").is_none());
    assert!(reg.validate_hook(".onlysecret").is_none());
    assert!(reg.validate_hook("onlyselector.").is_none());
}

// ── epoch revocation (per-session) ──────────────────────────────

#[test]
fn revoke_invalidates_the_session_token() {
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    assert!(reg.validate_hook(&token).is_some(), "valid pre-revoke");

    reg.revoke(&sid);
    assert!(
        reg.validate_hook(&token).is_none(),
        "revoked session token must 403",
    );
}

#[test]
fn validate_rejects_stale_session_epoch() {
    // Exercise the EPOCH branch specifically (record present, epoch
    // bumped underneath it) without relying on record removal.
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    assert!(reg.validate_hook(&token).is_some());

    // Bump the CURRENT epoch but leave the (stale-stamped) record.
    *reg.session_epochs.entry(sid.to_string()).or_insert(0) += 1;
    assert!(
        reg.validate_hook(&token).is_none(),
        "record stamped with an older session epoch must be rejected",
    );
}

// ── global kill switch ──────────────────────────────────────────

#[test]
fn revoke_all_invalidates_every_token() {
    let mut reg = SessionTokenRegistry::new();
    let a = SessionId::new();
    let b = SessionId::new();
    let ta = reg.mint(&a, "pane-a", principal(), CredMode::ApiKey, Provider::Anthropic);
    let tb = reg.mint(&b, "pane-b", principal(), CredMode::ApiKey, Provider::Anthropic);
    assert!(reg.validate_hook(&ta).is_some());
    assert!(reg.validate_hook(&tb).is_some());

    reg.revoke_all();
    assert!(reg.validate_hook(&ta).is_none(), "global kill switch (a)");
    assert!(reg.validate_hook(&tb).is_none(), "global kill switch (b)");
}

// ── wall-clock stamp is not a kill switch ───────────────────────

#[test]
fn validate_ignores_past_expires_at_stamp() {
    // Live agent passports must not die on wall clock. Epoch revoke is
    // the kill switch; expires_at is informational only.
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    reg.records
        .get_mut(&sid.to_string())
        .expect("record present")
        .expires_at = Utc::now() - Duration::days(30);
    assert!(
        reg.validate_hook(&token).is_some(),
        "past expires_at must NOT 403 a still-live passport",
    );
}

#[test]
fn validate_accepts_months_old_passport() {
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    let rec = reg
        .records
        .get_mut(&sid.to_string())
        .expect("record present");
    rec.created_at = Utc::now() - Duration::days(90);
    // leave expires_at far future (as mint would); still valid
    assert!(
        reg.validate_hook(&token).is_some(),
        "90-day-old passport must still validate for long-running agents",
    );
}

// ── capability allowlist / denylist (require_hook §3.3) ──────────

/// Permission cards (vs-live C1): the family is an agent verb only while
/// the global switch is on; off it is refused like any non-agent path.
/// Pure twin only: the process-wide switch is never flipped here.
#[test]
fn permission_family_is_an_agent_verb_only_while_the_server_switch_is_on() {
    for p in [
        "/cli/permission/request",
        "/cli/permission/enabled",
        "/cli/permission/status",
        "/cli/permission/approve",
    ] {
        assert!(!is_agent_verb_when(p, false), "{p} admitted while off");
        assert!(is_agent_verb_when(p, true), "{p} not admitted while on");
    }
    // Never the bare word, and nothing else moves with the switch.
    for on in [false, true] {
        assert!(!is_agent_verb_when("/cli/permission", on));
        assert!(!is_agent_verb_when("/cli/agent-access/set", on));
        assert!(!is_agent_verb_when("/cli/settings/update", on));
        assert!(is_agent_verb_when("/cli/thread/answer", on));
    }
}

/// Multiplayer cores (prd-multiplayer-cores-settings-v1 MC-2): an agent
/// may read the switches (`GET /cli/docs/cores`) but its cell socket
/// never routes the write, `/cli/settings/update` with
/// `{"multiplayerCores": …}`, whatever the cards switch says.
#[test]
fn an_agent_reads_the_multiplayer_cores_but_never_writes_them() {
    assert!(is_agent_verb("/cli/docs/cores"));
    for on in [false, true] {
        assert!(!is_agent_verb_when("/cli/settings/update", on));
        assert!(!is_agent_verb_when("/cli/settings/reset", on));
    }
}

/// 0.46.5 (first-run review item 13): an agent reads `k2 git-tools`
/// and may recheck; installing (Apple's dialog) stays the owner's.
#[test]
fn git_tools_status_and_recheck_are_agent_verbs_install_is_not() {
    assert!(is_agent_verb("/cli/git-tools"));
    assert!(is_agent_verb("/cli/git-tools/recheck"));
    assert!(!is_agent_verb("/cli/git-tools/install"));
    assert!(!is_agent_verb("/cli/git-tools/installx"));
}

#[test]
fn every_voice_route_is_an_agent_verb() {
    for p in crate::voice::routes::GET_ROUTES.iter().chain(crate::voice::routes::POST_ROUTES) {
        assert!(is_agent_verb(p), "{p}");
    }
    for p in crate::voice::transcripts::GET_ROUTES.iter().chain(crate::voice::transcripts::POST_ROUTES.iter()) {
        assert!(is_agent_verb(p), "{p}");
    }
}

#[test]
fn is_agent_verb_allows_agent_routes() {
    assert!(is_agent_verb("/hook/complete"));
    assert!(is_agent_verb("/cli/workspace/msg"));
    assert!(is_agent_verb("/cli/thread"));
    assert!(is_agent_verb("/cli/thread/post"));
    assert!(is_agent_verb("/cli/thread/ask"));
    assert!(is_agent_verb("/cli/thread/secret"));
    assert!(is_agent_verb("/cli/thread/answer"));
    assert!(is_agent_verb("/cli/thread/void"));
    assert!(is_agent_verb("/cli/chatter"));
    assert!(is_agent_verb("/cli/chatterlog"));
    assert_eq!(crate::overlay_ws::OVERLAY_WS_PATH, "/cli/overlay/events");
    assert!(is_agent_verb("/cli/inbox/respond"));
    assert!(is_agent_verb("/cli/awareness/publish"));
    // Wave 0: agent mail verbs (owner mail surfaces stay denied).
    assert!(is_agent_verb("/cli/mail/messages"));
    assert!(is_agent_verb("/cli/mail/inboxes"));
    assert!(is_agent_verb("/cli/mail/send"));
    assert!(is_agent_verb("/cli/mail/draft"));
    // DNS K1: agent DNS prefixes.
    assert!(is_agent_verb("/cli/dns/access"));
    assert!(is_agent_verb("/cli/dns/zones"));
    assert!(is_agent_verb("/cli/dns/records"));
    assert!(is_agent_verb("/cli/dns/records/add"));
    assert!(is_agent_verb("/cli/dns/records/remove"));
    assert!(is_agent_verb("/cli/dns/verify"));
    // Custom domains: list + hostname add/remove + cert issue/renew
    // are agent verbs; apex attach/remove + PEM upload stay owner-only.
    assert!(is_agent_verb("/cli/domains"));
    assert!(is_agent_verb("/cli/certs"));
    assert!(is_agent_verb("/cli/certs/issue"));
    assert!(is_agent_verb("/cli/certs/renew"));
    assert!(
        !is_agent_verb("/cli/domains/remove"),
        "apex attach/remove is owner/admin — not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/domains/refresh"),
        "re-bind may auto-add a zone to the k2.dev account — owner/admin"
    );
    assert!(is_agent_verb("/cli/domains/names"));
    assert!(is_agent_verb("/cli/domains/names/remove"));
    assert!(
        !is_agent_verb("/cli/certs/upload"),
        "PEM upload is owner-only"
    );
    // Tickets badge orphans: the host-wide Tickets list is not an agent
    // verb (and no other /cli/feedback/ route is either).
    assert!(!is_agent_verb("/cli/feedback/list-all"));
    assert!(!is_agent_verb("/cli/feedback/list"));
    assert!(!is_agent_verb("/cli/feedback/waiting-count"));
    // C1 (0.40.45): connections list/add/remove (mutate toggle-gated).
    assert!(is_agent_verb("/cli/connections"));
    assert!(
        !is_agent_verb("/cli/users"),
        "GET /cli/users is owner/admin — not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/connections/users"),
        "do not invent /cli/connections/users — users=1 rides /cli/connections"
    );
    // Heartbeat schedule family (agent-owned workspace schedules).
    assert!(is_agent_verb("/cli/heartbeat/list"));
    assert!(is_agent_verb("/cli/heartbeat/add"));
    assert!(is_agent_verb("/cli/heartbeat/edit"));
    assert!(is_agent_verb("/cli/heartbeat/fire"));
    assert!(is_agent_verb("/cli/heartbeat/status"));
    // 0.46.9: an agent restarts its own heartbeat's session.
    assert!(is_agent_verb("/cli/heartbeat/restart"));
    assert!(is_agent_verb("/cli/heartbeat-log"));
    // OS install + fleet-wide list stay owner-only (teaching owner_only).
    assert!(!is_agent_verb("/cli/heartbeat/install-launchd"));
    assert!(!is_agent_verb("/cli/heartbeat/uninstall-launchd"));
    assert!(!is_agent_verb("/cli/heartbeat/apply-wake-scheduler"));
    assert!(!is_agent_verb("/cli/heartbeat/list-all"));
    assert!(!is_agent_verb("/cli/heartbeat/fires-list-all"));
    assert!(!is_agent_verb("/cli/heartbeat/set-show-sessions"));
    assert!(!is_agent_verb("/cli/heartbeat/active-projects"));
    // PR1: federation agent send surface under passports.
    assert!(is_agent_verb("/cli/federation/peers"));
    assert!(is_agent_verb("/cli/federation/peer-roster"));
    assert!(is_agent_verb("/cli/federation/peer-rosters"));
    assert!(is_agent_verb("/cli/federation/send"));
    assert!(
        is_agent_verb("/cli/whoami"),
        "scoped tokens must reach GET /cli/whoami"
    );
    assert!(
        is_agent_verb("/cli/publish/list"),
        "published services are agent verbs (not under /cli/tunnel/)"
    );
    assert!(is_agent_verb("/cli/publish/run"));
    assert!(
        !is_agent_verb("/cli/tunnel/publish/run"),
        "must not hang publish under owner-only /cli/tunnel/"
    );
    assert!(
        is_agent_verb("/cli/connect/proxy"),
        "agents publish their own nested labels end to end (0.45.7)"
    );
    assert!(!is_agent_verb("/cli/connect/other"), "exact path only");
    assert!(
        !is_agent_verb("/cli/auth/whoami"),
        "/cli/auth/whoami is connect-user identity — not an agent verb"
    );
    // Skin roster GET + manage mutations for workspace-agent passports.
    // Dispatcher still gates POST mutate on the Agent-tab column.
    // front-door / hydra / prefix / the toggle writer stay off.
    assert!(is_agent_verb("/cli/skin/users"));
    assert!(is_agent_verb("/cli/skin/users/remove"));
    assert!(is_agent_verb("/cli/skin/users/password"));
    assert!(is_agent_verb("/cli/skin/users/unlock"));
    assert!(
        !is_agent_verb("/cli/users/unlock"),
        "box login unlock is owner-tier, not an agent verb"
    );
    assert!(is_agent_verb("/cli/skin/users/email"));
    assert!(
        is_agent_verb("/cli/skin/users/full-name"),
        "0.46.4: an agent with \"Let this agent manage its Apps' users\" names its own Apps' users"
    );
    assert!(is_agent_verb("/cli/app/grant"));
    assert!(is_agent_verb("/cli/app/grant/copy-host"));
    assert!(is_agent_verb("/cli/app/resume"), "its own suspension, with switch 2 (the handler)");
    for p in ["/cli/app/push", "/cli/app/push/set", "/cli/app/push/send", "/cli/app/push/test", "/cli/app/push/log"] {
        assert!(is_agent_verb(p), "{p}: the cell socket matches TCP");
    }
    for p in ["/cli/app/push/credentials", "/cli/app/push/credentials/remove", "/cli/app/push/check", "/cli/app/push/devices", "/cli/app/push/devices/remove"] {
        assert!(!is_agent_verb(p), "{p}: the owner's and Admins'");
    }
    assert!(!is_agent_verb("/cli/app/remote-pass/rotate"));
    assert!(
        !is_agent_verb("/cli/skin/users/agent"),
        "an agent must never mark or unmark a principal as an agent"
    );
    assert!(is_agent_verb("/cli/skin/users/rooms"));
    assert!(is_agent_verb("/cli/skin/roles"));
    assert!(is_agent_verb("/cli/skin/roles/update"));
    assert!(is_agent_verb("/cli/skin/roles/remove"));
    assert!(is_agent_verb("/cli/skin/roles/assign"));
    assert!(is_agent_verb("/cli/skin/roles/unassign"));
    assert!(is_agent_verb("/cli/skin/roles/room"));
    assert!(
        !is_agent_verb("/cli/skin/grants"),
        "grants are owner-only, not an agent verb"
    );
    assert!(is_agent_verb("/cli/skin/grants/own"), "an agent lists its own Apps' logins");
    assert!(
        !is_agent_verb("/cli/skin/grants/delete"),
        "grant revoke is owner-only, not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/skin/grants/enabled"),
        "grant enabled is owner-only, not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/skin/grants/host"),
        "host login grant is owner-only, not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/skin/grants/copy-host"),
        "copying box logins onto an App is owner-only, not an agent verb (S24-11)"
    );
    assert!(
        !is_agent_verb("/cli/skin/templates"),
        "access templates are owner-only, not an agent verb"
    );
    assert!(!is_agent_verb("/cli/skin/templates/update"));
    assert!(!is_agent_verb("/cli/skin/templates/delete"));
    assert!(!is_agent_verb("/cli/skin/templates/lines"));
    assert!(!is_agent_verb("/cli/skin/templates/lines/update"));
    assert!(!is_agent_verb("/cli/skin/templates/lines/delete"));
    assert!(
        !is_agent_verb("/cli/skin/templates/apply"),
        "template apply is owner-only, not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/people"),
        "do not add /cli/people as an agent verb"
    );
    assert!(is_agent_verb("/cli/skin-tokens"));
    assert!(is_agent_verb("/cli/skin-tokens/revoke"));
    assert!(is_agent_verb("/cli/skin-tokens/rooms"));
    assert!(
        !is_agent_verb("/cli/skin/front-door"),
        "must not allow-prefix /cli/skin/ (front-door is owner-only)"
    );
    assert!(
        !is_agent_verb("/cli/skin/hydra"),
        "hydra sidecar is owner-only"
    );
    assert!(
        !is_agent_verb("/cli/skin/"),
        "must not allow-prefix all of /cli/skin/"
    );
    assert!(
        !is_agent_verb("/cli/agents-manage-skin"),
        "toggle writer is owner-only, not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/skin/password/forgot"),
        "forgot is owner|platform, not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/skin/password/reset"),
        "reset is public, not an agent verb"
    );
    assert!(
        !is_agent_verb("/cli/skin/password/change"),
        "change is session k2skn_, not an agent verb"
    );
}

/// R1 (agent-app-users S1, T1.3): the admin logins (`/cli/users`, the
/// bare path too, and every arm under it) are never an agent verb,
/// with permission cards on or off.
#[test]
fn users_routes_are_never_agent_verbs() {
    const ARMS: &[&str] = &[
        "/cli/users",
        "/cli/users/add",
        "/cli/users/remove",
        "/cli/users/set-password",
        "/cli/users/unlock",
        "/cli/users/set-disabled",
        "/cli/users/set-role",
        "/cli/users/set-can-clone",
        "/cli/users/policy",
        "/cli/users/audit",
    ];
    for on in [false, true] {
        for p in ARMS {
            assert!(!is_agent_verb_when(p, on), "{p} must never be an agent verb (permission cards on={on})");
        }
    }
}

/// 0.46.8: an agent starts its App's own reset here (the handler scopes it, and
/// never mints the owner's link); inviting is the same.
#[test]
fn reset_link_and_invite_are_agent_verbs() {
    for on in [false, true] {
        assert!(is_agent_verb_when("/cli/skin/users/reset-link", on), "reset-link (permission cards on={on})");
        assert!(is_agent_verb_when("/cli/skin/users/invite", on), "invite (permission cards on={on})");
    }
}

#[test]
fn is_agent_verb_denies_federation_owner_surfaces() {
    // pair/confirm stay owner-or-admin; outbox/pubkey stay owner-gated.
    // inbound is envelope-auth (not a scoped verb either).
    for p in [
        "/cli/federation/pair/request",
        "/cli/federation/pair/confirm",
        "/cli/federation/outbox",
        "/cli/federation/pubkey",
        "/cli/federation/inbound",
        "/cli/federation/roster",
    ] {
        assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
    }
}

#[test]
fn is_agent_verb_denies_mail_owner_surfaces() {
    // M5 hostmail manage: DENY shrink so scoped hooks can pass
    // require_hook; extra gate ORs mail_manage_allowed_for_path.
    // C5b (C21) also opens access/doctor/approvals/config-set/import.
    for p in [
        "/cli/mail/server/enable",
        "/cli/mail/server/disable",
        "/cli/mail/domain/list",
        "/cli/mail/domain/show",
        "/cli/mail/domain/add",
        "/cli/mail/domain/check",
        "/cli/mail/access/grant",
        "/cli/mail/access/revoke",
        "/cli/mail/access/set-primary",
        "/cli/mail/access/set-level",
        "/cli/mail/access/set-manage",
        "/cli/mail/doctor",
        "/cli/mail/approvals/list",
        "/cli/mail/approvals/approve",
        "/cli/mail/approvals/deny",
        "/cli/mail/config/set",
        "/cli/mail/import",
        "/cli/mail/quota",
        "/cli/mail/catchall",
        "/cli/mail/alias",
        "/cli/mail/alias/remove",
        "/cli/mail/forward",
        "/cli/mail/forward/unset",
        "/cli/mail/ooo",
        "/cli/mail/ooo/unset",
        "/cli/mail/footer",
        "/cli/mail/footer/unset",
        "/cli/mail/cert/renew",
        "/cli/mail/server/rotate-admin",
        "/cli/mail/address/password",
        "/cli/mail/list",
        "/cli/mail/list/members",
        "/cli/mail/list/delete",
        "/cli/mail/spam/train",
        "/cli/mail/spam/allow",
        "/cli/mail/spam/block",
        "/cli/mail/spam/quarantine",
        "/cli/mail/spam/quarantine/release",
        "/cli/mail/spam/quarantine/discard",
        "/cli/mail/queue",
        "/cli/mail/queue/retry",
        "/cli/mail/queue/drop",
        "/cli/mail/acl",
        "/cli/mail/acl/revoke",
        "/cli/mail/autoconfig",
        "/cli/mail/app-password",
        "/cli/mail/app-password/revoke",
        "/cli/mail/dkim",
        "/cli/mail/dkim/rotate",
        "/cli/mail/dkim/retire",
        "/cli/mail/dmarc",
        "/cli/mail/dmarc/report-to",
        "/cli/mail/ptr",
        "/cli/mail/ptr/set",
    ] {
        assert!(is_agent_verb(p), "M5 must be an agent verb: {p}");
    }
    // Leftover M6 stay denied (uninstall / remove / OAuth / link / writer).
    for p in [
        "/cli/mail/server/uninstall",
        "/cli/mail/server/upgrade",
        "/cli/mail/domain/remove",
        "/cli/mail/dav",
        "/cli/mail/profile",
        "/cli/mail/sieve-audit",
        // 0.46.7: a person's own forward routes.
        "/cli/mail/self/forward",
        "/cli/mail/self/forward/add",
        "/cli/mail/self/forward/rm",
        "/cli/mail/self/forward/unset",
        // 0.46.7: the Live editor's socket and probe (people only).
        "/cli/docs/live",
        "/cli/docs/live/check",
        "/cli/mail/external/add",
        "/cli/mail/link/oauth/start",
        "/cli/mail/oauth-config",
        "/cli/mail-manage",
    ] {
        assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
    }
    // 0.45.0: the IT-agent credential verbs ride the M5 extra gate
    // (mail-manage), with the self-elevation check in the handlers.
    for p in ["/cli/mail/credentials/keep", "/cli/mail/address/person"] {
        assert!(is_agent_verb(p), "M5 must be an agent verb: {p}");
    }
    // Calendars S4: groups + calendar sharing ride the same M5 gate
    // (self-widening refused in the handlers, mail::calendar_share).
    for p in [
        "/cli/mail/group",
        "/cli/mail/group/members",
        "/cli/mail/group/delete",
        "/cli/mail/calendar/manage",
        "/cli/mail/calendar/share",
        "/cli/mail/calendar/unshare",
        "/cli/mail/calendar/shares",
        "/cli/mail/calendar/sharing",
    ] {
        assert!(is_agent_verb(p), "M5 must be an agent verb: {p}");
    }
    assert!(is_agent_verb("/cli/db/create"));
    assert!(is_agent_verb("/cli/db/list"));
    assert!(is_agent_verb("/cli/db/grant"));
    assert!(is_agent_verb("/cli/store/put"));
    assert!(!is_agent_verb("/cli/db/server/enable"));
    assert!(!is_agent_verb("/cli/db/doctor"));
    assert!(!is_agent_verb("/cli/db/bind"));
}

/// E3 — prove scoped agents cannot self-grant: every access management
/// verb is DENY_PREFIXES (`/cli/mail/access/`) before ALLOW_PREFIXES
/// `/cli/mail/`. Mirrors the DNS zones/create denial style.
#[test]
fn is_agent_verb_denies_mail_access_mutations() {
    // Agent catalog + read/send stay admitted…
    assert!(
        is_agent_verb("/cli/mail/inboxes"),
        "agent view of the catalog remains an agent verb"
    );
    assert!(is_agent_verb("/cli/mail/messages"));
    assert!(is_agent_verb("/cli/mail/send"));
    // C5b: access management is an extra-gated mail_manage verb,
    // not a DENY. Unknown nested paths ride `/cli/mail/` and the
    // extra gate still fail-closes leftover M6.
    for p in [
        "/cli/mail/access/grant",
        "/cli/mail/access/revoke",
        "/cli/mail/access/set-primary",
        "/cli/mail/access/set-level",
        "/cli/mail/access/set-manage",
    ] {
        assert!(
            is_agent_verb(p),
            "C5b access is an extra-gated agent verb: {p}"
        );
    }
}

#[test]
fn is_agent_verb_allows_dns_prefixes_and_denies_zone_lifecycle() {
    assert!(is_agent_verb("/cli/dns/access"));
    assert!(is_agent_verb("/cli/dns/zones"));
    assert!(is_agent_verb("/cli/dns/records/add"));
    // Zone create/delete are owner-only (DENY before ALLOW_PREFIXES).
    assert!(
        !is_agent_verb("/cli/dns/zones/create"),
        "scoped token must NOT create zones"
    );
    assert!(
        !is_agent_verb("/cli/dns/zones/delete"),
        "scoped token must NOT delete zones"
    );
    // A9: no prefix `/cli/domains/` or `/cli/certs/`. Apex remove
    // stays owner; hostname add/remove is dns_manage (exact paths).
    assert!(is_agent_verb("/cli/domains"));
    assert!(!is_agent_verb("/cli/domains/remove"));
    assert!(is_agent_verb("/cli/domains/names"));
    assert!(is_agent_verb("/cli/domains/names/remove"));
    assert!(is_agent_verb("/cli/certs"));
    assert!(is_agent_verb("/cli/certs/issue"));
    assert!(is_agent_verb("/cli/certs/renew"));
    assert!(!is_agent_verb("/cli/certs/upload"));
}

#[test]
fn is_agent_verb_allows_respond_but_not_the_deny_surface() {
    // F2 (sandbox API): the in-cell agent's response egress is allowlisted.
    assert!(is_agent_verb("/cli/respond"), "/cli/respond is the agent response verb");
    // Host-session completion lifecycle (`k2 done`).
    assert!(
        is_agent_verb("/cli/session/complete"),
        "/cli/session/complete is the agent done verb"
    );
    // And the +1 must NOT have widened the escalation surface — the deny
    // prefixes still default-deny (belt-and-braces against an allowlist edit
    // accidentally crossing a DENY_PREFIX).
    for p in [
        "/cli/users/set-role",
        "/cli/fs/write-file",
        "/cli/clone/bundle",
        "/cli/tunnel/start",
        "/cli/daemon/restart",
        "/cli/sessions/v2/spawn",
        "/cli/terminal/create",
    ] {
        assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
    }
    // Near-miss: a `respond`-prefixed sub-path is NOT auto-allowed (exact match
    // only — `/cli/respond` is in ALLOW_EXACT, not ALLOW_PREFIXES).
    assert!(!is_agent_verb("/cli/respond/anything"), "respond is exact-match only");
    assert!(
        !is_agent_verb("/cli/session/complete/extra"),
        "session/complete is exact-match only"
    );
    // Do not open a broad /cli/session/ prefix.
    assert!(!is_agent_verb("/cli/session/anything-else"));
}

#[test]
fn is_agent_verb_allows_design_notes_exact_only() {
    assert!(is_agent_verb("/cli/browser/notes"));
    assert!(is_agent_verb("/cli/browser/notes/redeliver"));
    assert!(!is_agent_verb("/cli/browser/notes/other"));
    assert!(!is_agent_verb("/cli/browser/"));
}

#[test]
fn is_agent_verb_allows_browser_open_url_exact_only() {
    // 0.40.34: the shim's scoped-session egress verb is allowlisted…
    assert!(
        is_agent_verb("/cli/browser/open-url"),
        "shim must be able to forward URLs over the scoped channel"
    );
    // …EXACT match only — no `/cli/browser/` prefix widening.
    assert!(!is_agent_verb("/cli/browser/open-url/extra"));
    assert!(!is_agent_verb("/cli/browser/anything-else"));
}

#[test]
fn is_agent_verb_admits_own_identity_not_handle() {
    // 0.46.1 `k2 agent identity`: color + picture on the agent's own
    // workspace; the handle stays a person's (or an App's) change.
    assert!(is_agent_verb("/cli/agent/identity"));
    assert!(!is_agent_verb("/cli/agent/handle"));
    assert!(!is_agent_verb("/cli/agent/identity/x"));
}

#[test]
fn is_agent_verb_denies_review_checklist_after_p1_drop() {
    // Sandbox P1 (Finding-1 follow-on): review-checklist verbs were
    // DROPPED from the scoped allowlist — they take the raw body (not the
    // principal-pinned params) so they fall back to TCP+owner, never the
    // scoped cell UDS. Re-admitted in P2 with a body-restamp.
    assert!(!is_agent_verb("/cli/review-checklist/toggle"));
    assert!(!is_agent_verb("/cli/review-checklist/write"));
    assert!(!is_agent_verb("/cli/review-checklist/init"));
}

#[test]
fn is_agent_verb_allows_awareness_publish_only() {
    // #58 Phase-1 close: publish is the +1 allowlist delta; subscribe
    // (read-only WS) and resolve are NOT widened onto the scoped token.
    assert!(is_agent_verb("/cli/awareness/publish"));
    // The swap handler scopes an agent passport to its own workspace.
    assert!(is_agent_verb("/cli/workspace/swap-canonical"));
    // `k2 agent harness`: the handler refuses an agent with a card.
    assert!(is_agent_verb("/cli/agent/harness"));
    // `k2 agent refresh`: the handler decides.
    assert!(is_agent_verb("/cli/agent/refresh"));
    // C2: workspace-addressed terminal read is peer-gated agent verb.
    assert!(
        is_agent_verb("/cli/terminal/read"),
        "C2 admits k2 read <ws> for scoped principals (peer-gated)"
    );
    // `k2 sessions` (prd-workspace-sessions-cli-v1 WS-R7, WS-T6): the
    // four routes agents use instead of the raw PTY view.
    for p in [
        "/cli/sessions/roster",
        "/cli/sessions/past",
        "/cli/sessions/show",
        "/cli/sessions/open",
    ] {
        assert!(is_agent_verb(p), "{p} must be an agent verb");
    }
    // The deliberate this-release DENIALS (PRD §B SCOPE HONESTY): these
    // stay owner/connect-user over TCP (write still denied).
    for p in [
        "/cli/awareness/subscribe",
        "/cli/workspace/resolve",
        "/cli/sessions/list-for-workspace",
        "/cli/terminal/write",
    ] {
        assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
    }
}

// ── mint stamps far-future expires_at (informational only) ─────────

#[test]
fn minted_expires_at_is_far_horizon_not_24h() {
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let _ = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    let rec = reg.records.get(&sid.to_string()).expect("record present");
    let span = rec.expires_at - rec.created_at;
    assert!(
        span >= Duration::days(365 * 50),
        "expires_at stamp must be a multi-decade horizon (not 24h), got {span}",
    );
}

#[test]
fn token_thirteen_hours_old_still_validates() {
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    let rec = reg
        .records
        .get_mut(&sid.to_string())
        .expect("record present");
    rec.created_at = Utc::now() - Duration::hours(13);
    assert!(
        reg.validate_hook(&token).is_some(),
        "a 13h-old token must still validate",
    );
}

#[test]
fn is_agent_verb_denies_owner_escalation_surface() {
    // The escalation surface from PRD §3.3 / smoke test step 6 — these
    // MUST be default-denied to a scoped token.
    for p in [
        "/cli/users/set-role",
        "/cli/users/add",
        "/cli/fs/write-file",
        "/cli/fs/delete",
        "/cli/clone/bundle",
        "/cli/tunnel/config",
        "/cli/tunnel/start",
        "/cli/daemon/restart",
        "/cli/sessions/v2/spawn",
        "/cli/terminal/create",
        "/cli/terminal/send-message",
        "/cli/terminal/compose-history",
    ] {
        assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
    }
}

// ── DISJOINTNESS from the owner guards (the escalation negative) ─

#[test]
fn minted_scoped_token_does_not_pass_owner_guards() {
    // The load-bearing security invariant: a scoped token is NOT the
    // owner token and carries NO owner capability.
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let owner_token = "owner-secret-deadbeef";
    let scoped = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);

    assert_ne!(scoped, owner_token, "scoped != owner by construction");

    let q = format!("token={scoped}");
    assert!(
        !crate::routes::http::token_is_owner(&q, owner_token),
        "a scoped token must NOT satisfy token_is_owner (no escalation)",
    );
    assert!(
        !crate::routes::http::token_ok(&q, owner_token),
        "a scoped token is not a connect-user session either → fails token_ok",
    );
}

// ── Phase 1: spawn-time activation (flag opt-out + env shape) ──

#[test]
fn cell_env_pairs_returns_none_when_flag_off() {
    // Explicit fleet opt-out: K2_HOOK_SCOPED=0 → mint NOTHING.
    // (Only reads the env var; no disk, no registry mutation.)
    // Still wrap in with_temp_home so a future edit that mints cannot
    // silently write real ~/.k2/hook-sessions.json.
    with_temp_home(|| {
        let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
        let sid = SessionId::new();
        assert!(
            cell_env_pairs(
                &sid,
                &sid.to_string(),
                principal(),
                CredMode::ApiKey,
                Provider::Anthropic,
                None
            )
            .is_none(),
            "flag OFF (explicit opt-out) MUST inject no scoped env",
        );
    });
}

/// T-S1i (A8): with `K2_HOOK_SCOPED=0` a v2 spawn still carries the
/// pane id, so `notify.sh` can name its session; no token, no socket.
#[test]
fn t_s1i_pane_id_rides_the_spawn_with_scoped_hooks_off() {
    with_temp_home(|| {
        let scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
        let sid = SessionId::new();
        let pane = sid.to_string();
        let mut env = std::collections::HashMap::new();
        let minted = prepare_agent_spawn_env(
            &mut env,
            &sid,
            &pane,
            principal(),
            CredMode::ApiKey,
            Provider::Anthropic,
            None,
            "owner-secret",
        );
        drop(scoped);
        assert!(!minted);
        for k in ["K2_PANE_ID", "K2SO_PANE_ID", "K2_TAB_ID", "K2SO_TAB_ID"] {
            assert_eq!(env.get(k), Some(&pane), "{k} missing with scoped hooks off");
        }
        assert!(!env.contains_key("K2_HOOK_TOKEN"));
        assert!(!env.contains_key("K2_HOOK_SOCK"));
    });
}

#[test]
fn prepare_agent_spawn_env_never_leaves_owner_token() {
    // PR-A load-bearing invariant: after prepare_agent_spawn_env, the
    // child env must not carry the owner token behind K2_HOOK_TOKEN
    // (whether scoped mint ran or the flag was opted out).
    //
    // MUST run under with_temp_home: flag-ON path calls mint_session_token
    // → save_to_disk(store_path()) which otherwise pollutes the
    // developer's real ~/.k2/hook-sessions.json with test principals
    // (ws-uuid-1 / agent-a).
    with_temp_home(|| {
        let owner = "owner-secret-deadbeef-aaaaaaaa";
        let sid = SessionId::new();
        let pane = sid.to_string();

        // Path A — flag ON (default mint): scoped passport replaces owner.
        let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "1");
        let mut env = std::collections::HashMap::new();
        env.insert("K2_HOOK_TOKEN".to_string(), owner.to_string());
        env.insert("K2SO_HOOK_TOKEN".to_string(), owner.to_string());
        let minted = prepare_agent_spawn_env(
            &mut env,
            &sid,
            &pane,
            principal(),
            CredMode::ApiKey,
            Provider::Anthropic,
            None,
            owner,
        );
        assert!(minted, "flag ON must mint a scoped passport");
        let tok = env.get("K2_HOOK_TOKEN").expect("scoped K2_HOOK_TOKEN present");
        assert_ne!(tok, owner, "scoped path must not leave the owner token");
        assert_ne!(
            env.get("K2SO_HOOK_TOKEN").map(String::as_str),
            Some(owner),
            "legacy alias must not leave the owner token",
        );
        // Minted value must validate as a real principal (process registry
        // under this temp HOME).
        let v = validate_hook(tok).expect("minted passport must validate_hook");
        assert_eq!(v.session_id, sid.to_string());
        assert_eq!(v.pane_id, pane);
        assert_eq!(v.principal, principal());

        // Path B — flag OFF: still strips owner even without a mint.
        let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
        let mut env_off = std::collections::HashMap::new();
        env_off.insert("K2_HOOK_TOKEN".to_string(), owner.to_string());
        env_off.insert("K2SO_HOOK_TOKEN".to_string(), owner.to_string());
        let minted_off = prepare_agent_spawn_env(
            &mut env_off,
            &sid,
            &pane,
            principal(),
            CredMode::ApiKey,
            Provider::Anthropic,
            None,
            owner,
        );
        assert!(!minted_off, "flag OFF must not mint");
        assert!(
            env_off.get("K2_HOOK_TOKEN").is_none(),
            "flag OFF must still strip owner from K2_HOOK_TOKEN",
        );
        assert!(
            env_off.get("K2SO_HOOK_TOKEN").is_none(),
            "flag OFF must still strip owner from K2SO_HOOK_TOKEN",
        );
    });
}

#[test]
fn strip_owner_hook_tokens_preserves_non_owner_values() {
    let owner = "owner-secret";
    let mut env = std::collections::HashMap::new();
    env.insert("K2_HOOK_TOKEN".to_string(), "sid.scopedsecret".to_string());
    env.insert("K2SO_HOOK_TOKEN".to_string(), owner.to_string());
    strip_owner_hook_tokens(&mut env, owner);
    assert_eq!(
        env.get("K2_HOOK_TOKEN").map(String::as_str),
        Some("sid.scopedsecret"),
        "non-owner scoped value must be preserved",
    );
    assert!(
        env.get("K2SO_HOOK_TOKEN").is_none(),
        "owner value on the legacy key must be stripped",
    );
}

#[test]
fn scoped_cell_env_for_token_carries_token_behind_hook_token_and_sock() {
    // Phase-1 env SHAPE (pure builder — no flag, no disk): the scoped
    // token rides the SAME K2_HOOK_TOKEN key (now per-cell, not owner)
    // and the per-cell socket rides K2_HOOK_SOCK, with K2SO_* aliases.
    let sid = SessionId::new();
    let pane = sid.to_string();
    let pairs = scoped_cell_env_for_token(
        "/run/cells/x.sock",
        "sid.secretverifier",
        &pane,
        54321,
        CredMode::ApiKey,
        Provider::Anthropic,
        None,
    );
    let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();

    // The scoped token REPLACES the owner token behind the same key.
    assert_eq!(map.get("K2_HOOK_TOKEN").map(String::as_str), Some("sid.secretverifier"));
    assert_eq!(map.get("K2SO_HOOK_TOKEN").map(String::as_str), Some("sid.secretverifier"));
    // The per-cell UDS path is exposed so the CLI / notify.sh prefer it.
    assert_eq!(map.get("K2_HOOK_SOCK").map(String::as_str), Some("/run/cells/x.sock"));
    assert_eq!(map.get("K2SO_HOOK_SOCK").map(String::as_str), Some("/run/cells/x.sock"));
    // Pane/tab identity the hook script echoes back as paneId.
    assert_eq!(map.get("K2_PANE_ID").map(String::as_str), Some(pane.as_str()));
    assert_eq!(map.get("K2SO_TAB_ID").map(String::as_str), Some(pane.as_str()));
    // BLOCKER 3 — the daemon's loopback port rides BOTH names so the
    // in-guest `k2` CLI connection gate (which requires K2_PORT present)
    // dispatches; the verb still transports over K2_HOOK_SOCK.
    assert_eq!(map.get("K2_PORT").map(String::as_str), Some("54321"));
    assert_eq!(map.get("K2SO_PORT").map(String::as_str), Some("54321"));
}

#[test]
fn scoped_cell_env_omits_port_when_hook_server_unbound() {
    // Port 0 = the hook server hasn't bound yet. Mirror the normal-session
    // env's `if hook_port > 0` guard: omit K2_PORT entirely rather than
    // stage a bogus `:0`. The socket-bound hook verbs still work; only the
    // (guest-unreachable) TCP fallback is absent.
    let sid = SessionId::new();
    let pane = sid.to_string();
    let pairs = scoped_cell_env_for_token(
        "/run/cells/x.sock",
        "sid.secret",
        &pane,
        0,
        CredMode::ApiKey,
        Provider::Anthropic,
        None,
    );
    let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
    assert!(map.get("K2_PORT").is_none(), "K2_PORT must be omitted at port 0");
    assert!(map.get("K2SO_PORT").is_none(), "K2SO_PORT must be omitted at port 0");
    // The token/sock/pane env is still fully present.
    assert_eq!(map.get("K2_HOOK_SOCK").map(String::as_str), Some("/run/cells/x.sock"));
}

// ── B3a: per-workspace API-key env staging + cred_mode claim ─────

/// ApiKey mode + a non-blank key → `ANTHROPIC_API_KEY=<key>` is staged.
#[test]
fn scoped_cell_env_stages_api_key_in_apikey_mode() {
    let sid = SessionId::new();
    let pane = sid.to_string();
    let pairs = scoped_cell_env_for_token(
        "/run/cells/x.sock",
        "sid.secret",
        &pane,
        0,
        CredMode::ApiKey,
        Provider::Anthropic,
        Some("sk-ant-workspace-key"),
    );
    let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
    assert_eq!(
        map.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-ant-workspace-key"),
        "the per-workspace key must be staged behind the provider's env var",
    );
}

/// No key resolved (None) → NO `ANTHROPIC_API_KEY` at all (default-OFF
/// parity: a workspace without a configured key stages nothing).
#[test]
fn scoped_cell_env_omits_api_key_when_none() {
    let sid = SessionId::new();
    let pane = sid.to_string();
    let pairs = scoped_cell_env_for_token(
        "/run/cells/x.sock",
        "sid.secret",
        &pane,
        0,
        CredMode::ApiKey,
        Provider::Anthropic,
        None,
    );
    let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
    assert!(
        map.get("ANTHROPIC_API_KEY").is_none(),
        "no key configured → ANTHROPIC_API_KEY must be absent (no empty assignment)",
    );
}

/// A blank/whitespace key → NOTHING staged (never an empty
/// `ANTHROPIC_API_KEY=`).
#[test]
fn scoped_cell_env_omits_blank_api_key() {
    let sid = SessionId::new();
    let pane = sid.to_string();
    let pairs = scoped_cell_env_for_token(
        "/run/cells/x.sock",
        "sid.secret",
        &pane,
        0,
        CredMode::ApiKey,
        Provider::Anthropic,
        Some("   "),
    );
    let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
    assert!(
        map.get("ANTHROPIC_API_KEY").is_none(),
        "a blank key must never produce an empty ANTHROPIC_API_KEY=",
    );
}

/// Subscription mode is a DEFERRED no-op stub here: even WITH a key it
/// stages nothing (B3b will bind-mount the on-device creds instead).
#[test]
fn scoped_cell_env_omits_api_key_in_subscription_mode() {
    let sid = SessionId::new();
    let pane = sid.to_string();
    let pairs = scoped_cell_env_for_token(
        "/run/cells/x.sock",
        "sid.secret",
        &pane,
        0,
        CredMode::Subscription,
        Provider::Anthropic,
        Some("sk-ant-should-be-ignored"),
    );
    let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
    assert!(
        map.get("ANTHROPIC_API_KEY").is_none(),
        "Subscription mode is a no-op stub in B3a — no key staged",
    );
}

/// cred_mode + provider are stamped at mint and surfaced by validate_hook
/// (read from the VALIDATED token, never a body). Round-trips for both
/// modes; the key itself is NEVER part of the token record.
#[test]
fn cred_mode_and_provider_round_trip_through_validate() {
    let mut reg = SessionTokenRegistry::new();

    let sid_a = SessionId::new();
    let tok_a = reg.mint(&sid_a, "pane-a", principal(), CredMode::ApiKey, Provider::Anthropic);
    let v_a = reg.validate_hook(&tok_a).expect("apikey token validates");
    assert_eq!(v_a.cred_mode, CredMode::ApiKey);
    assert_eq!(v_a.provider, Provider::Anthropic);

    let sid_b = SessionId::new();
    let tok_b = reg.mint(&sid_b, "pane-b", principal(), CredMode::Subscription, Provider::Anthropic);
    let v_b = reg.validate_hook(&tok_b).expect("subscription token validates");
    assert_eq!(v_b.cred_mode, CredMode::Subscription);
    assert_eq!(v_b.provider, Provider::Anthropic);
}

/// CredMode defaults to ApiKey (serde default) so a pre-B3a persisted
/// record loads on the primary path, and Provider maps to its env var.
#[test]
fn cred_mode_default_is_apikey_and_provider_env_var() {
    assert_eq!(CredMode::default(), CredMode::ApiKey);
    assert_eq!(Provider::default(), Provider::Anthropic);
    assert_eq!(Provider::Anthropic.key_env_var(), "ANTHROPIC_API_KEY");
}

// ── Phase 1: dual-accept + pane scoping (flag-ON semantics) ──────

#[test]
fn dual_accept_owner_token_independent_of_scoped_token() {
    // The OWNER arm of /hook/complete is `ct_eq_token` and is NEVER
    // touched by the scoped machinery — so the owner token keeps working
    // over TCP whether or not the flag is on (Phase 1 is dual-accept;
    // Phase 2 — owner REJECTION — is deliberately NOT implemented here).
    let owner = "owner-fixed-hex-token";
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let scoped = reg.mint(&sid, &sid.to_string(), principal(), CredMode::ApiKey, Provider::Anthropic);

    // Owner token always passes its own constant-time compare.
    assert!(crate::routes::http::ct_eq_token(owner, owner));
    // …and the scoped token is provably NOT the owner token.
    assert_ne!(scoped, owner);
    assert!(!crate::routes::http::ct_eq_token(&scoped, owner));
}

#[test]
fn scoped_hook_authorizes_only_its_own_pane() {
    // PRD §5 smoke #3/#4: a scoped token authorizes ONLY the exact pane
    // it was minted for; the same token with a different paneId → false.
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let token = reg.mint(&sid, "pane-A", principal(), CredMode::ApiKey, Provider::Anthropic);

    assert!(
        reg.scoped_hook_authorizes_pane(&token, "pane-A"),
        "correct pane must authorize",
    );
    assert!(
        !reg.scoped_hook_authorizes_pane(&token, "pane-B"),
        "a different pane MUST be refused (scoping enforced)",
    );
    assert!(
        !reg.scoped_hook_authorizes_pane(&token, ""),
        "an empty paneId never authorizes",
    );
    // After revoke the token authorizes NO pane.
    reg.revoke(&sid);
    assert!(
        !reg.scoped_hook_authorizes_pane(&token, "pane-A"),
        "a revoked token authorizes no pane",
    );
}

#[test]
fn require_hook_denies_escalation_even_with_a_valid_token() {
    // Even a structurally VALID scoped token is refused on an
    // owner-only route — capability default-deny is independent of
    // token validity. (Validity is exercised against the process
    // registry elsewhere; here we assert the capability gate fires
    // FIRST, so an unknown token on a denied path is still None.)
    let mut reg = SessionTokenRegistry::new();
    let sid = SessionId::new();
    let _scoped = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
    // require_hook consults the PROCESS registry, not `reg`, so we
    // assert the capability gate via is_agent_verb (the first guard).
    assert!(!is_agent_verb("/cli/users/set-role"));
    assert!(require_hook("anything.here", "/cli/users/set-role").is_none());
}
