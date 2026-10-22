//! Lane E1 tests (PRD rev 3.1 §13.2, §13.3): the Agent-tab switch actions,
//! their `to=off` twins, the connection actions and `garden.keep-mine`, end
//! to end through the real core (request → card → a person's answer →
//! the Settings route's own entry), plus the GardenOwner resolver and the
//! Settings widening. The gate is passed (`routes::handle_gated`); the
//! switch paths and the two HTTP doors run in
//! `tests/permission_switches_integration.rs` (no `set_server_switch` in
//! lib tests). Fail loudly; no test wakes a real agent.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::permission::catalog::{Direction, Floor, Grant, Undo, WallCoverage, WallMatch};
use serde_json::{json, Value};

use super::{Writer, NOT_CARDABLE, SWITCHES};
use crate::compute_routes::Who;
use crate::permission::actions::{self, Answerer, Applied, Check, Ctx};
use crate::permission::routes::{self, handle_gated};
use crate::sidecar_routes::Caller;

// ── fixtures ────────────────────────────────────────────────────────

/// A temp HOME (no real `~/.k2`, app settings or audit file is read or
/// written) and this thread's own DB (the server-wide pending cap and the
/// logins list see only this test's rows).
struct Env {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    /// mail.send-mode reads the singleton `mail_server` row. Taken first
    /// (the order every mail test uses: the mail lock, then the HOME lock).
    _mail: crate::mail::MailServerTestGuard,
}

fn env() -> Env {
    let mail = crate::mail::mail_server_test_lock();
    let home = crate::test_support::TempHome::new();
    Env { _db: k2_core::db::scoped_for_test(), _home: home, _mail: mail }
}

struct Ws {
    id: String,
    handle: String,
    path: String,
}

/// A workspace (a real folder under the temp HOME) with its pinned Chat.
fn seed(tag: &str) -> Ws {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let home = std::env::var("HOME").expect("HOME (TempHome)");
    let path = format!("{home}/ws/{handle}");
    std::fs::create_dir_all(&path).expect("workspace folder");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path])
        .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, path }
}

fn login(name: &str, role: Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password-for-e1-9").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

fn as_login(user: &str) -> Answerer {
    let role = k2_core::connect_users::role_for_user(user).expect("a login");
    Answerer::Login { user: user.to_string(), role: format!("{role:?}").to_ascii_lowercase() }
}

fn ask(ws: &Ws, action: &str, params: Value) -> (String, Value) {
    let who = Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) };
    let body = json!({ "action": action, "params": params, "why": "Run the suite on the minis" });
    let r = handle_gated(routes::REQUEST, &who, true, body.to_string().as_bytes(), &HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

/// The card and the request id, as `<card>#<request>`: asks close
/// together share a batch card, whose answers name the item.
fn ask_card(ws: &Ws, action: &str, params: Value) -> String {
    let (s, v) = ask(ws, action, params);
    assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("pending")), "{action}: {v}");
    let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}"));
    let id = v["id"].as_str().unwrap_or_else(|| panic!("no id: {v}"));
    format!("{card}#{id}")
}

fn answer(author: &str, ws: &Ws, card: &str, label: &str) -> (String, Value) {
    let (card, item) = card.split_once('#').unwrap_or_else(|| panic!("a card from ask_card: {card}"));
    // `item` is read on a batch card only; a single card ignores it.
    let b = json!({ "addr": ws.handle, "id": card, "answer": label, "item": item });
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), b.to_string().as_bytes(), author);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

/// The state this answer left the card's request in (`decided[]`: a
/// batch card's `result` is the batch's, not the item's).
fn decided(a: &Value, card: &str) -> String {
    let (_, id) = card.split_once('#').unwrap_or_else(|| panic!("a card from ask_card: {card}"));
    a["decided"]
        .as_array()
        .and_then(|d| d.iter().find(|x| x["id"] == id))
        .and_then(|x| x["state"].as_str())
        .unwrap_or_else(|| panic!("{id} was not decided: {a}"))
        .to_string()
}

/// Ask, then Allow as `author`; the request must end `approved`.
fn ask_and_allow(ws: &Ws, action: &str, params: Value, author: &str) -> Value {
    let card = ask_card(ws, action, params);
    let (s, a) = answer(author, ws, &card, "Allow");
    assert_eq!(s, "200 OK", "{action} by {author}: {a}");
    assert_eq!(decided(&a, &card), "approved", "{action} by {author}: {a}");
    a
}

