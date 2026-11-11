//! Tests for the setup checklist (prd-setup-checklist-v1 §8).

use std::collections::BTreeSet;

use chrono::{Duration, Utc};
use rusqlite::Connection;

use super::*;
use crate::connect_users::{ConnectUserView, Role};

fn db() -> Connection {
    let conn = Connection::open(":memory:").expect("open");
    conn.execute_batch("PRAGMA foreign_keys = ON;").expect("pragma");
    crate::db::run_migrations(&conn).expect("migrate");
    conn
}

fn user(name: &str, role: Role, created: chrono::DateTime<Utc>) -> ConnectUserView {
    ConnectUserView {
        username: name.to_string(),
        created_at: created,
        disabled: false,
        role,
        was_viewer: false,
        can_clone: true,
        color: None,
        first_name: None,
        last_name: None,
        display_name: name.to_string(),
    }
}

/// anna and gina (Members), julie (an unlinked Admin), z3thon (an Admin
/// the owner linked).
fn people() -> People {
    let t = Utc::now() - Duration::days(10);
    People {
        users: vec![
            user("anna", Role::Member, t),
            user("gina", Role::Member, t),
            user("julie", Role::Admin, t),
            user("z3thon", Role::Admin, t),
        ],
        linked: vec!["z3thon".to_string()],
    }
}

fn none() -> BTreeSet<String> {
    BTreeSet::new()
}

fn ids(rows: &[Row]) -> Vec<&'static str> {
    rows.iter().map(|r| r.id).collect()
}

fn row<'a>(rows: &'a [Row], id: &str) -> &'a Row {
    rows.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no {id} row in {rows:?}"))
}

fn all_done() -> Facts {
    Facts {
        llm_signed_in: true,
        has_agent: true,
        tunnel_done: true,
        airgap: false,
        owner_linked: true,
        phones: 1,
        ..Facts::default()
    }
}

// ── rows: done and todo ──────────────────────────────────────────────

#[test]
fn each_row_is_todo_until_its_state_is_there_then_done() {
    let empty = Facts::default();
    let rows0 = rows(Viewer::OwnerToken, &empty, &none());
    assert_eq!(ids(&rows0), vec!["llm_sign_in", "first_agent", "tunnel", "phone", "own_sign_in"]);
    for r in &rows0 {
        assert_eq!(r.state, State::Todo, "{}", r.id);
    }
    let cases: [(&str, Facts); 4] = [
        ("llm_sign_in", Facts { llm_signed_in: true, ..Facts::default() }),
        ("first_agent", Facts { has_agent: true, ..Facts::default() }),
        ("tunnel", Facts { tunnel_done: true, ..Facts::default() }),
        ("phone", Facts { phones: 2, ..Facts::default() }),
    ];
    for (id, facts) in cases {
        let rs = rows(Viewer::OwnerToken, &facts, &none());
        for r in &rs {
            let want = if r.id == id { State::Done } else { State::Todo };
            assert_eq!(r.state, want, "{id} set: row {}", r.id);
        }
    }
    // own_sign_in is never "done": it goes once a login is linked.
    let linked = rows(Viewer::OwnerToken, &Facts { owner_linked: true, ..Facts::default() }, &none());
    assert!(!ids(&linked).contains(&"own_sign_in"), "{linked:?}");
}

#[test]
fn opens_targets_are_the_contracts() {
    let rs = rows(Viewer::OwnerToken, &Facts { tunnel_done: true, ..Facts::default() }, &none());
    let opens: Vec<(&str, &str)> = rs.iter().map(|r| (r.id, r.opens.expect("opens"))).collect();
    assert_eq!(
        opens,
        vec![
            ("llm_sign_in", "llm_sign_in"),
            ("first_agent", "add_workspace"),
            ("tunnel", "tunnel"),
            ("phone", "phone"),
            ("own_sign_in", "owner_account_prompt"),
        ]
    );
    assert!(rs.iter().all(|r| r.note.is_none()), "tunnel on: no notes {rs:?}");
}

#[test]
fn skipped_shows_as_skipped_and_done_wins_over_skipped() {
    let skipped: BTreeSet<String> = ["first_agent".to_string(), "phone".to_string()].into();
    let rs = rows(Viewer::Login(Role::Member), &Facts { phones: 1, ..Facts::default() }, &skipped);
    assert_eq!(row(&rs, "first_agent").state, State::Skipped);
    assert_eq!(row(&rs, "phone").state, State::Done, "a done row reads done even when skipped");
}

