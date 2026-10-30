//! `/cli/publish/visibility`, `/open` and `/sessions` (PV-5, PV-7, PV-10, PV-14,
//! DM-3). Every refusal here is a security path: the test says what it refuses
//! and that nothing moved.

use super::*;
use crate::compute_routes::Who;
use std::collections::HashMap;

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn body(r: &CliResponse) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
}

fn mode_of(label: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    sv::effective_mode(&conn, label).expect("mode").as_str().to_string()
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.to_string(), session_id: "s-1".to_string() }
}

fn member() -> Who {
    Who::Login { username: "mel".to_string(), role: Role::Member }
}

fn admin() -> Who {
    Who::Login { username: "ada".to_string(), role: Role::Admin }
}

/// A box with a tunnel (E2E on) and a workspace holding one tunnelled service.
struct Box1 {
    home: crate::test_support::TempHome,
    ws: String,
    other: String,
    label: String,
}

fn fixture(public: bool) -> Box1 {
    let home = crate::test_support::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    std::fs::write(
        home.path().join(".k2/tunnel.json"),
        json!({ "token": "tok_test", "subdomain": "rosson", "e2e": true }).to_string(),
    )
    .expect("tunnel.json");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let (ws, other) = (format!("ws-{id}"), format!("other-{id}"));
    let label = format!("site-{}", &id[..10]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    for w in [&ws, &other] {
        conn.execute("INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)", rusqlite::params![w, format!("/tmp/{w}"), w]).expect("workspace");
    }
    k2_core::published_services::insert(&conn, &ws, &label, "true", &format!("/tmp/{ws}"), 41901, "tunnel", "stopped", "cmd", "").expect("service");
    if public {
        sv::set_public(&conn, &label, "public before 0.46.7", sv::SET_BY_UPGRADE).expect("public");
    }
    drop(conn);
    Box1 { home, ws, other, label }
}

#[test]
fn the_mutating_routes_are_post_only_and_the_reads_answer_get() {
    let p = HashMap::new();
    for path in ["/cli/publish/open", "/cli/publish/sessions/revoke"] {
        let r = crate::publish_routes::dispatch(path, &p).expect("GET twin");
        assert_eq!(r.status, "405 Method Not Allowed", "{path}");
    }
    for path in ["/cli/publish/visibility", "/cli/publish/sessions"] {
        let r = crate::publish_routes::dispatch(path, &p).expect("GET route");
        assert_eq!(r.status, "400 Bad Request", "{path}: {}", r.body);
    }
}

/// Floors: a Member can't reach `open` or `sessions` at all (PV-14).
#[test]
fn the_route_floors_hold_for_logins() {
    use crate::routes::route_policy::{lookup, Floor};
    let route = |p: &str| lookup(p).unwrap_or_else(|| panic!("{p} isn't classified"));
    assert_eq!(route("/cli/publish/open").post, Some(Floor::Admin));
    assert_eq!(route("/cli/publish/open").get, None, "POST only");
    assert_eq!(route("/cli/publish/sessions/revoke").post, Some(Floor::Admin));
    assert_eq!(route("/cli/publish/sessions").get, Some(Floor::Admin));
    assert_eq!(route("/cli/publish/visibility").get, Some(Floor::Member));
    assert_eq!(route("/cli/publish/visibility").post, Some(Floor::Member));
}

/// PV-14 leak test: another workspace's agent can't tell a site from nothing:
/// not its mode, not its sessions, and it can't change it.
#[test]
fn another_workspaces_agent_gets_the_same_answer_as_for_no_site() {
    let b = fixture(false);
    let foreign = |path: &str| {
        let mut p = params(&[("label", &b.label), ("project_id", &b.other)]);
        p.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        crate::publish_routes::dispatch(path, &p).expect("route")
    };
    let nothing = {
        let mut p = params(&[("label", "no-such-site"), ("project_id", &b.other)]);
        p.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        crate::publish_routes::dispatch("/cli/publish/visibility", &p).expect("route")
    };
    assert_eq!(nothing.status, "404 Not Found", "{}", nothing.body);
    for path in ["/cli/publish/visibility", "/cli/publish/sessions"] {
        let r = foreign(path);
        assert_eq!(r.status, nothing.status, "{path}: {}", r.body);
        assert_eq!(body(&r)["code"], "label_not_found", "{path}");
        assert!(!r.body.contains("private"), "{path} leaked a mode: {}", r.body);
    }
    let set = handle_set(&params(&[("label", &b.label), ("mode", "private")]), &agent(&b.other));
    assert_eq!(set.status, "404 Not Found", "{}", set.body);
    let revoke = handle_revoke(&params(&[("label", &b.label)]), &agent(&b.other));
    assert_eq!(revoke.status, "404 Not Found", "{}", revoke.body);
    let _ = &b.home;
}

#[test]
fn a_site_with_no_row_shows_private_and_its_own_agent_can_read_it() {
    let b = fixture(false);
    let mut p = params(&[("label", &b.label), ("project_id", &b.ws)]);
    p.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
    let r = handle_show(&p);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["mode"], "private", "{v}");
    assert_eq!(v["explicit"], false, "{v}");
    assert_eq!(v["enforced"], true, "{v}");
}