fn ctx(ws: &Ws, params: Value) -> Ctx {
    Ctx { workspace_id: ws.id.clone(), workspace_handle: ws.handle.clone(), params: params.as_object().cloned().unwrap_or_default() }
}

fn check(action: &str, ws: &Ws, params: Value) -> Check {
    let r = actions::find(action).unwrap_or_else(|| panic!("{action} is not registered"));
    (r.check)(&ctx(ws, params))
}

fn apply(action: &str, ws: &Ws, params: Value, who: &Answerer) -> Applied {
    let r = actions::find(action).unwrap_or_else(|| panic!("{action} is not registered"));
    (r.apply)(&ctx(ws, params), who, None)
}

/// The values each switch is asked with in these tests.
fn params_for(id: &str) -> Value {
    match id {
        "apps.core" | "apps.core-off" => json!({ "core": "db" }),
        "mail.send-mode" => json!({ "mode": "on" }),
        "db.agent-access" => json!({ "level": "write" }),
        _ => json!({}),
    }
}

/// The Agent tab's state for a switch, read the way Settings reads it.
fn state(w: Writer, ws: &Ws) -> bool {
    match w {
        Writer::Access("compute") => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::compute::store::agents_can_use_compute(&conn, &ws.id)
        }
        Writer::Access("compute_operate") => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::compute::admin::agents_can_operate_compute(&conn, &ws.id)
        }
        Writer::Access("compute_manage") => k2_core::workspace::settings::agents_can_manage_compute(&ws.id),
        Writer::Access("agents_manage") => k2_core::workspace::settings::agents_can_manage_agents(&ws.id),
        Writer::Access("projects_manage") => k2_core::workspace::settings::agents_can_manage_projects(&ws.id),
        Writer::Access("chat_manage") => k2_core::workspace::settings::agents_can_manage_chat(&ws.id),
        Writer::Access("vault_manage") => k2_core::workspace::settings::agents_can_manage_vault(&ws.id),
        Writer::Access("gardens_manage") => k2_core::workspace::settings::agents_can_manage_gardens(&ws.id),
        Writer::Access("apps_build") => crate::apps::access::level(&ws.id) >= crate::apps::access::Level::Build,
        Writer::Access("apps_manage_full") => crate::apps::access::level(&ws.id) == crate::apps::access::Level::Full,
        Writer::Access(other) => panic!("no state reader for {other}"),
        Writer::AppCore => crate::apps::cores::ticked(&ws.id).iter().any(|c| c == "db"),
        Writer::Dns => k2_core::workspace::settings::get_dns_manage_enabled(&ws.path),
        Writer::MailManage => k2_core::workspace::settings::mail_manage_allowed_for_path(&ws.path),
        Writer::MailSendMode => k2_core::workspace::settings::mail_agent_send_for_path(&ws.path) == "on",
        Writer::DbAccess => k2_core::workspace::settings::db_agent_access_for_path(&ws.path) == "write",
    }
}

/// Turn the switch's prerequisite on first (switch 2 sits under switch 1).
fn prerequisite(w: Writer, ws: &Ws) {
    if w == Writer::Access("apps_manage_full") {
        ask_and_allow(ws, "apps.build", json!({}), "owner");
    }
}

// ── the registry ────────────────────────────────────────────────────

const SWITCH_IDS: [&str; 15] = [
    "compute.use",
    "compute.operate",
    "compute.manage",
    "agents.manage",
    "projects.manage",
    "apps.build",
    "apps.manage-full",
    "apps.core",
    "chat.manage",
    "vault.manage",
    "gardens.manage",
    "dns.manage",
    "mail.manage",
    "mail.send-mode",
    "db.agent-access",
];