#[test]
fn the_phone_row_points_at_the_tunnel_first_when_the_caller_can_turn_it_on() {
    // Owner token, tunnel todo: opens the tunnel, with the note.
    let rs = rows(Viewer::OwnerToken, &Facts::default(), &none());
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), (Some("tunnel"), Some(NOTE_NEEDS_TUNNEL)));
    // A Member can't see the tunnel row: the phone page, with the note.
    let rs = rows(Viewer::Login(Role::Member), &Facts::default(), &none());
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), (Some("phone"), Some(NOTE_NEEDS_TUNNEL)));
    // The owner skipped the tunnel: not todo any more, so the phone page.
    let skipped: BTreeSet<String> = ["tunnel".to_string()].into();
    let rs = rows(Viewer::OwnerToken, &Facts::default(), &skipped);
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), (Some("phone"), Some(NOTE_NEEDS_TUNNEL)));
    // Tunnel on: the phone page, no note.
    let rs = rows(Viewer::Login(Role::Member), &Facts { tunnel_done: true, ..Facts::default() }, &none());
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), (Some("phone"), None));
    // A paired phone needs no note.
    let rs = rows(Viewer::OwnerToken, &Facts { phones: 1, ..Facts::default() }, &none());
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), (Some("phone"), None));
}

// ── role visibility ──────────────────────────────────────────────────

#[test]
fn each_role_sees_only_the_rows_it_can_act_on() {
    let f = Facts::default();
    assert_eq!(ids(&rows(Viewer::Login(Role::Member), &f, &none())), vec!["first_agent", "phone"]);
    assert_eq!(ids(&rows(Viewer::Login(Role::Admin), &f, &none())), vec!["llm_sign_in", "first_agent", "phone"]);
    assert_eq!(
        ids(&rows(Viewer::Login(Role::Owner), &f, &none())),
        vec!["llm_sign_in", "first_agent", "tunnel", "phone"]
    );
    assert_eq!(
        ids(&rows(Viewer::OwnerToken, &f, &none())),
        vec!["llm_sign_in", "first_agent", "tunnel", "phone", "own_sign_in"],
        "only the owner token sees own_sign_in"
    );
    assert!(rows(Viewer::Login(Role::VoiceChat), &f, &none()).is_empty(), "Voice & Chat never gets the list");
}

#[test]
fn air_gap_hides_the_tunnel_row_and_its_note() {
    let f = Facts { airgap: true, ..Facts::default() };
    let rs = rows(Viewer::OwnerToken, &f, &none());
    assert_eq!(ids(&rs), vec!["llm_sign_in", "first_agent", "phone", "own_sign_in"]);
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), (Some("phone"), None));
}

#[test]
fn v1_rows_are_in_contract_order() {
    let all = rows(Viewer::OwnerToken, &Facts::default(), &none());
    let order: Vec<usize> = all.iter().map(|r| ROW_IDS.iter().position(|id| *id == r.id).expect("known id")).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
    assert_eq!(ids(&all), V1_ROWS.to_vec());
}

// ── the readers' rules ───────────────────────────────────────────────

#[test]
fn the_tunnel_counts_when_named_held_and_enabled_not_when_running() {
    use crate::tunnel::TunnelConfig;
    let mut cfg = TunnelConfig::default();
    assert!(!tunnel_configured(&cfg), "a fresh config");
    cfg.subdomain = "rosson".into();
    assert!(!tunnel_configured(&cfg), "no token");
    cfg.token = "tok".into();
    assert!(tunnel_configured(&cfg), "file token");
    cfg.enabled = false;
    assert!(!tunnel_configured(&cfg), "disabled");
    cfg.enabled = true;
    cfg.token = String::new();
    cfg.token_home = Some(crate::tunnel::config::TOKEN_HOME_VAULT.to_string());
    assert!(tunnel_configured(&cfg), "the token is in the vault (never read here)");
    cfg.subdomain = "  ".into();
    assert!(!tunnel_configured(&cfg), "no subdomain");
}

fn entry(tool: &str, kind: &str, state: &str) -> crate::llm_accounts::Entry {
    crate::llm_accounts::Entry {
        id: format!("{tool}-{kind}-{state}"),
        tool: tool.into(),
        label: "work".into(),
        kind: kind.into(),
        position: 0,
        created_by: None,
        created_at: 0,
        last_used_at: None,
        state: state.into(),
        detail: None,
        email: None,
        org: None,
        plan: None,
        expires_at: None,
        refreshed_at: None,
        usage_json: None,
        usage_checked_at: None,
        removed_at: None,
    }
}

#[test]
fn an_llm_login_counts_when_signed_in_or_an_api_key_for_the_three_tools() {
    use crate::llm_accounts::{kind, state};
    assert!(!llm_signed_in(&[]));
    for s in [state::NOT_SET_UP, state::SIGNING_IN, state::NEEDS_LOGIN, state::UNKNOWN] {
        assert!(!llm_signed_in(&[entry("claude", kind::SUBSCRIPTION, s)]), "{s}");
    }
    for t in ["claude", "codex", "grok"] {
        assert!(llm_signed_in(&[entry(t, kind::SUBSCRIPTION, state::SIGNED_IN)]), "{t}");
        assert!(llm_signed_in(&[entry(t, kind::API_KEY, state::NOT_SET_UP)]), "{t} api key");
    }
    assert!(!llm_signed_in(&[entry("gemini", kind::SUBSCRIPTION, state::SIGNED_IN)]), "not one of the three");
}

// ── per person: hide, skip, finish ───────────────────────────────────