/// PA-12: an agent may make its site private, never PIN or public.
#[test]
fn an_agent_can_make_its_site_private_and_only_asks_for_the_rest() {
    let b = fixture(true);
    let who = agent(&b.ws);

    // PIN: a card, and a PIN it sends is never read.
    let r = handle_set(&params(&[("label", &b.label), ("mode", "pin"), ("pin", "hunter2hunter2")]), &who);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v = body(&r);
    assert_eq!((v["status"].as_str(), v["code"].as_str()), (Some("held"), Some("pin_needs_person")), "{v}");
    assert!(!r.body.contains("hunter2"), "the PIN never comes back: {}", r.body);
    assert_eq!(mode_of(&b.label), "public", "nothing moved");

    // Public: needs a reason, then a card.
    let r = handle_set(&params(&[("label", &b.label), ("mode", "public")]), &who);
    assert_eq!(body(&r)["code"], "reason_required", "{}", r.body);
    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "launch")]), &who);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v = body(&r);
    assert_eq!((v["status"].as_str(), v["code"].as_str()), (Some("held"), Some("public_needs_person")), "{v}");
    assert_eq!(v["label"], b.label.as_str(), "the wall reads this: {v}");

    // Private: done, past the parser warning like anyone else.
    let r = handle_set(&params(&[("label", &b.label), ("mode", "private"), ("yes", "1")]), &who);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(mode_of(&b.label), "private");

    // Now public by a card only: it stays private.
    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "launch")]), &who);
    assert_eq!(body(&r)["status"], "held", "{}", r.body);
    assert_eq!(mode_of(&b.label), "private");
}

/// A K2 shell tab carries a passport, so it follows the agent's rules.
#[test]
fn a_shell_tab_follows_the_agents_rules() {
    let b = fixture(false);
    let shell = Who::Shell { project_id: b.ws.clone(), session_id: "tab".into() };
    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "x")]), &shell);
    assert_eq!(body(&r)["status"], "held", "{}", r.body);
    let r = handle_open(&params(&[("label", &b.label)]), &shell);
    assert_eq!(body(&r)["code"], "person_only", "{}", r.body);
}