#[test]
fn e1_registers_17_switch_actions_their_off_twins_and_keep_as_mine() {
    let all = actions::all();
    let ids: Vec<&str> = all.iter().map(|r| r.spec.id).collect();
    let mut want: Vec<String> = Vec::new();
    for id in SWITCH_IDS.iter().chain(["connections.create"].iter()) {
        want.push(id.to_string());
        want.push(format!("{id}-off"));
    }
    want.push("connections.add".into());
    want.push("garden.keep-mine".into());
    for w in &want {
        assert!(ids.contains(&w.as_str()), "{w} is not registered: {ids:?}");
    }
    // 17 card-able switch actions (PRD §5.2 with vault and DNS kept) + the
    // to=off twins (one per standing switch) + garden.keep-mine.
    let e1: Vec<&str> = ids.iter().copied().filter(|i| want.iter().any(|w| w == i)).collect();
    assert_eq!(e1.len(), 17 + 16 + 1, "{e1:?}");
    for r in &all {
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn every_standing_switch_undoes_through_its_tighten_twin() {
    for sw in SWITCHES.iter() {
        let Undo::Twin { action } = sw.on.undo else { panic!("{} has no twin", sw.on.id) };
        assert_eq!(action, sw.off.id, "{}", sw.on.id);
        assert_eq!(format!("{}-off", sw.on.id), sw.off.id);
        assert_eq!((sw.on.direction, sw.off.direction), (Direction::Loosen, Direction::Tighten), "{}", sw.on.id);
        assert_eq!((sw.on.grant, sw.off.grant), (Grant::Standing, Grant::Standing));
        assert_eq!(sw.off.undo, Undo::None);
        assert_eq!((sw.off.coverage, sw.off.walls.len()), (WallCoverage::None, 0), "a twin is asked, never a wall");
        assert_eq!(sw.on.words.setting, sw.off.words.setting, "the same Settings control");
        // The twin takes the switch's own values (A2's undo passes them).
        let keys = |s: &k2_core::permission::catalog::ActionSpec| s.params.iter().map(|p| p.key).collect::<Vec<_>>();
        assert_eq!(keys(&sw.on), keys(&sw.off), "{}", sw.on.id);
        assert!(actions::find(sw.off.id).is_some());
    }
    let c = actions::find("connections.create").expect("registered");
    assert_eq!(c.spec.undo, Undo::Twin { action: "connections.create-off" });
    let add = actions::find("connections.add").expect("registered");
    assert_eq!((add.spec.grant, add.spec.undo), (Grant::OneTime, Undo::None));
}

/// The floors are the code's (Appendix A, catalog-check): Admin, except
/// `compute.manage` (Owner both ways) and the Apps raises (the owner
/// raises, an Admin lowers); Keep as mine is the Garden's owner.
#[test]
fn the_floors_are_the_codes() {
    let floor = |id: &str| actions::find(id).unwrap_or_else(|| panic!("{id}")).spec.floor;
    for id in ["compute.manage", "compute.manage-off", "apps.build", "apps.manage-full", "apps.core"] {
        assert_eq!(floor(id), Floor::Owner, "{id}");
    }
    for id in ["apps.build-off", "apps.manage-full-off", "apps.core-off", "connections.add", "connections.create", "mail.send-mode"] {
        assert_eq!(floor(id), Floor::Admin, "{id}");
    }
    for id in ["compute.use", "compute.operate", "agents.manage", "projects.manage", "chat.manage", "vault.manage", "gardens.manage", "dns.manage", "mail.manage", "db.agent-access"] {
        assert_eq!((floor(id), floor(&format!("{id}-off"))), (Floor::Admin, Floor::Admin), "{id}");
    }
    assert_eq!(floor("garden.keep-mine"), Floor::GardenOwner);
}

/// Each catalog wall is one lane C's walls map teaches (same action, same
/// match); the Apps holds teach `apps.approve`, so switch 2 and the cores
/// have none of their own.
#[test]
fn the_walls_match_the_refusals() {
    let code = |id: &str| -> Vec<&'static str> {
        actions::find(id)
            .unwrap_or_else(|| panic!("{id}"))
            .spec
            .walls
            .iter()
            .map(|w| match w.matches {
                WallMatch::Code { status: 403, code } => code,
                other => panic!("{id}: unexpected wall {other:?}"),
            })
            .collect()
    };
    assert_eq!(code("compute.use"), ["compute_off"]);
    assert_eq!(code("compute.operate"), ["operate_off"]);
    assert_eq!(code("compute.manage"), ["owner_only"]);
    assert_eq!(code("agents.manage"), ["gated"]);
    assert_eq!(code("projects.manage"), ["agent_permission_off"]);
    assert_eq!(code("apps.build"), ["apps_manage_off"]);
    assert!(code("apps.manage-full").is_empty() && code("apps.core").is_empty());
    assert_eq!(code("chat.manage"), ["chat_manage_off"]);
    assert_eq!(code("vault.manage"), ["vault_manage_required"]);
    assert_eq!(code("gardens.manage"), ["person_only", "person_only"]);
    assert_eq!(code("dns.manage"), ["dns_manage_disabled"]);
    assert_eq!(code("connections.create"), ["agents_create_connections_disabled"]);
    assert_eq!(code("connections.add"), ["agents_create_connections_disabled", "not_connected"]);
    assert_eq!(code("mail.manage"), ["owner_only"]);
    assert_eq!(code("mail.send-mode"), ["gated"]);
    assert_eq!(code("db.agent-access"), ["forbidden"]);
    assert!(code("garden.keep-mine").is_empty());
}

#[test]
fn rows_outside_v1_say_it_cant_be_requested_or_name_their_cover() {
    let _env = env();
    let ws = seed("e1nc");
    for (id, covered) in [("domain.apex.add", Some("dns.manage")), ("compute.grant", Some("compute.use"))] {
        let (s, v) = ask(&ws, id, json!({}));
        assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("not_cardable")), "{id}: {v}");
        assert_eq!(v["permission"]["action"].as_str(), covered, "{id}: {v}");
    }
    // Rosson (2026-10-21): agents can't ask for wider roles or clones.
    for id in ["app.role-caps", "clone.approve", "garden.lock", "garden.widget.resume", "sources.bind-secret"] {
        let (s, v) = ask(&ws, id, json!({}));
        assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("not_cardable")), "{id}: {v}");
        assert_eq!(v["permission"]["requestable"], false, "{id}: {v}");
        assert!(v["error"]["hint"].as_str().is_some_and(|h| h.ends_with("It can't be requested.")), "{id}: {v}");
    }
    assert!(NOT_CARDABLE.iter().all(|n| actions::find(n.id).is_none()));
}

