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
    Facts { llm_signed_in: true, has_agent: true, tunnel_done: true, airgap: false, owner_linked: true, phones: 1 }
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
    let opens: Vec<(&str, &str)> = rs.iter().map(|r| (r.id, r.opens)).collect();
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
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), ("tunnel", Some(NOTE_NEEDS_TUNNEL)));
    // A Member can't see the tunnel row: the phone page, with the note.
    let rs = rows(Viewer::Login(Role::Member), &Facts::default(), &none());
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), ("phone", Some(NOTE_NEEDS_TUNNEL)));
    // The owner skipped the tunnel: not todo any more, so the phone page.
    let skipped: BTreeSet<String> = ["tunnel".to_string()].into();
    let rs = rows(Viewer::OwnerToken, &Facts::default(), &skipped);
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), ("phone", Some(NOTE_NEEDS_TUNNEL)));
    // Tunnel on: the phone page, no note.
    let rs = rows(Viewer::Login(Role::Member), &Facts { tunnel_done: true, ..Facts::default() }, &none());
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), ("phone", None));
    // A paired phone needs no note.
    let rs = rows(Viewer::OwnerToken, &Facts { phones: 1, ..Facts::default() }, &none());
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), ("phone", None));
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
    assert_eq!((row(&rs, "phone").opens, row(&rs, "phone").note), ("phone", None));
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
fn skip_refuses_a_row_the_caller_does_not_see() {
    let conn = db();
    let p = people();
    let f = Facts::default();
    for bad in ["tunnel", "llm_sign_in", "own_sign_in", "meet_k5", "k2_cli", "nope", ""] {
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
    crate::db::run_migrations_through(&conn, Some("0286_compute_key_chain")).expect("0.46.11");
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