#[test]
fn two_logins_hide_and_skip_apart_while_the_owner_token_and_a_linked_login_share_one_list() {
    let conn = db();
    let p = people();
    let f = Facts::default();
    let member = Viewer::Login(Role::Member);
    assert_eq!(p.person_key("anna"), "anna");
    assert_eq!(p.person_key("gina"), "gina");
    assert_eq!(p.person_key(OWNER), OWNER);
    assert_eq!(p.person_key("z3thon"), OWNER, "a linked login is the owner");

    let s = apply_on(&conn, &p, "anna", member, &f, true, &Action::Hide, 100).expect("hide");
    assert!(s.hidden);
    let g = status_on(&conn, &p, "gina", member, &f, true, 101).expect("gina");
    assert!(!g.hidden, "anna's hide is anna's");
    let s = apply_on(&conn, &p, "gina", member, &f, true, &Action::Skip("first_agent".into()), 102).expect("skip");
    assert_eq!(row(&s.rows, "first_agent").state, State::Skipped);
    let a = status_on(&conn, &p, "anna", member, &f, true, 103).expect("anna");
    assert_eq!(row(&a.rows, "first_agent").state, State::Todo, "gina's skip is gina's");

    // The owner token hides; the linked login (another viewer, same person) sees it.
    let s = apply_on(&conn, &p, &p.person_key(OWNER), Viewer::OwnerToken, &f, true, &Action::Hide, 104).expect("hide");
    assert!(s.hidden);
    let z = status_on(&conn, &p, &p.person_key("z3thon"), Viewer::Login(Role::Admin), &f, true, 105).expect("z3thon");
    assert!(z.hidden, "one person, one list");
    let j = status_on(&conn, &p, &p.person_key("julie"), Viewer::Login(Role::Admin), &f, true, 106).expect("julie");
    assert!(!j.hidden, "an unlinked Admin is their own person");

    // show clears hidden for the whole person.
    let s = apply_on(&conn, &p, OWNER, Viewer::Login(Role::Admin), &f, true, &Action::Show, 107).expect("show");
    assert!(!s.hidden);
    assert!(!status_on(&conn, &p, OWNER, Viewer::OwnerToken, &f, true, 108).expect("owner").hidden);
}

#[test]
fn the_dropped_k2_command_row_is_never_emitted_even_with_an_old_skip() {
    // k2_cli was dropped 11-10. A skip stored before then is harmless.
    let skipped: BTreeSet<String> = ["k2_cli".to_string()].into();
    let rs = rows(Viewer::OwnerToken, &Facts::default(), &skipped);
    assert_eq!(ids(&rs), V1_ROWS.to_vec());
    assert!(!ROW_IDS.contains(&"k2_cli"));
}

#[test]
fn skip_refuses_a_row_the_caller_does_not_see() {
    let conn = db();
    let p = people();
    let f = Facts::default();
    for bad in ["tunnel", "llm_sign_in", "own_sign_in", "meet_kessel", "k2_cli", "nope", ""] {
        let e = apply_on(&conn, &p, "anna", Viewer::Login(Role::Member), &f, true, &Action::Skip(bad.into()), 1)
            .expect_err(bad);
        assert_eq!(e, Error::UnknownRow(bad.to_string()));
    }
    let rec = load_record(&conn, "anna").expect("load").expect("the row was made");
    assert!(rec.skipped.is_empty(), "nothing was stored: {rec:?}");
}

#[test]
fn finishing_writes_once_and_stays_finished_after_a_row_comes_undone() {
    let conn = db();
    let p = people();
    let member = Viewer::Login(Role::Member);
    let s = status_on(&conn, &p, "anna", member, &Facts::default(), true, 10).expect("todo");
    assert!(!s.finished && !s.newly_finished);
    assert!(load_record(&conn, "anna").expect("load").is_none(), "an unfinished read writes nothing");

    let s = status_on(&conn, &p, "anna", member, &all_done(), true, 11).expect("done");
    assert!(s.finished && s.newly_finished);
    assert_eq!(load_record(&conn, "anna").expect("load").expect("row").finished_at, Some(11));

    let s = status_on(&conn, &p, "anna", member, &all_done(), true, 12).expect("again");
    assert!(s.finished && !s.newly_finished, "written once");
    assert_eq!(load_record(&conn, "anna").expect("load").expect("row").finished_at, Some(11));

    // The phone's session ended and the workspace went: still finished.
    let undone = Facts { has_agent: false, phones: 0, ..all_done() };
    let s = status_on(&conn, &p, "anna", member, &undone, true, 13).expect("undone");
    assert!(s.finished && !s.newly_finished);
    assert_eq!(row(&s.rows, "phone").state, State::Todo, "the rows still tell the truth");

    // Only show brings it back.
    let s = apply_on(&conn, &p, "anna", member, &undone, true, &Action::Show, 14).expect("show");
    assert!(!s.finished);
}