/// Every entry names a classified route (the floor `route_floor` re-checks).
#[test]
fn every_writer_route_is_classified_and_at_least_admin_for_logins() {
    for sw in SWITCHES.iter() {
        let path = super::writer_route(sw.writer);
        let row = crate::routes::route_policy::lookup(path).unwrap_or_else(|| panic!("{path} unclassified"));
        assert!(row.post.is_some(), "{path} is a POST writer");
    }
    for path in ["/cli/agents-create-connections", "/cli/connections"] {
        assert!(crate::routes::route_policy::lookup(path).is_some(), "{path}");
    }
}

// ── each switch, through the card, as the owner ─────────────────────

/// Allow applies the switch through its Settings entry; asking again is
/// `already_done`; the `-off` twin turns it off again. projects.manage
/// starts on (its default), so its loosen ask is already done.
#[test]
fn every_switch_turns_on_through_the_card_and_off_through_its_twin() {
    let _env = env();
    for sw in SWITCHES.iter() {
        let ws = seed("e1sw");
        let p = params_for(sw.on.id);
        prerequisite(sw.writer, &ws);
        if sw.writer == Writer::Access("projects_manage") {
            assert!(state(sw.writer, &ws), "Manage K2 projects is on by default");
            let (s, v) = ask(&ws, sw.on.id, p.clone());
            assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("already_done")), "{v}");
        } else {
            assert!(!state(sw.writer, &ws), "{} starts off", sw.on.id);
            ask_and_allow(&ws, sw.on.id, p.clone(), "owner");
            assert!(state(sw.writer, &ws), "{} is on after Allow", sw.on.id);
            let (s, v) = ask(&ws, sw.on.id, p.clone());
            assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("already_done")), "{}: {v}", sw.on.id);
        }
        let a = ask_and_allow(&ws, sw.off.id, params_for(sw.off.id), "owner");
        assert!(!state(sw.writer, &ws), "{} is off after its twin: {a}", sw.on.id);
        assert_eq!(check(sw.off.id, &ws, params_for(sw.off.id)), Check::AlreadyDone, "{}", sw.off.id);
    }
}

// ── floors at the click ─────────────────────────────────────────────

