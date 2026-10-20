//! Per-agent App permissions, in process (`prd-agent-app-permissions-v1`
//! §11; slices A, C, E): the switch × caller × verb matrix, switch 2's own
//! widenings, the person-present list from every caller, the switch
//! writer's floors (the owner raises, an Admin lowers), Members read-only,
//! purge with the person, and the one-time review note.

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::app_grace::requests as reqs;
use k2_core::connect_users::Role;
use k2_core::deploy::store;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::access::{self, Level, SwitchChange};
use super::egress_routes::test_seams;
use super::routes::Who;
use crate::sidecar_routes::Caller;

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

fn ws_path(ws: &str) -> String {
    format!("/tmp/k2-access-test/{ws}")
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, ws_path(ws)]).unwrap();
        }
        for (app, ws, iso) in [("jobs", &ws_a, "container"), ("other", &ws_b, "container"), ("plain", &ws_a, "process"), ("hum", &ws_b, "process")] {
            // Each App has its service row (a held config change files
            // against it).
            let svc = k2_core::published_services::insert(
                &conn,
                ws,
                app,
                k2_core::published_services::CMD_APP_SENTINEL,
                &format!("/tmp/k2-access-test/apps/{app}"),
                0,
                k2_core::published_services::EXPOSE_LOCAL,
                k2_core::published_services::DESIRED_STOPPED,
                k2_core::published_services::KIND_CMD,
                "",
            )
            .unwrap();
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &svc.id,
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: iso,
                    memory_limit_bytes: Some(256 << 20),
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
    }
    MSGS.lock().unwrap().clear();
    super::alerts::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn shell(ws: &str) -> Who {
    Who::Shell { project_id: ws.into(), session_id: format!("shell-{ws}") }
}

fn login(name: &str, role: Role) -> Who {
    Who::Login { username: name.into(), role }
}

/// Set a workspace's switches straight in the table (the writer has its
/// own test).
fn set_level(ws: &str, l: Level) {
    let (b, f) = match l {
        Level::Off => (0, 0),
        Level::Build => (1, 0),
        Level::Full => (1, 1),
    };
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_apps_full = ?3 WHERE id = ?1", params![ws, b, f]).unwrap();
}