#[test]
fn show_brings_a_skipped_row_back_as_todo() {
    let conn = db();
    let p = people();
    let member = Viewer::Login(Role::Member);
    let f = Facts { has_agent: true, ..Facts::default() };
    let s = apply_on(&conn, &p, "anna", member, &f, true, &Action::Skip("phone".into()), 1).expect("skip");
    assert!(s.finished && s.newly_finished, "the skip finished the list");
    assert_eq!(row(&s.rows, "phone").state, State::Skipped);
    let s = apply_on(&conn, &p, "anna", member, &f, true, &Action::Show, 2).expect("show");
    assert!(!s.finished && !s.newly_finished, "show reopens it: {s:?}");
    assert_eq!(row(&s.rows, "phone").state, State::Todo, "the skipped row is to do again");
    assert_eq!(row(&s.rows, "first_agent").state, State::Done, "done rows stay done");
    // The next read does not finish it again.
    let s = status_on(&conn, &p, "anna", member, &f, true, 3).expect("read");
    assert!(!s.finished);
    assert_eq!(row(&s.rows, "phone").state, State::Todo);
    assert!(load_record(&conn, "anna").expect("load").expect("row").skipped.is_empty());
}

#[test]
fn skipping_the_last_open_row_finishes_the_list() {
    let conn = db();
    let p = people();
    let member = Viewer::Login(Role::Member);
    let f = Facts { has_agent: true, ..Facts::default() };
    let s = apply_on(&conn, &p, "gina", member, &f, false, &Action::Skip("phone".into()), 5).expect("skip");
    assert!(s.finished && s.newly_finished);
    assert!(!s.enabled, "the switch is reported as given");
}

#[test]
fn the_wire_shape_is_the_contracts() {
    let conn = db();
    let p = people();
    let s = status_on(&conn, &p, OWNER, Viewer::OwnerToken, &Facts { has_agent: true, ..Facts::default() }, false, 1)
        .expect("status");
    let v = serde_json::to_value(&s).expect("json");
    assert_eq!(
        v,
        serde_json::json!({
            "enabled": false,
            "hidden": false,
            "finished": false,
            "rows": [
                { "id": "llm_sign_in", "state": "todo", "opens": "llm_sign_in" },
                { "id": "first_agent", "state": "done", "opens": "add_workspace" },
                { "id": "tunnel", "state": "todo", "opens": "tunnel" },
                { "id": "phone", "state": "todo", "opens": "tunnel", "note": "needs_tunnel" },
                { "id": "own_sign_in", "state": "todo", "opens": "owner_account_prompt" }
            ]
        })
    );
}

// ── the existing-install cutoff ──────────────────────────────────────

fn upgraded(seed: &str) -> Connection {
    let conn = Connection::open(":memory:").expect("open");
    crate::db::run_migrations_through(&conn, Some("0288_mail_linked_by")).expect("0.46.11");
    crate::db::seed_audit_sentinels(&conn).expect("sentinels");
    conn.execute_batch(seed).expect("seed");
    crate::db::run_migrations(&conn).expect("upgrade");
    conn
}

#[test]
fn a_fresh_database_has_no_cutoff_and_everyone_starts_unfinished() {
    let conn = db();
    assert_eq!(cutoff(&conn).expect("cutoff"), None);
    let p = people();
    assert!(!treated_finished(None, &p, OWNER));
    let s = status_on(&conn, &p, OWNER, Viewer::OwnerToken, &Facts::default(), true, 1).expect("status");
    assert!(!s.finished);
    // The sentinels alone are not "in use".
    let conn = upgraded("");
    assert_eq!(cutoff(&conn).expect("cutoff"), None, "audit sentinels only");
}

#[test]
fn an_existing_server_marks_its_people_finished_and_later_logins_start_fresh() {
    for seed in [
        "INSERT INTO projects (id, name, path) VALUES ('p-old', 'old', '/tmp/p-old');",
        "INSERT INTO llm_accounts (id, tool, label, created_at) VALUES ('a1', 'claude', 'work', 1);",
    ] {
        let conn = upgraded(seed);
        let cut = cutoff(&conn).expect("cutoff").unwrap_or_else(|| panic!("no cutoff after {seed}"));
        assert!(cut > 0);
        let before = chrono::DateTime::<Utc>::from_timestamp(cut - 3600, 0).expect("ts");
        let after = chrono::DateTime::<Utc>::from_timestamp(cut + 3600, 0).expect("ts");
        let p = People {
            users: vec![user("old", Role::Member, before), user("new", Role::Member, after)],
            linked: vec![],
        };
        let member = Viewer::Login(Role::Member);
        let f = Facts::default();
        assert!(status_on(&conn, &p, OWNER, Viewer::OwnerToken, &f, true, cut + 10).expect("owner").finished);
        assert!(status_on(&conn, &p, "old", member, &f, true, cut + 10).expect("old").finished, "made before");
        assert!(!status_on(&conn, &p, "new", member, &f, true, cut + 10).expect("new").finished, "made after");
        // A row made for a pre-cutoff person (hide, skip) keeps them finished.
        let s = apply_on(&conn, &p, "old", member, &f, true, &Action::Skip("phone".into()), cut + 11).expect("skip");
        assert!(s.finished && !s.newly_finished, "{seed}");
        // Show brings the list back by hand.
        let s = apply_on(&conn, &p, "old", member, &f, true, &Action::Show, cut + 12).expect("show");
        assert!(!s.finished);
        // Running the migrations again changes nothing.
        crate::db::run_migrations(&conn).expect("again");
        assert_eq!(cutoff(&conn).expect("cutoff"), Some(cut));
    }
}