#[test]
fn an_admin_answers_admin_rows_and_lowers_the_owners_but_never_raises_them() {
    let _env = env();
    let admin = login("e1adm", Role::Admin);
    let member = login("e1mem", Role::Member);
    let ws = seed("e1fl");
    // Admin row: a Member is refused at the card, an Admin applies it.
    let card = ask_card(&ws, "compute.use", json!({}));
    let (s, v) = answer(&member, &ws, &card, "Allow");
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("action_card_person_only")), "{v}");
    assert!(!state(Writer::Access("compute"), &ws), "a refused answer applies nothing");
    let (s, v) = answer(&admin, &ws, &card, "Allow");
    assert_eq!((s.as_str(), v["action"]["result"]["outcome"].as_str()), ("200 OK", Some("approved")), "{v}");
    assert!(state(Writer::Access("compute"), &ws));
    // Owner rows: the Admin is refused at the card (both ways for compute).
    let mut cards = HashMap::new();
    for id in ["compute.manage", "apps.build"] {
        let card = ask_card(&ws, id, json!({}));
        let (s, v) = answer(&admin, &ws, &card, "Allow");
        assert_eq!((s.as_str(), v["error"]["required"].as_str()), ("403 Forbidden", Some("owner")), "{id}: {v}");
        cards.insert(id, card);
    }
    assert!(!state(Writer::Access("compute_manage"), &ws) && !state(Writer::Access("apps_build"), &ws));
    // An Owner login (the phone) raises switch 1 on the same card; an
    // Admin may lower it.
    let owner = login("e1own", Role::Owner);
    let (s, v) = answer(&owner, &ws, &cards["apps.build"], "Allow");
    assert_eq!((s.as_str(), decided(&v, &cards["apps.build"]).as_str()), ("200 OK", "approved"), "{v}");
    assert!(state(Writer::Access("apps_build"), &ws));
    ask_and_allow(&ws, "apps.build-off", json!({}), &admin);
    assert!(!state(Writer::Access("apps_build"), &ws), "an Admin lowers switch 1");
}

/// The route's own rules run at the click, not only the catalog floor: an
/// Admin calling the Apps raise or compute-manage entry (as if the floor
/// check above were skipped) is refused by the writer itself, and a Member
/// is refused by the route floor with Settings' own `role_required`.
#[test]
fn the_entries_rerun_the_routes_own_checks() {
    let _env = env();
    let admin = as_login(&login("e1adm2", Role::Admin));
    let member = as_login(&login("e1mem2", Role::Member));
    let ws = seed("e1en");
    for id in ["apps.build", "compute.manage", "compute.manage-off"] {
        match apply(id, &ws, json!({}), &admin) {
            Applied::Refused { code, .. } => assert_eq!(code, "owner_only", "{id}"),
            other => panic!("{id}: an Admin must be refused by the writer: {other:?}"),
        }
    }
    for sw in SWITCHES.iter() {
        match apply(sw.on.id, &ws, params_for(sw.on.id), &member) {
            Applied::Refused { code, .. } => assert_eq!(code, "role_required", "{}", sw.on.id),
            other => panic!("{}: a Member must be refused: {other:?}", sw.on.id),
        }
    }
    match apply("connections.create", &ws, json!({}), &member) {
        Applied::Refused { code, .. } => assert_eq!(code, "role_required"),
        other => panic!("{other:?}"),
    }
    // connections.add: the route floor is Member, the add privileged only
    // for owner-or-admin: a Member hears the route's own refusal.
    let other = seed("e1en2");
    match apply("connections.add", &ws, json!({ "to": other.handle }), &member) {
        Applied::Refused { code, .. } => assert_eq!(code, "agents_create_connections_disabled"),
        r => panic!("{r:?}"),
    }
    assert!(!k2_core::connections::are_local_peers(&ws.id, &other.id), "nothing applied");
}

/// The Settings door and the card door store the same thing (parity,
/// §6.4): the Settings route as an Admin login on one workspace, the card
/// as an Admin login on another.
#[test]
fn the_card_and_the_settings_route_store_the_same_state() {
    let _env = env();
    let user = login("e1par", Role::Admin);
    let admin = as_login(&user);
    for sw in SWITCHES.iter().filter(|s| s.on.floor == Floor::Admin && s.writer != Writer::Access("projects_manage")) {
        let (a, b) = (seed("e1pa"), seed("e1pb"));
        // The card door.
        assert!(matches!(apply(sw.on.id, &a, params_for(sw.on.id), &admin), Applied::Done { .. }), "{}", sw.on.id);
        // The Settings door: the route's own handler as that login.
        let r = settings_door(sw.writer, &user, &b, true);
        assert!(r.status.starts_with('2'), "{}: {} {}", sw.on.id, r.status, r.body);
        assert!(state(sw.writer, &a) && state(sw.writer, &b), "{}", sw.on.id);
        assert!(matches!(apply(sw.off.id, &a, params_for(sw.off.id), &admin), Applied::Done { .. }), "{}", sw.off.id);
        let r = settings_door(sw.writer, &user, &b, false);
        assert!(r.status.starts_with('2'), "{}: {} {}", sw.off.id, r.status, r.body);
        assert!(!state(sw.writer, &a) && !state(sw.writer, &b), "{}", sw.off.id);
    }
}