/// PV-10: owner and Admin can; Member can't; the reason is required.
#[test]
fn public_is_an_owner_or_admin_choice_with_a_reason() {
    let b = fixture(false);
    let set = |who: &Who, extra: &[(&str, &str)]| {
        let mut p = params(&[("label", &b.label), ("mode", "public")]);
        for (k, v) in extra {
            p.insert(k.to_string(), v.to_string());
        }
        handle_set(&p, who)
    };
    let r = set(&member(), &[("reason", "launch")]);
    assert_eq!((r.status, body(&r)["code"].as_str().map(str::to_string)), ("403 Forbidden", Some("owner_or_admin".to_string())), "{}", r.body);
    assert_eq!(mode_of(&b.label), "private");

    let r = set(&Who::Owner, &[]);
    assert_eq!(body(&r)["code"], "reason_required", "{}", r.body);
    let r = set(&Who::Owner, &[("reason", "   ")]);
    assert_eq!(body(&r)["code"], "reason_required", "{}", r.body);
    assert_eq!(mode_of(&b.label), "private");

    let r = set(&admin(), &[("reason", "launch day")]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!((v["mode"].as_str(), v["reason"].as_str()), (Some("public"), Some("launch day")), "{v}");
    assert_eq!(v["setBy"], "login:ada", "{v}");

    for who in [Who::Wrong, Who::Invalid] {
        let r = set(&who, &[("reason", "x")]);
        assert_eq!(r.status, "403 Forbidden", "{who:?}: {}", r.body);
    }
}

/// PV-7: who, the length rules, the hash, and the generation.
#[test]
fn a_pin_is_a_managers_and_is_stored_only_as_a_hash() {
    let b = fixture(false);
    let set = |who: &Who, pin: Option<&str>| {
        let mut p = params(&[("label", &b.label), ("mode", "pin")]);
        if let Some(x) = pin {
            p.insert("pin".to_string(), x.to_string());
        }
        handle_set(&p, who)
    };
    assert_eq!(body(&set(&member(), Some("12345678")))["code"], "owner_or_admin");
    assert_eq!(mode_of(&b.label), "private");
    assert_eq!(body(&set(&Who::Owner, None))["code"], "pin_required");
    assert_eq!(body(&set(&Who::Owner, Some("12345")))["code"], "pin_length");
    assert_eq!(body(&set(&Who::Owner, Some(&"x".repeat(65))))["code"], "pin_length");
    assert_eq!(body(&set(&Who::Owner, Some("1234\n5678")))["code"], "pin_characters");
    assert_eq!(mode_of(&b.label), "private");

    let r = set(&Who::Owner, Some("123456"));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!((v["mode"].as_str(), v["pinSet"].as_bool()), (Some("pin"), Some(true)), "{v}");
    assert!(v["warnings"].as_array().expect("warnings").iter().any(|w| w.as_str().is_some_and(|w| w.contains("under 8"))), "{v}");
    assert!(!r.body.contains("123456"), "the PIN never comes back: {}", r.body);

    let db = k2_core::db::shared();
    let (stored, generation): (String, i64) = {
        let conn = db.lock();
        conn.query_row("SELECT pin_hash, pin_generation FROM site_visibility WHERE label = ?1", rusqlite::params![b.label], |r| Ok((r.get(0)?, r.get(1)?))).expect("row")
    };
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    assert!(!stored.contains("123456"));

    let r = set(&Who::Owner, Some("a-longer-pin"));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["warnings"].as_array().expect("warnings").iter().filter(|w| w.as_str().is_some_and(|w| w.contains("under 8"))).count(), 0);
    let conn = db.lock();
    let next: i64 = conn.query_row("SELECT pin_generation FROM site_visibility WHERE label = ?1", rusqlite::params![b.label], |r| r.get(0)).expect("row");
    assert!(next > generation, "a change bumps the generation: {generation} → {next}");
}