// ── the phone count, through the real session store ──────────────────

#[test]
fn phones_are_each_logins_own_and_the_owners_are_every_linked_logins() {
    let _home = crate::test_env::TempHome::new();
    for (name, role) in [("anna", Role::Member), ("gina", Role::Member), ("z3thon", Role::Admin)] {
        crate::connect_users::add_user(name, "password123").expect("add");
        if role != Role::Member {
            crate::connect_users::set_role(name, role).expect("role");
        }
    }
    let pair = |name: &str, dev: &str| {
        let epoch = crate::connect_users::live_user_epoch(name).expect("epoch");
        crate::connect_users::create_device_session(name, epoch, dev, "iPhone", "owner-token").expect("pair");
    };
    let unlinked = People::load().expect("people");
    assert_eq!(phones_for(&unlinked, OWNER).expect("owner"), 0, "the owner token with no linked login: todo");
    pair("anna", "dev-anna-1");
    pair("z3thon", "dev-z3-1");
    let unlinked = People::load().expect("people");
    assert_eq!(phones_for(&unlinked, "anna").expect("anna"), 1);
    assert_eq!(phones_for(&unlinked, "gina").expect("gina"), 0, "anna's phone is anna's");
    assert_eq!(phones_for(&unlinked, OWNER).expect("owner"), 0, "z3thon is not linked yet");
    crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["z3thon"] })).expect("link");
    let p = People::load().expect("people");
    assert_eq!(phones_for(&p, &p.person_key(OWNER)).expect("owner"), 1, "the linked login's phone");
    assert_eq!(phones_for(&p, &p.person_key("z3thon")).expect("z3thon"), 1, "the same person");

    // load_facts reads the same: the owner is linked, one phone, no tunnel.
    let conn = db();
    let settings = crate::app_settings::load();
    let f = load_facts(&conn, &p, &settings, OWNER).expect("facts");
    assert!(f.owner_linked);
    assert_eq!(f.phones, 1);
    assert!(!f.tunnel_done && !f.has_agent && !f.llm_signed_in, "{f:?}");
    let f = load_facts(&conn, &p, &settings, "gina").expect("facts");
    assert_eq!(f.phones, 0);
}

// ── Kessel at first run (prd-setup-checklist-v1 §10, kessel-first-run-contract) ──

fn kessel_live() -> crate::orchestrator::Live {
    crate::orchestrator::Live { workspace_id: "w-kessel".into(), name: "Kessel".into(), path: "/home/r/k2-agents/kessel".into() }
}

/// The flag on, a tool signed in, no Kessel: the offer's every condition.
fn offer_facts() -> Facts {
    Facts { kessel_first_run: true, llm_signed_in: true, kessel_default_path: "/home/r/k2-agents/kessel".into(), ..Facts::default() }
}

#[test]
fn the_offer_shows_only_with_the_flag_a_tool_no_kessel_and_an_admin_or_better() {
    let f = offer_facts();
    for v in [Viewer::OwnerToken, Viewer::Login(Role::Owner), Viewer::Login(Role::Admin)] {
        assert_eq!(kessel_status(v, &f, false).expect("on").offer, Some(OFFER_MEET_KESSEL), "{v:?}");
    }
    for v in [Viewer::Login(Role::Member), Viewer::Login(Role::VoiceChat)] {
        assert_eq!(kessel_status(v, &f, false).expect("on").offer, None, "{v:?}");
    }
    assert_eq!(kessel_status(Viewer::OwnerToken, &Facts { kessel_first_run: false, ..f.clone() }, false), None, "flag off: nothing");
    assert_eq!(kessel_status(Viewer::OwnerToken, &Facts { llm_signed_in: false, ..f.clone() }, false).expect("on").offer, None, "no tool");
    let with_kessel = Facts { kessel: Some(kessel_live()), ..f.clone() };
    let s = kessel_status(Viewer::OwnerToken, &with_kessel, false).expect("on");
    assert_eq!(s.offer, None, "Kessel is there");
    assert_eq!(s.kessel.expect("kessel").workspace_id, "w-kessel");
    assert_eq!(kessel_status(Viewer::OwnerToken, &f, true).expect("on").offer, None, "dismissed");
    let d = kessel_status(Viewer::OwnerToken, &f, false).expect("on").kessel_defaults;
    assert_eq!((d.name.as_str(), d.handle.as_str(), d.path.as_str()), ("Kessel", "kessel", "/home/r/k2-agents/kessel"));
}