/// What Settings sends for each switch, through the route's handler (the
/// route floor is the login's role here: `role_gate` passed for Admin).
fn settings_door(w: Writer, user: &str, ws: &Ws, on: bool) -> crate::cli_response::CliResponse {
    let login = Caller::Login { username: user.to_string() };
    match w {
        Writer::Access(toggle) => crate::sidecar_routes::handle(
            "/cli/agent-access/set",
            login,
            json!({ "toggle": toggle, "workspace": ws.id, "value": on }).to_string().as_bytes(),
            &HashMap::new(),
            "test",
        ),
        Writer::AppCore => crate::sidecar_routes::handle(
            "/cli/agent-access/set",
            login,
            json!({ "toggle": "app_core", "workspace": ws.id, "core": "db", "value": on }).to_string().as_bytes(),
            &HashMap::new(),
            "test",
        ),
        Writer::Dns => {
            let p: HashMap<String, String> =
                [("project", ws.path.as_str()), ("enable", if on { "1" } else { "0" })].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            crate::misc_routes::dispatch("/cli/dns-manage", &p).expect("dns-manage arm")
        }
        Writer::MailManage => crate::mail_routes::handle_mail_manage(json!({ "project": ws.path, "enable": i64::from(on) }).to_string().as_bytes()),
        Writer::MailSendMode => crate::mail::routes_server::handle_config_set(
            json!({ "workspace": ws.path, "agentSend": if on { "on" } else { "off" } }).to_string().as_bytes(),
        ),
        Writer::DbAccess => crate::workspace_routes::handle_workspace_set(
            json!({ "project": ws.path, "fields": { "db_agent_access": if on { "write" } else { "off" } } }).to_string().as_bytes(),
        ),
    }
}

// ── values and the current state ────────────────────────────────────