/// (HTTP status, JSON body).
fn call(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::routes::handle(path, who.clone(), b"", &p);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn grace(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::grace::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn access_set(caller: Caller, body: J) -> (u16, J) {
    let r = crate::sidecar_routes::handle("/cli/agent-access/set", caller, body.to_string().as_bytes(), &HashMap::new(), "test");
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("access set: not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum O {
    Run,
    Held,
    Refused,
}
use O::{Held, Refused, Run};

fn outcome(st: u16, v: &J) -> O {
    if st == 202 && v["status"] == "held" {
        return Held;
    }
    match v["error"]["code"].as_str() {
        Some(
            "owner_only" | "apps_manage_off" | "forbidden" | "wrong_credential" | "member_read_only" | "vault_confirm_required",
        ) => Refused,
        _ => Run,
    }
}

fn run_row(path: &str, post: bool, who: &Who, body: &J) -> (u16, J) {
    if post {
        call(path, who, body.clone())
    } else {
        let q: Vec<(&str, &str)> = body.as_object().unwrap().iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect();
        get(path, who, &q)
    }
}

/// §11 switch matrix: for each switch state (off, switch 1, switches
/// 1 + 2), each `/cli/app/*` verb from an agent runs, is held or is
/// refused. Person-present items are held in every state.
#[test]
fn agent_switch_matrix_off_build_full() {
    let f = fx("matrix");
    // Box admission never decides a row here (it has its own test).
    let _pool = super::admission::pool_for_test(64 << 30);
    // jobs sits behind the vm wall, so lowering it to container is the
    // isolation row: since 0.46.3 a move onto the unwalled process engine
    // is refused outright while the owner has that engine off (the
    // default), before any hold.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE apps SET isolation = 'vm' WHERE app_id = 'jobs'", []).unwrap();
    }
    let ghost = json!({"app": "ghost-app"});
    // (route, POST?, body, off, switch 1, switches 1+2)
    let rows: Vec<(&str, bool, J, O, O, O)> = vec![
        // Reads: own Apps, every state.
        ("/cli/app/list", false, json!({}), Run, Run, Run),
        ("/cli/app/inspect", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/history", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/logs", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/errors", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/audit", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/quota", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/link", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/egress", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/box-service", false, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/remote-pass", false, json!({"app": "jobs"}), Run, Run, Run),
        // Create and run: held without switch 1.
        ("/cli/app/build", true, json!({}), Held, Run, Run),
        ("/cli/app/install", true, json!({}), Held, Run, Run),
        ("/cli/app/deploy", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/rollback", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/stop", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/start", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/restart", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/config", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/uninstall", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/link/rotate", true, ghost.clone(), Held, Run, Run),
        ("/cli/app/exec", true, ghost.clone(), Held, Run, Run),
        // Lowering a quota is narrowing.
        ("/cli/app/quota/set", true, json!({"app": "jobs", "rps": 10}), Held, Run, Run),
        // Budgets inside the default limits (limits.rs): switch 1 applies
        // them (Rosson 2026-10-20: switch 1 = build and run within the
        // default limits; switch 2 = manage caps and budgets).
        ("/cli/app/config", true, json!({"app": "plain", "memory": "1G"}), Held, Run, Run),
        // Raises: switch 2 makes them itself; switch 1 proposes (budgets
        // past its default limits included).
        ("/cli/app/quota/set", true, json!({"app": "jobs", "rps": 900}), Held, Held, Run),
        ("/cli/app/config", true, json!({"app": "jobs", "memory": "3G"}), Held, Held, Run),
        ("/cli/app/config", true, json!({"app": "jobs", "cpuWeight": 400}), Held, Held, Run),
        ("/cli/app/config", true, json!({"app": "plain", "dbPrivate": "on"}), Held, Held, Run),
        ("/cli/app/config", true, json!({"app": "plain", "io": {"wbps": "500M"}}), Held, Held, Run),
        // Loosening the wall: the person, in every state (§2.2).
        ("/cli/app/config", true, json!({"app": "jobs", "path": "open"}), Held, Held, Held),
        ("/cli/app/config", true, json!({"app": "jobs", "portAdd": "5000/udp"}), Held, Held, Held),
        ("/cli/app/config", true, json!({"app": "jobs", "isolation": "container"}), Held, Held, Held),
        ("/cli/app/config", true, json!({"app": "jobs", "dbRaw": "on"}), Held, Held, Held),
        // Purge: never an agent (held as a manage request without switch 1;
        // the replay is refused).
        ("/cli/app/uninstall", true, json!({"app": "plain", "purge": true}), Held, Refused, Refused),
        // Narrowing never waits.
        ("/cli/app/suspend", true, ghost.clone(), Run, Run, Run),
        ("/cli/app/remote-pass/revoke", true, json!({"app": "jobs"}), Run, Run, Run),
        ("/cli/app/egress/rm", true, json!({"app": "jobs", "host": "nowhere.example:443"}), Run, Run, Run),
        // The owner's, in person: never an agent.
        ("/cli/app/resume", true, json!({"app": "jobs"}), Refused, Refused, Refused),
        ("/cli/app/remote-pass/rotate", true, json!({"app": "jobs"}), Refused, Refused, Refused),
    ];
    let a = agent(&f.ws_a);
    for (state, l) in [Level::Off, Level::Build, Level::Full].into_iter().enumerate() {
        set_level(&f.ws_a, l);
        assert_eq!(access::level(&f.ws_a), l);
        for (path, post, body, o0, o1, o2) in &rows {
            let (st, v) = run_row(path, *post, &a, body);
            let want = [*o0, *o1, *o2][state];
            assert_eq!(outcome(st, &v), want, "{l:?}: {path} {body} → {st} {v}");
        }
    }
    // The raises applied in the last state (switch 2), and nothing that
    // loosens the wall did.
    let h = k2_core::db::shared();
    let conn = h.lock();
    let plain = store::get_app(&conn, "plain").unwrap().unwrap();
    assert_eq!(plain.memory_limit_bytes, Some(1 << 30), "switch 1 raised its own memory inside the default limits");
    assert_eq!(store::get_app(&conn, "jobs").unwrap().unwrap().memory_limit_bytes, Some(3 << 30), "switch 2 raised past them");
    let cfg = k2_core::app_grace::config::load(&conn, "plain").unwrap();
    assert!(cfg.db.private, "switch 2 turned its own private database on");
    let jobs = store::get_app(&conn, "jobs").unwrap().unwrap();
    assert_eq!((jobs.path.as_str(), jobs.isolation.as_str()), ("guarded", "vm"), "the wall never moved");
    let jcfg = k2_core::app_grace::config::load(&conn, "jobs").unwrap();
    assert!(!jcfg.db.raw && jcfg.ports.is_empty(), "{jcfg:?}");
    assert_eq!(super::quotas::load_override(&conn, "jobs").unwrap().rps, Some(900));
    let _ = &f.ws_b;
}

/// §11 matrix for humans: the owner (token, login, shell tab) and an
/// Admin act; a Member reads only (Q7); loosening and purge wait for the
/// person from everyone, the owner token included.
#[test]
fn human_caller_matrix() {
    let f = fx("humans");
    let callers: Vec<(&str, Who)> = vec![
        ("member", login("mo", Role::Member)),
        ("admin", login("ada", Role::Admin)),
        ("shell", shell(&f.ws_b)),
        ("owner-login", login("rosson", Role::Owner)),
        ("owner", Who::Owner),
    ];
    // (route, POST?, body, member, admin, shell, owner login, owner token)
    let rows: Vec<(&str, bool, J, [O; 5])> = vec![
        ("/cli/app/list", false, json!({}), [Run, Run, Run, Run, Run]),
        ("/cli/app/inspect", false, json!({"app": "hum"}), [Run, Run, Run, Run, Run]),
        ("/cli/app/install", true, json!({}), [Refused, Run, Run, Run, Run]),
        ("/cli/app/stop", true, json!({"app": "ghost-app"}), [Refused, Run, Run, Run, Run]),
        ("/cli/app/suspend", true, json!({"app": "ghost-app"}), [Refused, Run, Run, Run, Run]),
        ("/cli/app/exec", true, json!({"app": "ghost-app"}), [Refused, Run, Run, Run, Run]),
        ("/cli/app/egress/add", true, json!({"app": "hum", "host": "api.example.com"}), [Refused, Held, Held, Held, Held]),
        ("/cli/app/config", true, json!({"app": "hum", "memory": "2G"}), [Refused, Run, Run, Run, Run]),
        // Owner-only (non-loosening): the owner; an Admin proposes.
        ("/cli/app/config", true, json!({"app": "hum", "dbPrivate": "on"}), [Refused, Held, Run, Run, Run]),
        ("/cli/app/quota/set", true, json!({"app": "hum", "rps": 700}), [Refused, Held, Run, Run, Run]),
        // Loosening: held for the person from every caller.
        ("/cli/app/config", true, json!({"app": "other", "path": "open"}), [Refused, Held, Held, Held, Held]),
        ("/cli/app/config", true, json!({"app": "hum", "dbRaw": "on"}), [Refused, Held, Held, Held, Held]),
        ("/cli/app/config", true, json!({"app": "hum", "portAdd": "7000"}), [Refused, Held, Held, Held, Held]),
        // Purge: the owner in person only (no confirm here → refused).
        ("/cli/app/uninstall", true, json!({"app": "hum", "purge": true}), [Refused, Refused, Refused, Refused, Refused]),
    ];
    for (path, post, body, want) in &rows {
        for (i, (name, who)) in callers.iter().enumerate() {
            let (st, v) = run_row(path, *post, who, body);
            assert_eq!(outcome(st, &v), want[i], "{name}: {path} {body} → {st} {v}");
            if *name == "member" && *post {
                assert_eq!(code(&v), access::MEMBER_READ_ONLY, "{path}: {v}");
            }
        }
    }
    // Members never reach the grace verbs either.
    let (st, v) = grace("/cli/app/clean-mode", &login("mo", Role::Member), json!({"app": "hum", "on": true}));
    assert_eq!((st, code(&v)), (403, access::MEMBER_READ_ONLY), "{v}");
    let (st, v) = grace("/cli/app/deny", &login("mo", Role::Member), json!({"request": "w_x"}));
    assert_eq!((st, code(&v)), (403, access::MEMBER_READ_ONLY), "{v}");
    assert!(super::exec::may_exec(&login("mo", Role::Member)).is_err(), "no shell for a Member");
    // The App is still there: no purge happened.
    let h = k2_core::db::shared();
    let conn = h.lock();
    assert!(store::get_app(&conn, "hum").unwrap().is_some());
}

/// Switch 2 widens its own Apps with no Approvals queue: egress hosts,
/// box services, keys, clean mode off, quotas. Switch 1 proposes the same.
#[test]
fn switch_2_applies_its_own_widenings_and_switch_1_proposes_them() {
    let f = fx("full");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::published_services::insert(
            &conn,
            &f.ws_a,
            "bff",
            "npm start",
            "/tmp",
            3000,
            k2_core::published_services::EXPOSE_LOCAL,
            k2_core::published_services::DESIRED_STOPPED,
            k2_core::published_services::KIND_CMD,
            "",
        )
        .unwrap();
    }
    let a = agent(&f.ws_a);
    crate::vault::publish_env::set_clean_mode(&{
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::published_services::get_by_project_name(&conn, &f.ws_a, "bff").unwrap().unwrap().id
    }, true)
    .unwrap();

    // Switch 1: proposals.
    set_level(&f.ws_a, Level::Build);
    let (st, v) = call("/cli/app/egress/add", &a, json!({"app": "jobs", "host": "api.stripe.com"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let held_egress = v["request"]["id"].as_str().unwrap().to_string();
    // Someone else's proposal (the owner token: egress hosts wait for the
    // person) never rides along with the agent's own.
    let (st, v) = call("/cli/app/egress/add", &Who::Owner, json!({"app": "jobs", "host": "owner.example.com"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let owner_egress = v["request"]["id"].as_str().unwrap().to_string();
    let (st, v) = call("/cli/app/box-service/request", &a, json!({"app": "jobs", "name": "files-api", "hostPort": 9100}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let (st, v) = grace("/cli/app/clean-mode", &a, json!({"app": "bff", "on": false}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let (st, v) = grace("/cli/app/key/mint", &a, json!({"app": "bff", "scopes": ["users"]}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");

    // Switch 2: applied at once, audited, never queued.
    set_level(&f.ws_a, Level::Full);
    let (st, v) = call("/cli/app/egress/add", &a, json!({"app": "jobs", "host": "api.github.com"}));
    assert_eq!((st, v["status"].as_str()), (200, Some("allowed")), "{v}");
    assert_eq!(v["request"]["status"], "approved", "{v}");
    assert_eq!(v["request"]["confirmVia"], access::SELF_VIA, "{v}");
    let (st, v) = call("/cli/app/box-service/request", &a, json!({"app": "jobs", "name": "media-api", "hostPort": 9200}));
    assert_eq!((st, v["status"].as_str()), (200, Some("allowed")), "{v}");
    let (st, v) = grace("/cli/app/clean-mode", &a, json!({"app": "bff", "on": false}));
    assert_eq!((st, v["cleanMode"].as_bool()), (200, Some(false)), "{v}");
    let (st, v) = grace("/cli/app/key/mint", &a, json!({"app": "bff", "scopes": ["users"]}));
    assert_ne!(st, 202, "switch 2 mints its own App's key, never held: {v}");
    if st != 200 {
        // The test process may have no open vault: the mint was attempted
        // (never held, never refused for the switch).
        assert!(matches!(code(&v), "vault_closed" | "vault_error"), "{v}");
    }
    let (st, v) = grace("/cli/app/key/mint", &a, json!({"app": "bff", "scopes": ["users"], "deliver": "show"}));
    assert_eq!((st, code(&v)), (403, "owner_only"), "an agent never sees a key: {v}");
    // Broad egress is the open path: still the person's.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE apps SET path = 'open' WHERE app_id = 'plain'", []).unwrap();
    }
    let (st, v) = call("/cli/app/egress/mode", &a, json!({"app": "plain", "mode": "broad"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");

    let h = k2_core::db::shared();
    let conn = h.lock();
    let allowed: Vec<String> =
        k2_core::deploy::egress::policy(&conn, "jobs").unwrap().allowed.iter().map(k2_core::deploy::egress::HostSpec::display).collect();
    // The agent's own pending proposal merged into its new ask and went
    // live with it; the owner's waits for the person.
    assert_eq!(allowed, vec!["api.github.com:443", "api.stripe.com:443"], "the agent's own hosts are live");
    assert_eq!(reqs::get(&conn, &held_egress).unwrap().unwrap().status, reqs::STATUS_APPROVED);
    assert_eq!(reqs::get(&conn, &owner_egress).unwrap().unwrap().status, reqs::STATUS_PENDING, "the owner's ask still waits");
    let services = k2_core::deploy::box_service::list(&conn, "jobs").unwrap();
    let status: Vec<(String, String)> = services.iter().map(|s| (s.name.clone(), s.status.clone())).collect();
    assert!(status.contains(&("media-api".into(), "allowed".into())), "{status:?}");
    assert!(status.contains(&("files-api".into(), "allowed".into())), "its own earlier proposal merged: {status:?}");
    let audit = k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, "jobs", 50).unwrap();
    assert!(audit.iter().any(|r| r.event == "widening_self_applied"), "{audit:?}");
}

/// The person-present list never yields to a switch: a request of a
/// loosening kind can't be self-applied, even by a switch-2 agent.
#[test]
fn self_apply_never_takes_a_loosening_kind() {
    let f = fx("guard");
    set_level(&f.ws_a, Level::Full);
    let a = agent(&f.ws_a);
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::get_app(&conn, "jobs").unwrap().unwrap()
    };
    for kind in [reqs::KIND_DB_RAW, reqs::KIND_OVERCOMMIT, reqs::KIND_EGRESS_MODE, reqs::KIND_CONFIG, reqs::KIND_INSTALL] {
        let r = super::approvals::file(&super::approvals::Filing {
            who: &a,
            kind,
            app: Some(&app),
            label: "jobs",
            workspace: &f.ws_a,
            body: json!({}),
            diff: json!({ "lines": [kind] }),
            summary: format!("{kind} for jobs"),
            reasons: vec!["test".into()],
            reason: None,
        })
        .unwrap();
        let e = access::self_apply(&r, &a).expect_err(kind);
        assert_eq!(e.code, "forbidden", "{kind}");
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert_eq!(reqs::get(&conn, &r.id).unwrap().unwrap().status, reqs::STATUS_PENDING, "{kind} still waits for the person");
    }
    // A switch-1 agent can't self-apply anything.
    set_level(&f.ws_a, Level::Build);
    let r = super::approvals::file(&super::approvals::Filing {
        who: &a,
        kind: reqs::KIND_EGRESS,
        app: Some(&app),
        label: "jobs",
        workspace: &f.ws_a,
        body: json!({}),
        diff: json!({}),
        summary: "egress".into(),
        reasons: vec![],
        reason: None,
    })
    .unwrap();
    assert_eq!(access::self_apply(&r, &a).unwrap_err().code, "forbidden");
    assert!(access::LOOSENING_CAPS.contains(&"files:agentconfig") && access::LOOSENING_CAPS.contains(&"heartbeats:write"));
}

/// Q2 / F5: only the owner raises, an Admin only lowers, an agent never;
/// switch 2 sits under switch 1; every change audits and leaves the agent
/// an inbox note (no wake); the old open switch is retired (N1).
#[test]
fn the_owner_raises_an_admin_lowers_and_agents_never_touch_their_switches() {
    let f = fx("writer");
    k2_core::connect_users::add_user("ada", "Correct-Horse-9-Battery").unwrap();
    k2_core::connect_users::set_role("ada", Role::Admin).unwrap();
    k2_core::connect_users::add_user("rosson", "Correct-Horse-9-Battery").unwrap();
    k2_core::connect_users::set_role("rosson", Role::Owner).unwrap();
    let admin = || Caller::Login { username: "ada".into() };
    let ws = f.ws_a.clone();
    let body = |toggle: &str, value: bool| json!({"workspace": ws, "toggle": toggle, "value": value});

    // An agent, even for itself: refused.
    let ag = crate::sidecar_routes::AgentCaller { project_id: ws.clone(), map_key: None, args: vec![], sandboxed: false };
    let (st, _) = access_set(Caller::Agent(ag), body("apps_build", true));
    assert_eq!(st, 403);
    let (st, _) = access_set(Caller::Shell { project_id: ws.clone() }, body("apps_build", true));
    assert_eq!(st, 403, "a shell tab doesn't change agent access");
    // An Admin can't raise either switch.
    for t in ["apps_build", "apps_manage", "apps_manage_full"] {
        let (st, v) = access_set(admin(), body(t, true));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{t}: {v}");
    }
    assert_eq!(access::level(&ws), Level::Off);
    assert!(MSGS.lock().unwrap().is_empty(), "nothing changed, no note");
    // Switch 2 needs switch 1.
    let (st, v) = access_set(Caller::Owner, body("apps_manage_full", true));
    assert_eq!((st, code(&v)), (409, "parent_switch_off"), "{v}");
    // The owner raises (the old toggle name still works, flagged).
    let (st, v) = access_set(Caller::Owner, body("apps_manage", true));
    assert_eq!((st, v["level"].as_str()), (200, Some("build")), "{v}");
    assert!(v["deprecated"].is_string(), "{v}");
    let (st, v) = access_set(Caller::Login { username: "rosson".into() }, body("apps_manage_full", true));
    assert_eq!((st, v["level"].as_str(), v["agentsCanManageAppsFull"].as_bool()), (200, Some("full"), Some(true)), "{v}");
    {
        let msgs = MSGS.lock().unwrap();
        let mine: Vec<&String> = msgs.iter().filter(|(w, _)| w == &ws).map(|(_, t)| t).collect();
        assert_eq!(mine.len(), 2, "one note per change: {msgs:?}");
        assert!(mine[1].starts_with("[note] Your App access changed"), "{}", mine[1]);
        assert!(mine[1].contains("Build and run Apps on, manage them end to end on"), "{}", mine[1]);
        assert!(mine[1].contains("owner:rosson"), "{}", mine[1]);
    }
    // An Admin lowers: switch 1 off takes switch 2 with it.
    let (st, v) = access_set(admin(), body("apps_build", false));
    assert_eq!((st, v["level"].as_str()), (200, Some("off")), "{v}");
    assert_eq!(k2_core::workspace::settings::agents_app_switches(&ws), (false, false));
    // Back on: switch 2 stays off.
    let (_, v) = access_set(Caller::Owner, body("apps_build", true));
    assert_eq!(v["level"], "build", "{v}");
    // An Admin may lower switch 2 alone.
    let (_, _) = access_set(Caller::Owner, body("apps_manage_full", true));
    let (st, v) = access_set(admin(), body("apps_manage_full", false));
    assert_eq!((st, v["level"].as_str()), (200, Some("build")), "{v}");
    // No change, no note.
    let before = MSGS.lock().unwrap().len();
    let (st, v) = access_set(admin(), body("apps_manage_full", false));
    assert_eq!((st, v["changed"].as_bool()), (200, Some(false)), "{v}");
    assert_eq!(MSGS.lock().unwrap().len(), before);
    // N1: the open switch is retired; turning it off still works.
    let (st, v) = access_set(Caller::Owner, body("apps_open", true));
    assert_eq!((st, code(&v)), (410, access::OPEN_RETIRED), "{v}");
    let (st, _) = access_set(admin(), body("apps_open", false));
    assert_eq!(st, 200);
    // Every change wrote a deploy-audit row on the workspace's Apps.
    let h = k2_core::db::shared();
    let conn = h.lock();
    let rows = k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, "jobs", 50).unwrap();
    let changes: Vec<&str> = rows.iter().filter(|r| r.event == "access_changed").map(|r| r.actor.as_str()).collect();
    assert!(changes.len() >= 5, "{changes:?}");
    assert!(changes.contains(&"owner") && changes.iter().any(|a| a.contains("ada")), "{changes:?}");
    // The other workspace was never touched.
    assert_eq!(access::level(&f.ws_b), Level::Off);
}

/// The writer used by `/cli/agents-manage-skin` and the hire path: the
/// same floors without a route around them.
#[test]
fn set_switches_refuses_a_non_owner_raise() {
    let f = fx("setsw");
    let e = access::set_switches(&f.ws_a, SwitchChange { build: Some(true), full: None }, false, "user:ada").unwrap_err();
    assert_eq!(e.code, "owner_only");
    let v = access::set_switches(&f.ws_a, SwitchChange { build: Some(true), full: Some(true) }, true, "owner").unwrap();
    assert_eq!(v["level"], "full", "{v}");
    let v = access::set_switches(&f.ws_a, SwitchChange { build: Some(false), full: None }, false, "user:ada").unwrap();
    assert_eq!(v["level"], "off", "{v}");
    assert!(access::set_switches("no-such-ws", SwitchChange::default(), true, "owner").is_err());
}

/// Slice E (F3): purge needs the owner in person, every time: the owner
/// token alone is refused, a confirm bound to another App is refused, a
/// fresh confirm bound to this App purges. Uninstall without purge is
/// switch 1's (N2).
#[test]
fn purge_needs_the_person_every_time() {
    let f = fx("purge");
    set_level(&f.ws_a, Level::Full);
    let (st, v) = call("/cli/app/uninstall", &agent(&f.ws_a), json!({"app": "plain", "purge": true}));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/uninstall", &shell(&f.ws_a), json!({"app": "plain", "purge": true}));
    assert_eq!((st, code(&v)), (403, "owner_only"), "a shell tab can't pass a confirm: {v}");
    let (st, v) = call("/cli/app/uninstall", &login("ada", Role::Admin), json!({"app": "plain", "purge": true}));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/uninstall", &Who::Owner, json!({"app": "plain", "purge": true}));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "the owner token alone never purges: {v}");
    assert_eq!(v["error"]["action"], access::PURGE_CONFIRM, "{v}");
    let vault = test_seams::vault_handle();
    let other = vault.confirm.mint(access::PURGE_CONFIRM, Some("jobs"), "owner", "local-auth");
    let (st, v) = call("/cli/app/uninstall", &Who::Owner, json!({"app": "plain", "purge": true, "confirmToken": other}));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "bound to another App: {v}");
    let tok = vault.confirm.mint(access::PURGE_CONFIRM, Some("plain"), "owner", "local-auth");
    let (st, v) = call("/cli/app/uninstall", &Who::Owner, json!({"app": "plain", "purge": true, "confirmToken": tok}));
    assert_eq!((st, v["purged"].as_bool()), (200, Some(true)), "{v}");
    let (st, v) = call("/cli/app/uninstall", &Who::Owner, json!({"app": "plain", "purge": true, "confirmToken": tok}));
    assert_eq!(st, 404, "gone (and a confirm works once): {v}");
    // An Owner login with its own stamp.
    let tok = vault.confirm.mint(access::PURGE_CONFIRM, Some("hum"), "connect:rosson", "login-password");
    let (st, v) = call("/cli/app/uninstall", &login("rosson", Role::Owner), json!({"app": "hum", "purge": true, "confirmToken": tok}));
    assert_eq!((st, v["purged"].as_bool()), (200, Some(true)), "{v}");
    // N2: switch 1 uninstalls its own App without purge.
    set_level(&f.ws_a, Level::Build);
    let (st, v) = call("/cli/app/uninstall", &agent(&f.ws_a), json!({"app": "jobs"}));
    assert_eq!((st, v["purged"].as_bool()), (200, Some(false)), "{v}");
}

/// Q4: the agents 0197 kept on switch 1 are named to the owner once.
#[test]
fn the_review_note_goes_to_the_owner_once() {
    let f = fx("review");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&f.ws_a, &f.ws_b, &"gone-ws".to_string()] {
            conn.execute("INSERT INTO app_access_review (project_id, created_at) VALUES (?1, 1)", params![ws]).unwrap();
        }
    }
    let pending = access::pending_review();
    assert_eq!(pending.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec![f.ws_a.as_str(), f.ws_b.as_str()], "a deleted workspace isn't named");
    let id = access::review_note_once().expect("one note");
    let item = k2_core::feedback::get_item(&id).expect("the review ticket");
    assert!(item.body.as_deref().unwrap_or("").contains("now have Build and run Apps on"), "{item:?}");
    assert!(item.body.as_deref().unwrap_or("").contains(&f.ws_a), "{item:?}");
    assert!(access::pending_review().is_empty());
    assert_eq!(access::review_note_once(), None, "once");
    let list = get("/cli/app/list", &Who::Owner, &[]).1;
    assert_eq!(list["accessReview"].as_array().map(Vec::len), Some(2), "{list}");
    assert!(list["accessReview"][0]["notedAt"].is_i64(), "{list}");
    let mine = get("/cli/app/list", &agent(&f.ws_a), &[]).1;
    assert!(mine.get("accessReview").is_none() && mine["access"]["level"] == "off", "{mine}");
}

fn mem(app: &str) -> Option<i64> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app(&conn, app).unwrap().unwrap().memory_limit_bytes
}

fn approve_as_owner(req: &str) -> Result<J, super::routes::Refusal> {
    let tok = test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(req), "owner", "local-auth");
    super::approvals::handle("/cli/app/approve", &Who::Owner, json!({ "request": req, "confirmToken": tok }).to_string().as_bytes(), &HashMap::new())
}

/// Rosson 2026-10-20, the integrated shape of slices A and F: switch 1
/// builds and runs inside the default limits (a budget inside them
/// applies; past them, or one they don't size, is held for the owner);
/// switch 2 manages budgets itself, inside box admission; every memory
/// budget that grows meets admission from every caller, now and when a
/// held request is approved later; loosening the wall waits for the
/// person on both switches.
#[test]
fn switch_1_inside_the_default_limits_switch_2_inside_admission() {
    let f = fx("shape");
    let a = agent(&f.ws_a);
    // Apps hold 256 MiB each; plain is the one that grows. Pool 4 GiB.
    let pool = super::admission::pool_for_test(4 << 30);

    // Switch 1: inside the default limits, applied at once.
    set_level(&f.ws_a, Level::Build);
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "memory": "2G"}));
    assert_eq!((st, v["status"].as_str()), (200, None), "2 GiB is inside the default: {v}");
    assert_eq!(mem("plain"), Some(2 << 30));
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "cpuWeight": 80}));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "disk": "1G"}));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "disk": "4G"}));
    assert_eq!((st, v["status"].as_str()), (200, None), "a disk raise inside 10 GiB: {v}");
    // Past the limits: held for the owner, saying which limit.
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "memory": "3G"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert!(v["limits"][0].as_str().unwrap().contains("up to 2 GiB"), "{v}");
    let held_3g = v["held"]["request"]["id"].as_str().unwrap_or_else(|| panic!("no held request id: {v}")).to_string();
    assert_eq!(mem("plain"), Some(2 << 30), "unchanged until the owner approves");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "cpuWeight": 400}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "disk": "11G"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    // A budget the default limits don't size (tasks) is switch 2's.
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "tasks": 100}));
    assert_eq!((st, v["status"].as_str()), (200, None), "setting a task limit narrows: {v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "tasks": 200}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert!(v["held"]["request"].to_string().contains("Build and run Apps"), "the reason names the switch: {v}");
    // Loosening: the person, on switch 1.
    let (st, v) = call("/cli/app/config", &a, json!({"app": "jobs", "dbRaw": "on"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");

    // The owner approves the held 3 GiB: it fits the box, so it applies.
    let r = approve_as_owner(&held_3g).unwrap_or_else(|e| panic!("approve: {} {}", e.code, e.hint));
    assert_eq!(r["request"]["status"], "approved", "{r}");
    assert_eq!(mem("plain"), Some(3 << 30));

    // A held raise approved later meets today's box: shrink the pool,
    // then approve. Refused, nothing changes.
    // (Filed: it fits today's 4 GiB, 768 MiB + 3200 MiB.)
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "memory": "3200M"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let later = v["held"]["request"]["id"].as_str().unwrap().to_string();
    // (A guard clears the override when it drops: one at a time.)
    drop(pool);
    let small = super::admission::pool_for_test(3 << 30);
    // The decision stands; applying it is refused (approvals' shape).
    let r = approve_as_owner(&later).unwrap_or_else(|e| panic!("approve: {} {}", e.code, e.hint));
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("box_budget_full")), "{r}");
    assert_eq!(mem("plain"), Some(3 << 30));
    drop(small);
    let _pool = super::admission::pool_for_test(4 << 30);

    // Switch 2: its own budgets, past the default limits, inside the box.
    set_level(&f.ws_a, Level::Full);
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "tasks": 400}));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "cpuWeight": 400}));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "jobs", "memory": "512M"}));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    // 256 + 256 + 512 MiB held by the others: 3 GiB fits 4 GiB, 3.5 doesn't.
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "memory": "3G"}));
    assert_eq!(st, 200, "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "memory": "3500M"}));
    assert_eq!((st, code(&v)), (409, "box_budget_full"), "switch 2 still meets admission: {v}");
    assert_eq!(mem("plain"), Some(3 << 30));
    // Loosening stays the person's on switch 2.
    let (st, v) = call("/cli/app/config", &a, json!({"app": "jobs", "path": "open"}));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");

    // Humans aren't bound by an agent's limits, but meet the box too.
    let (st, v) = call("/cli/app/config", &Who::Owner, json!({"app": "hum", "memory": "8G"}));
    assert_eq!((st, code(&v)), (409, "box_budget_full"), "{v}");
    assert_eq!(mem("hum"), Some(256 << 20));
    // The box is full (3840 of 4096 MiB): even 300 MiB waits until
    // someone lowers; lowering always applies.
    let (st, v) = call("/cli/app/config", &Who::Owner, json!({"app": "hum", "memory": "300M"}));
    assert_eq!((st, code(&v)), (409, "box_budget_full"), "{v}");
    let (st, v) = call("/cli/app/config", &Who::Owner, json!({"app": "plain", "memory": "2G"}));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    let (st, v) = call("/cli/app/config", &Who::Owner, json!({"app": "hum", "memory": "300M"}));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    assert_eq!(mem("hum"), Some(300 << 20));
    let _ = &f.ws_b;
}