#[test]
fn dismissing_the_offer_is_per_person_and_show_keeps_it() {
    let conn = db();
    let p = people();
    let f = offer_facts();
    let admin = Viewer::Login(Role::Admin);
    let s = apply_on(&conn, &p, "julie", admin, &f, true, &Action::DismissOffer, 5).expect("dismiss");
    assert_eq!(s.kessel.expect("on").offer, None, "julie said not now");
    let o = status_on(&conn, &p, OWNER, Viewer::OwnerToken, &f, true, 6).expect("owner");
    assert_eq!(o.kessel.expect("on").offer, Some(OFFER_MEET_KESSEL), "the owner still gets it");
    // The owner token and a linked login are one person.
    apply_on(&conn, &p, OWNER, Viewer::OwnerToken, &f, true, &Action::DismissOffer, 7).expect("dismiss");
    assert_eq!(status_on(&conn, &p, OWNER, Viewer::Login(Role::Admin), &f, true, 8).expect("z3thon").kessel.expect("on").offer, None);
    let s = apply_on(&conn, &p, "julie", admin, &f, true, &Action::Show, 9).expect("show");
    assert_eq!(s.kessel.expect("on").offer, None, "show brings the list back, not the offer");
    assert_eq!(load_record(&conn, "julie").expect("load").expect("row").offer_dismissed_at, Some(5));
}

#[test]
fn the_chief_of_staff_row_replaces_first_agent_only_with_the_flag_on() {
    let off = rows(Viewer::OwnerToken, &Facts::default(), &none());
    assert_eq!(ids(&off), V1_ROWS.to_vec(), "flag off: first_agent as today");
    let f = Facts { kessel_first_run: true, has_agent: true, ..Facts::default() };
    let on = rows(Viewer::OwnerToken, &f, &none());
    assert_eq!(ids(&on), KESSEL_ROWS.to_vec(), "same position");
    let cos = row(&on, "chief_of_staff");
    assert_eq!((cos.state, cos.opens, cos.note), (State::Todo, Some("kessel_sheet"), Some(NOTE_NEEDS_KESSEL)), "no Kessel yet");
    let with_kessel = Facts { kessel: Some(kessel_live()), ..f.clone() };
    let cos = row(&rows(Viewer::Login(Role::Admin), &with_kessel, &none()), "chief_of_staff").clone();
    assert_eq!((cos.state, cos.opens, cos.note), (State::Todo, Some("kessel_thread_cos"), None));
    let hired = Facts { chief_of_staff: true, ..with_kessel };
    assert_eq!(row(&rows(Viewer::OwnerToken, &hired, &none()), "chief_of_staff").state, State::Done);
    // A Member sees neither row.
    let m = rows(Viewer::Login(Role::Member), &f, &none());
    assert!(!ids(&m).contains(&"chief_of_staff") && !ids(&m).contains(&"first_agent"), "{m:?}");
    // Order: every id is known, in contract order.
    let order: Vec<usize> = on.iter().map(|r| ROW_IDS.iter().position(|id| *id == r.id).expect("known id")).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
}

#[test]
fn the_kessel_wire_shape_is_the_contracts_and_absent_with_the_flag_off() {
    let conn = db();
    let p = people();
    let s = status_on(&conn, &p, OWNER, Viewer::OwnerToken, &Facts { has_agent: true, ..Facts::default() }, false, 1).expect("off");
    let v = serde_json::to_value(&s).expect("json");
    for k in ["offer", "kessel", "kesselDefaults"] {
        assert!(v.get(k).is_none(), "{k} is not sent with the flag off: {v}");
    }
    let f = Facts { has_agent: true, tunnel_done: true, ..offer_facts() };
    let v = serde_json::to_value(status_on(&conn, &p, OWNER, Viewer::OwnerToken, &f, true, 2).expect("on")).expect("json");
    assert_eq!(v["offer"], serde_json::json!("meet_kessel"));
    assert_eq!(v["kessel"], serde_json::Value::Null);
    assert_eq!(v["kesselDefaults"], serde_json::json!({ "name": "Kessel", "handle": "kessel", "path": "/home/r/k2-agents/kessel" }));
    assert_eq!(v["rows"][1], serde_json::json!({ "id": "chief_of_staff", "state": "todo", "opens": "kessel_sheet", "note": "needs_kessel" }));
    let f = Facts { kessel: Some(kessel_live()), ..f };
    let v = serde_json::to_value(status_on(&conn, &p, OWNER, Viewer::OwnerToken, &f, true, 3).expect("on")).expect("json");
    assert_eq!(v["offer"], serde_json::Value::Null);
    assert_eq!(v["kessel"], serde_json::json!({ "workspaceId": "w-kessel", "name": "Kessel", "path": "/home/r/k2-agents/kessel" }));
    assert_eq!(v["rows"][1], serde_json::json!({ "id": "chief_of_staff", "state": "todo", "opens": "kessel_thread_cos" }));
}