#[test]
fn values_are_checked_and_switch_two_needs_switch_one() {
    let _env = env();
    let ws = seed("e1val");
    let (s, v) = ask(&ws, "apps.core", json!({ "core": "nope" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
    let (s, v) = ask(&ws, "apps.core", json!({}));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
    let (s, v) = ask(&ws, "apps.manage-full", json!({}));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
    assert!(v["error"]["hint"].as_str().is_some_and(|h| h.contains("apps.build")), "{v}");
    let (s, v) = ask(&ws, "mail.send-mode", json!({ "mode": "freely" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
    let (s, v) = ask(&ws, "compute.use", json!({ "workspace": "someone-else" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
}

/// "with approval" is met by "on": asking for approval never lowers it.
#[test]
fn send_mode_approval_is_already_done_when_sending_is_on() {
    let _env = env();
    let ws = seed("e1mail");
    assert_eq!(check("mail.send-mode", &ws, json!({ "mode": "approval" })), Check::Needed);
    ask_and_allow(&ws, "mail.send-mode", json!({ "mode": "on" }), "owner");
    assert_eq!(check("mail.send-mode", &ws, json!({ "mode": "approval" })), Check::AlreadyDone);
    assert_eq!(check("mail.send-mode", &ws, json!({ "mode": "on" })), Check::AlreadyDone);
}

/// Q1: a switch turned on in Settings before the click → Allow applies
/// nothing and the request ends `already_done`.
#[test]
fn a_switch_turned_on_in_settings_first_is_already_on_at_the_click() {
    let _env = env();
    let ws = seed("e1q1");
    let card = ask_card(&ws, "chat.manage", json!({}));
    k2_core::workspace::settings::set_agents_can_manage_chat(&ws.id, true).expect("Settings");
    let (s, v) = answer("owner", &ws, &card, "Allow");
    assert_eq!((s.as_str(), v["decided"][0]["state"].as_str()), ("200 OK", Some("already_done")), "{v}");
}

// ── connections ─────────────────────────────────────────────────────

#[test]
fn connections_add_connects_once_and_checks_its_target() {
    let _env = env();
    let (ws, other) = (seed("e1ca"), seed("e1cb"));
    let (s, v) = ask(&ws, "connections.add", json!({ "to": ws.handle }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "itself: {v}");
    let (s, v) = ask(&ws, "connections.add", json!({ "to": "no-such-workspace-e1" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
    assert!(!k2_core::connections::are_local_peers(&ws.id, &other.id));
    let a = ask_and_allow(&ws, "connections.add", json!({ "to": other.handle }), "owner");
    assert!(k2_core::connections::are_local_peers(&ws.id, &other.id), "{a}");
    let (s, v) = ask(&ws, "connections.add", json!({ "to": other.handle }));
    assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("already_done")), "{v}");
    // The standing switch.
    assert!(!k2_core::workspace::settings::get_agents_can_create_connections(&ws.path));
    ask_and_allow(&ws, "connections.create", json!({}), "owner");
    assert!(k2_core::workspace::settings::get_agents_can_create_connections(&ws.path));
    ask_and_allow(&ws, "connections.create-off", json!({}), "owner");
    assert!(!k2_core::workspace::settings::get_agents_can_create_connections(&ws.path));
}

// ── GardenOwner (§5.2.1) ────────────────────────────────────────────

struct ZenEnv {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    f: k2_core::zen::ZenFiles,
}

fn zen_env() -> ZenEnv {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let f = k2_core::zen::ZenFiles::local();
    f.setup().expect("zen setup");
    ZenEnv { _home: home, _db: db, f }
}

fn zen(path: &str, owner: bool, caller: &Caller, body: Value) -> (String, Value) {
    let r = crate::zen_routes::handle(path, owner, caller, "test", &HashMap::new(), body.to_string().as_bytes());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn widget_folder(id: &str, body: &str) -> Value {
    json!([
        {"name": "manifest.json", "text": format!("{{\"schema\": 1, \"id\": \"{id}\", \"name\": \"Board\", \"caps\": [\"agents:read\"]}}")},
        {"name": "index.html", "text": format!("<!doctype html><p>{body}</p><script src=\"app.js\"></script>")},
        {"name": "app.js", "text": "console.log(1)\n"},
    ])
}

fn manager(id: &str) -> Caller {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle, agents_can_manage_gardens) VALUES (?1, ?1, ?2, ?1, 1)",
        rusqlite::params![id, format!("/tmp/zen-e1/{id}")],
    )
    .expect("insert workspace");
    Caller::Agent(crate::sidecar_routes::AgentCaller { project_id: id.into(), map_key: None, args: vec![], sandboxed: false })
}

#[test]
fn garden_owner_is_the_owner_token_or_an_owner_login() {
    let _e = zen_env();
    let owner = login("e1go", Role::Owner);
    let admin = login("e1ga", Role::Admin);
    let member = login("e1gm", Role::Member);
    let may = |c: &Caller| crate::zen_routes::garden_owner_may(c, Some("garden-1"));
    assert!(may(&Caller::Owner));
    assert!(may(&Caller::Login { username: owner }));
    for c in [
        Caller::Login { username: admin },
        Caller::Login { username: member },
        Caller::Login { username: "no-such-login".into() },
        Caller::Shell { project_id: "w".into() },
        Caller::Agent(crate::sidecar_routes::AgentCaller { project_id: "w".into(), map_key: None, args: vec![], sandboxed: false }),
        Caller::WrongCredential,
        Caller::Invalid,
    ] {
        assert!(!may(&c), "{c:?}");
    }
}

/// Settings widens with the card (exception 3): an Owner login claims,
/// resumes and approves; an Admin login doesn't; the lock and every other
/// Zen route stay owner-token only.
#[test]
fn settings_takes_an_owner_login_on_the_three_garden_owner_routes_only() {
    let e = zen_env();
    let owner = Caller::Login { username: login("e1zo", Role::Owner) };
    let admin = Caller::Login { username: login("e1za", Role::Admin) };
    let (s, v) = zen("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": widget_folder("board", "v1")}));
    assert_eq!(s, "200 OK", "{v}");
    // Keep as mine (claim).
    let (s, v) = zen("/cli/zen/widget/claim", false, &admin, json!({"widget": "board"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "{v}");
    let (s, v) = zen("/cli/zen/widget/claim", false, &owner, json!({"widget": "board"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["madeBy"], json!({"kind": "owner"}), "{v}");
    // Widget resume: past the gate for the Owner login (nothing paused).
    let g = e.f.find_garden("Garden 1").expect("Garden 1").1.id;
    let (s, v) = zen("/cli/zen/widget/resume", false, &admin, json!({"garden": g, "placement": "p1"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "{v}");
    let (s, v) = zen("/cli/zen/widget/resume", false, &owner, json!({"garden": g, "placement": "p1"}));
    assert_eq!((s.as_str(), v["resumed"].as_bool()), ("200 OK", Some(false)), "{v}");
    // Proposal approve.
    let plain = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('w-e1-plain', 'w-e1-plain', '/tmp/zen-e1/plain', 'w-e1-plain')", [])
            .expect("insert");
        Caller::Agent(crate::sidecar_routes::AgentCaller { project_id: "w-e1-plain".into(), map_key: None, args: vec![], sandboxed: false })
    };
    let (_, v) = zen("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": widget_folder("board", "v2")}));
    let id = v["proposal"]["id"].as_str().unwrap_or_else(|| panic!("a proposal: {v}")).to_string();
    let (s, v) = zen("/cli/zen/proposals/approve", false, &admin, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    let (s, v) = zen("/cli/zen/proposals/approve", false, &owner, json!({"id": id}));
    assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("approved")), "{v}");
    // Not widened: the lock, and every other route.
    let (s, v) = zen("/cli/zen/garden/lock", false, &owner, json!({"garden": g, "locked": true}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "{v}");
    let (s, v) = zen("/cli/zen/proposals/reject", false, &owner, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    let (s, v) = zen("/cli/zen/widget/sign", false, &owner, json!({"widget": "board"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
}

/// The card's Keep as mine: the Garden's owner from the phone (an Owner
/// login) keeps it through the claim entry; an Admin is refused at the card
/// and at the entry; a version that moved is stale; a kept one is already
/// done.
#[test]
fn keep_mine_through_the_card_takes_the_gardens_owner() {
    let e = zen_env();
    let owner_user = login("e1ko", Role::Owner);
    let admin_user = login("e1ka", Role::Admin);
    let mgr = manager("w-e1-keep");
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), "w-e1-keep", None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    let ws = Ws { id: "w-e1-keep".into(), handle: "w-e1-keep".into(), path: "/tmp/zen-e1/w-e1-keep".into() };
    let (s, v) = zen("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "board", "files": widget_folder("board", "agent")}));
    assert_eq!(s, "200 OK", "{v}");
    // The card names the version by its full 64-hex hash (a content hash,
    // never taken for a secret); its first 12+ characters work too (below).
    let live = e.f.widget_status("board").bundle.expect("a good version").hash;
    assert_eq!(live.len(), 64, "{live}");
    let (s, v) = ask(&ws, "garden.keep-mine", json!({ "widget": "board", "version": &live[..8] }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "too short a version: {v}");
    assert_ne!(crate::zen_routes::widget_live_version("board").expect("live").1, k2_core::garden_data::MadeBy::Owner);
    // A version that isn't live: refused at the ask.
    let (s, v) = ask(&ws, "garden.keep-mine", json!({ "widget": "board", "version": "0".repeat(64) }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
    let card = ask_card(&ws, "garden.keep-mine", json!({ "widget": "board", "version": live }));
    let (s, v) = answer(&admin_user, &ws, &card, "Allow");
    assert_eq!((s.as_str(), v["error"]["required"].as_str()), ("403 Forbidden", Some("owner")), "{v}");
    match apply("garden.keep-mine", &ws, json!({ "widget": "board", "version": live }), &as_login(&admin_user)) {
        Applied::Refused { code, .. } => assert_eq!(code, "owner_only"),
        other => panic!("the entry refuses an Admin too: {other:?}"),
    }
    let (s, v) = answer(&owner_user, &ws, &card, "Allow");
    assert_eq!((s.as_str(), v["action"]["result"]["outcome"].as_str()), ("200 OK", Some("approved")), "{v}");
    assert_eq!(crate::zen_routes::widget_live_version("board").expect("live").1, k2_core::garden_data::MadeBy::Owner);
    assert_eq!(check("garden.keep-mine", &ws, json!({ "widget": "board", "version": live })), Check::AlreadyDone);
    // The widget changes after the ask: stale at the click.
    let (s, v) = zen("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "board", "files": widget_folder("board", "agent2")}));
    assert_eq!(s, "200 OK", "{v}");
    let live2 = e.f.widget_status("board").bundle.expect("a good version").hash[..12].to_string();
    let card = ask_card(&ws, "garden.keep-mine", json!({ "widget": "board", "version": live2 }));
    let (s, v) = zen("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "board", "files": widget_folder("board", "agent3")}));
    assert_eq!(s, "200 OK", "{v}");
    let (s, v) = answer("owner", &ws, &card, "Allow");
    assert_eq!((s.as_str(), v["decided"][0]["state"].as_str()), ("200 OK", Some("stale")), "{v}");
}