/// `check` tells the CLI whether to ask for a PIN, before anyone types one.
#[test]
fn check_asks_without_a_pin_and_changes_nothing() {
    let b = fixture(false);
    let r = handle_set(&params(&[("label", &b.label), ("mode", "pin"), ("check", "1")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["check"], true);
    assert_eq!(mode_of(&b.label), "private", "check changes nothing");
    let r = handle_set(&params(&[("label", &b.label), ("mode", "pin"), ("check", "1")]), &member());
    assert_eq!(body(&r)["code"], "owner_or_admin", "{}", r.body);
    let r = handle_set(&params(&[("label", &b.label), ("mode", "pin"), ("check", "1")]), &agent(&b.ws));
    assert_eq!(body(&r)["status"], "held", "{}", r.body);
}

/// PV-3: moving a public site behind the gate needs `yes`; a site already behind
/// it, or already parsed, doesn't.
#[test]
fn moving_a_public_site_behind_the_gate_warns_and_needs_yes() {
    let b = fixture(true);
    let r = handle_set(&params(&[("label", &b.label), ("mode", "private")]), &Who::Owner);
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["code"], "needs_yes", "{v}");
    let text = v["error"].as_str().expect("words");
    assert!(text.contains("HTTP/1.1") && text.contains("--yes") && text.contains(&b.label), "{text}");
    assert_eq!(mode_of(&b.label), "public", "nothing moved");

    let r = handle_set(&params(&[("label", &b.label), ("mode", "private"), ("yes", "1")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let r = handle_set(&params(&[("label", &b.label), ("mode", "pin"), ("pin", "12345678")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "already behind the gate: {}", r.body);
}

/// PV-4 / PA-3: with E2E off the gate can't see the traffic: private and PIN
/// are refused, naming the door, and public still works.
#[test]
fn e2e_off_refuses_private_and_pin_and_says_so() {
    let b = fixture(false);
    std::fs::write(
        b.home.path().join(".k2/tunnel.json"),
        json!({ "token": "tok_test", "subdomain": "rosson", "e2e": false }).to_string(),
    )
    .expect("tunnel.json");
    for (mode, pin) in [("private", ""), ("pin", "12345678")] {
        let r = handle_set(&params(&[("label", &b.label), ("mode", mode), ("pin", pin)]), &Who::Owner);
        assert_eq!(r.status, "409 Conflict", "{mode}: {}", r.body);
        let v = body(&r);
        assert_eq!(v["code"], sv::DOOR_UNSUPPORTED, "{mode}: {v}");
        assert!(v["error"].as_str().expect("words").contains("E2E"), "{v}");
    }
    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "E2E is off here")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["enforced"], true, "public needs no gate: {}", r.body);
}

/// P-D3: an App's own logins decide; K2's front doors aren't set here.
#[test]
fn reserved_labels_and_unknown_labels_are_refused() {
    let b = fixture(false);
    for reserved in ["skin", "meet", "chat", "mail"] {
        let r = handle_set(&params(&[("label", reserved), ("mode", "private")]), &Who::Owner);
        assert_eq!(body(&r)["code"], "visibility_not_applicable", "{reserved}: {}", r.body);
    }
    let r = handle_set(&params(&[("label", "no-such-site"), ("mode", "private")]), &Who::Owner);
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    let r = handle_set(&params(&[("label", "Not A Label!"), ("mode", "private")]), &Who::Owner);
    assert_eq!(body(&r)["code"], "bad_label", "{}", r.body);
    let r = handle_set(&params(&[("label", &b.label), ("mode", "secret")]), &Who::Owner);
    assert_eq!(body(&r)["code"], "usage", "{}", r.body);
}

/// DM-3: public dev is a person's, with the flag; an agent can't even ask.
#[test]
fn a_dev_site_is_never_public_by_accident() {
    let b = fixture(false);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        sv::set_dev(&conn, &b.label, true, None, "test").expect("dev");
    }
    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "demo")]), &agent(&b.ws));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(body(&r)["code"], "dev_public_person_only", "{}", r.body);
    assert_ne!(body(&r)["status"], "held", "no card to ask for this: {}", r.body);

    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "demo")]), &Who::Owner);
    assert_eq!(body(&r)["code"], "dev_public_needs_flag", "{}", r.body);
    assert!(r.body.contains("source maps"), "{}", r.body);
    assert_eq!(mode_of(&b.label), "private");

    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "demo"), ("devIsPublic", "1")]), &member());
    assert_eq!(body(&r)["code"], "owner_or_admin", "{}", r.body);

    let r = handle_set(&params(&[("label", &b.label), ("mode", "public"), ("reason", "demo"), ("devIsPublic", "1")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(r.body.contains("source maps"), "the warning comes with it: {}", r.body);

    // Back to PIN: the person hears it again.
    let r = handle_set(&params(&[("label", &b.label), ("mode", "pin"), ("pin", "12345678")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(body(&r)["warnings"].as_array().expect("warnings").iter().any(|w| w.as_str().is_some_and(|w| w.contains("person with the PIN"))), "{}", r.body);
}

/// PV-5: agents refused; a code for a person; never an off-site path.
#[test]
fn open_gives_a_person_a_one_time_link_and_refuses_everyone_else() {
    let b = fixture(false);
    let open = |who: &Who, extra: &[(&str, &str)]| {
        let mut p = params(&[("label", &b.label)]);
        for (k, v) in extra {
            p.insert(k.to_string(), v.to_string());
        }
        handle_open(&p, who)
    };
    let r = open(&agent(&b.ws), &[]);
    assert_eq!((r.status, body(&r)["code"].as_str().map(str::to_string)), ("403 Forbidden", Some("person_only".to_string())), "{}", r.body);
    assert!(r.body.contains("local port"), "says what to do instead: {}", r.body);
    assert_eq!(body(&open(&member(), &[]))["code"], "owner_or_admin");
    assert_eq!(open(&Who::Invalid, &[]).status, "403 Forbidden");
    assert_eq!(open(&Who::Wrong, &[]).status, "403 Forbidden");

    let r = open(&Who::Owner, &[]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["oneTime"], true, "{v}");
    assert!(v["url"].as_str().expect("url").contains("/_k2/site/open?code="), "{v}");
    assert_eq!(v["expiresInSecs"], sv::OPEN_CODE_SECS, "{v}");
    let code = v["url"].as_str().expect("url").rsplit('=').next().expect("code").to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let red = sv::redeem_open_code(&conn, &b.label, &code).expect("redeem").expect("the code works");
        assert_eq!(red.user, sv::USER_OWNER_TOKEN);
        assert!(sv::redeem_open_code(&conn, &b.label, &code).expect("redeem").is_none(), "once");
    }

    for bad in ["//evil.example", "https://evil.example", "/\\evil.example", "docs"] {
        assert_eq!(body(&open(&Who::Owner, &[("path", bad)]))["code"], "bad_path", "{bad}");
    }
    let r = open(&admin(), &[("path", "/guide")]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
}

/// A public site needs no code: the plain address, with no session minted.
#[test]
fn open_on_a_public_site_is_the_plain_address() {
    let b = fixture(true);
    let r = handle_open(&params(&[("label", &b.label), ("path", "/a")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["oneTime"], false, "{v}");
    assert!(!r.body.contains("code="), "{v}");
}

/// PA-2: the K2 tab's swap: `{swap: false}` unless it's this server's gated site.
#[test]
fn the_tab_swap_only_touches_this_servers_gated_sites() {
    let b = fixture(true);
    let url = format!("https://{}.rosson.k2.dev/guide?x=1", b.label);
    let swap = |who: &Who, url: &str| handle_open(&params(&[("url", url)]), who);

    assert_eq!(body(&swap(&Who::Owner, &url))["swap"], false, "public: the plain address");
    for other in ["https://example.com/", "https://rosson.k2.dev/", "http://x.rosson.k2.dev/", "https://x.y.rosson.k2.dev/", "https://x.someone.k2.dev/"] {
        assert_eq!(body(&swap(&Who::Owner, other))["swap"], false, "{other}");
    }
    assert_eq!(body(&swap(&Who::Owner, "https://no-such-site.rosson.k2.dev/"))["swap"], false);

    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        sv::set_private(&conn, &b.label, "test").expect("private");
    }
    let r = swap(&Who::Owner, &url);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["swap"], true, "{v}");
    let link = v["url"].as_str().expect("url");
    assert!(link.contains("/_k2/site/open?code="), "{v}");
    let code = link.rsplit('=').next().expect("code");
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(sv::redeem_open_code(&conn, &b.label, code).expect("redeem").expect("works").path, "/guide?x=1", "lands where the person was going");
    drop(conn);

    let r = swap(&agent(&b.ws), &url);
    assert_eq!(body(&r)["code"], "person_only", "an agent's Browser gets no link: {}", r.body);
}

#[test]
fn sessions_list_who_is_in_and_revoke_ends_them() {
    let b = fixture(false);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        sv::create_session(&conn, &b.label, sv::SessionKind::Open, "ada").expect("session");
    }
    let r = handle_sessions(&params(&[("label", &b.label)]));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["sessions"].as_array().expect("sessions").len(), 1, "{v}");
    assert_eq!(v["sessions"][0]["user"], "ada", "{v}");
    assert!(v["sessions"][0].get("idHash").is_none(), "never the id: {v}");

    assert_eq!(handle_revoke(&params(&[("label", &b.label)]), &Who::Invalid).status, "403 Forbidden");
    let r = handle_revoke(&params(&[("label", &b.label)]), &agent(&b.ws));
    assert_eq!(r.status, "200 OK", "it only tightens: {}", r.body);
    assert_eq!(body(&r)["revoked"], 1);
    let v = body(&handle_sessions(&params(&[("label", &b.label)])));
    assert_eq!(v["sessions"].as_array().expect("sessions").len(), 0, "{v}");
}

/// PV-4: a bind beyond loopback is reported on a gated site, and not on a public
/// one (where the address is open to everyone anyway).
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_gated_site_listening_on_every_address_is_reported() {
    let b = fixture(false);
    let wide = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("bind");
    let port = wide.local_addr().expect("addr").port();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE published_services SET port = ?1 WHERE name = ?2", rusqlite::params![port as i64, b.label]).expect("port");
    }
    let v = body(&handle_show(&params(&[("label", &b.label)])));
    let warnings = v["warnings"].as_array().expect("warnings");
    assert!(warnings.iter().any(|w| w.as_str().is_some_and(|w| w.contains(&format!(":{port}")) && w.contains("without K2's check"))), "{v}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        sv::set_public(&conn, &b.label, "open", "test").expect("public");
    }
    let v = body(&handle_show(&params(&[("label", &b.label)])));
    assert_eq!(v["warnings"], json!([]), "{v}");
}

/// PV-10: the card's Allow goes through the route's own entry as the person,
/// so a Member's click is refused there too.
#[test]
fn the_cards_apply_runs_the_routes_floor_as_the_answerer() {
    use crate::permission::actions::{publish, Answerer, Applied, Ctx};
    let b = fixture(false);
    let mut map = serde_json::Map::new();
    map.insert("label".into(), J::String(b.label.clone()));
    map.insert("reason".into(), J::String("launch".into()));
    let ctx = Ctx { workspace_id: b.ws.clone(), workspace_handle: "w".into(), params: map };
    let reg = publish::actions().into_iter().find(|r| r.spec.id == crate::publish_cards::PUBLIC).expect("registered");

    let member = Answerer::Login { user: "mel".into(), role: "member".into() };
    match (reg.apply)(&ctx, &member, None) {
        Applied::Refused { code, .. } => assert_eq!(code, "owner_or_admin"),
        other => panic!("a Member's click must be refused: {other:?}"),
    }
    assert_eq!(mode_of(&b.label), "private");
    match (reg.apply)(&ctx, &Answerer::OwnerToken, None) {
        Applied::Done { message } => assert!(message.contains("public"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(mode_of(&b.label), "public");
    assert_eq!((reg.check)(&ctx), crate::permission::actions::Check::AlreadyDone);

    // Another workspace's agent can't ask about this site.
    let foreign = Ctx { workspace_id: b.other.clone(), workspace_handle: "o".into(), params: ctx.params.clone() };
    assert!(matches!((reg.check)(&foreign), crate::permission::actions::Check::Moved(_)));
}

/// PV-7: Allow on the PIN card decides nothing until the PIN is set, whoever
/// clicks and from wherever, and the card takes no PIN.
#[test]
fn the_pin_card_needs_the_pin_set_first() {
    use crate::permission::actions::{publish, Check, Ctx};
    let b = fixture(false);
    let mut map = serde_json::Map::new();
    map.insert("label".into(), J::String(b.label.clone()));
    let ctx = Ctx { workspace_id: b.ws.clone(), workspace_handle: "w".into(), params: map };
    let reg = publish::actions().into_iter().find(|r| r.spec.id == crate::publish_cards::PIN).expect("registered");
    assert_eq!((reg.check)(&ctx), Check::Needed);
    let r = handle_set(&params(&[("label", &b.label), ("mode", "pin"), ("pin", "12345678")]), &Who::Owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!((reg.check)(&ctx), Check::AlreadyDone, "the form set it: the card closes as done");
}