/// The facts read the seat: Kessel doesn't count as the person's first agent,
/// a live Chief of Staff is, and a deleted Kessel workspace empties the seat.
#[test]
fn load_facts_reads_the_seat_and_a_deleted_kessel_clears_it() {
    let _home = crate::test_env::TempHome::new();
    let conn = db();
    let p = People::load().expect("people");
    let mut settings = crate::app_settings::AppSettings::default();
    settings.kessel_first_run = true;
    for (id, path) in [("w-kessel", "/tmp/fr-kessel"), ("w-cos", "/tmp/fr-cos")] {
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?1, ?2)", rusqlite::params![id, path]).expect("project");
    }
    conn.execute("DELETE FROM projects WHERE id = 'w-cos'", []).expect("no cos yet");
    crate::orchestrator::record_kessel(&conn, "w-kessel", "Kessel", "owner", 1).expect("seat");
    let f = load_facts(&conn, &p, &settings, OWNER).expect("facts");
    assert!(f.kessel_first_run);
    assert_eq!(f.kessel.as_ref().map(|l| l.workspace_id.as_str()), Some("w-kessel"));
    assert!(!f.has_agent, "Kessel alone isn't the person's first agent");
    assert!(!f.chief_of_staff);
    assert!(f.kessel_default_path.ends_with("k2-agents/kessel"), "{}", f.kessel_default_path);

    conn.execute("INSERT INTO projects (id, name, path) VALUES ('w-cos', 'w-cos', '/tmp/fr-cos')", []).expect("cos");
    crate::orchestrator::record_chief_of_staff(&conn, "w-cos").expect("record");
    let f = load_facts(&conn, &p, &settings, OWNER).expect("facts");
    assert!(f.chief_of_staff && f.has_agent);
    let rs = rows(Viewer::OwnerToken, &f, &none());
    assert_eq!(row(&rs, "chief_of_staff").state, State::Done);

    conn.execute("DELETE FROM projects WHERE id = 'w-kessel'", []).expect("delete Kessel");
    let f = load_facts(&conn, &p, &settings, OWNER).expect("facts");
    assert_eq!(f.kessel, None, "a deleted Kessel workspace clears the seat");
    let s = kessel_status(Viewer::OwnerToken, &Facts { llm_signed_in: true, ..f.clone() }, false).expect("on");
    assert_eq!(s.offer, Some(OFFER_MEET_KESSEL), "the offer comes back");
    assert_eq!(row(&rows(Viewer::OwnerToken, &f, &none()), "chief_of_staff").opens, Some("kessel_sheet"));
}

#[test]
fn signed_in_tools_are_each_tools_own() {
    use crate::llm_accounts::{kind, state};
    let es = [
        entry("claude", kind::SUBSCRIPTION, state::NEEDS_LOGIN),
        entry("codex", kind::SUBSCRIPTION, state::SIGNED_IN),
        entry("grok", kind::API_KEY, state::NOT_SET_UP),
        entry("gemini", kind::SUBSCRIPTION, state::SIGNED_IN),
    ];
    assert_eq!(signed_in_tools(&es), vec!["codex", "grok"]);
}

// ── developer tools (dev_tools, 0.46.12) ─────────────────────────────

fn ready() -> crate::git_safe::GitProbe {
    crate::git_safe::GitProbe::Ready { program: "/usr/bin/git".into(), version: "git version 2.50.1".into() }
}

#[test]
fn the_dev_tools_fact_comes_from_the_probe_the_os_and_the_install() {
    use crate::git_safe::GitProbe;
    use std::time::{Duration as D, SystemTime};
    let now = SystemTime::now();
    let just = Some(now - D::from_secs(5));
    let long_ago = Some(now - D::from_secs(2 * 60 * 60));
    let clt = GitProbe::NeedsAppleClt;
    let f = |p: &GitProbe, os: &str, started, running| dev_tools_from(p, os, started, now, running, "ID=ubuntu\n");
    assert_eq!(f(&ready(), "macos", None, false), DevTools::NoRow, "git runs: no row");
    assert_eq!(f(&clt, "macos", None, false), DevTools::AppleMissing { installing: false });
    assert_eq!(f(&clt, "macos", just, false), DevTools::AppleMissing { installing: true }, "just clicked");
    assert_eq!(f(&clt, "macos", long_ago, true), DevTools::AppleMissing { installing: true }, "the installer is up");
    assert_eq!(f(&clt, "macos", long_ago, false), DevTools::AppleMissing { installing: false }, "closed: install again");
    assert_eq!(f(&ready(), "macos", long_ago, false), DevTools::AppleDone, "installed here: the row reads done");
    let broken = GitProbe::Broken { program: "/usr/bin/git".into(), error: "xcodebuild -license".into() };
    assert_eq!(f(&broken, "macos", None, false), DevTools::NoRow, "installing won't fix a broken git");
    assert_eq!(f(&GitProbe::Missing, "linux", None, false), DevTools::LinuxMissing { command: "sudo apt install git" });
    assert_eq!(
        dev_tools_from(&GitProbe::Missing, "linux", None, now, false, ""),
        DevTools::LinuxMissing { command: "install git with your package manager (apt, dnf or pacman)" }
    );
    assert_eq!(f(&ready(), "linux", None, false), DevTools::NoRow);
    assert_eq!(f(&GitProbe::Missing, "windows", None, false), DevTools::NoRow, "no step on Windows");
}

#[test]
fn the_dev_tools_row_is_absent_while_git_runs() {
    let rs = rows(Viewer::OwnerToken, &Facts { dev_tools: DevTools::NoRow, ..Facts::default() }, &none());
    assert!(!ids(&rs).contains(&"dev_tools"), "{rs:?}");
}

#[test]
fn on_a_mac_the_dev_tools_row_is_first_todo_and_the_owner_tokens_only() {
    let f = Facts { dev_tools: DevTools::AppleMissing { installing: false }, ..Facts::default() };
    let rs = rows(Viewer::OwnerToken, &f, &none());
    assert_eq!(ids(&rs), vec!["dev_tools", "llm_sign_in", "first_agent", "tunnel", "phone", "own_sign_in"]);
    assert_eq!(rs[0], Row { id: "dev_tools", state: State::Todo, opens: Some("dev_tools"), note: None });
    // Apple's installer opens on the daemon's own screen: no login gets it.
    for role in [Role::Owner, Role::Admin, Role::Member] {
        assert!(!ids(&rows(Viewer::Login(role), &f, &none())).contains(&"dev_tools"), "{role:?}");
    }
    // In contract order with the Kessel rows too.
    let on = rows(Viewer::OwnerToken, &Facts { kessel_first_run: true, ..f.clone() }, &none());
    assert_eq!(ids(&on)[..2].to_vec(), vec!["dev_tools", "llm_sign_in"]);
    let order: Vec<usize> = on.iter().map(|r| ROW_IDS.iter().position(|id| *id == r.id).expect("known id")).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
}

#[test]
fn the_dev_tools_row_says_installing_then_reads_done() {
    let f = Facts { dev_tools: DevTools::AppleMissing { installing: true }, ..Facts::default() };
    let r = row(&rows(Viewer::OwnerToken, &f, &none()), "dev_tools").clone();
    assert_eq!((r.state, r.opens, r.note), (State::Todo, Some("dev_tools"), Some(NOTE_INSTALLING)));
    let f = Facts { dev_tools: DevTools::AppleDone, ..Facts::default() };
    let r = row(&rows(Viewer::OwnerToken, &f, &none()), "dev_tools").clone();
    assert_eq!((r.state, r.opens, r.note), (State::Done, Some("dev_tools"), None));
    assert!(!ids(&rows(Viewer::Login(Role::Owner), &f, &none())).contains(&"dev_tools"));
}

#[test]
fn on_linux_the_dev_tools_row_carries_the_command_and_opens_nothing() {
    let f = Facts { dev_tools: DevTools::LinuxMissing { command: "sudo apt install git" }, ..Facts::default() };
    for v in [Viewer::OwnerToken, Viewer::Login(Role::Owner), Viewer::Login(Role::Admin)] {
        let rs = rows(v, &f, &none());
        assert_eq!(rs[0], Row { id: "dev_tools", state: State::Todo, opens: None, note: Some("sudo apt install git") }, "{v:?}");
    }
    assert!(!ids(&rows(Viewer::Login(Role::Member), &f, &none())).contains(&"dev_tools"));
    let json = serde_json::to_value(&rows(Viewer::OwnerToken, &f, &none())[0]).expect("json");
    assert_eq!(json, serde_json::json!({ "id": "dev_tools", "state": "todo", "opens": null, "note": "sudo apt install git" }));
}

#[test]
fn the_dev_tools_row_counts_toward_finished_and_can_be_skipped() {
    let conn = db();
    let p = people();
    let missing = Facts { dev_tools: DevTools::AppleMissing { installing: false }, ..all_done() };
    let s = status_on(&conn, &p, OWNER, Viewer::OwnerToken, &missing, true, 10).expect("status");
    assert!(!s.finished, "every other row done, dev_tools to do: {s:?}");
    let s = apply_on(&conn, &p, OWNER, Viewer::OwnerToken, &missing, true, &Action::Skip("dev_tools".into()), 11)
        .expect("skip");
    assert_eq!(row(&s.rows, "dev_tools").state, State::Skipped);
    assert!(s.finished && s.newly_finished, "skipped counts: {s:?}");

    let conn = db();
    let done = Facts { dev_tools: DevTools::AppleDone, ..all_done() };
    let s = status_on(&conn, &p, OWNER, Viewer::OwnerToken, &done, true, 12).expect("status");
    assert!(s.finished, "done counts: {s:?}");

    // A login can't skip a row it doesn't see.
    let conn = db();
    let e = apply_on(&conn, &p, "julie", Viewer::Login(Role::Admin), &missing, true, &Action::Skip("dev_tools".into()), 13);
    assert_eq!(e, Err(Error::UnknownRow("dev_tools".into())));
}

#[test]
fn load_facts_takes_an_injected_dev_tools_fact() {
    let d = DevTools::LinuxMissing { command: "sudo dnf install git" };
    {
        let _g = dev_tools_for_test(d);
        assert_eq!(dev_tools_now(), d);
        {
            let _inner = dev_tools_for_test(DevTools::AppleDone);
            assert_eq!(dev_tools_now(), DevTools::AppleDone);
        }
        assert_eq!(dev_tools_now(), d, "the inner guard restores the outer one");
    }
}
