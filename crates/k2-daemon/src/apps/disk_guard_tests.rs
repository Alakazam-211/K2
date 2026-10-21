//! Box usage and the disk guard, in process (`prd-app-resources-disk-guard-v1`
//! §17.2, lane G: tests 7–16). Every test fakes its disk under its own temp
//! `$HOME` (`k2_core::disk_guard::fake`, mode on for that root only), so no
//! test fills a real disk and no unrelated test sees the guard.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use k2_core::connect_users::Role;
use k2_core::deploy::store;
use k2_core::disk_guard::{self as dg, fake, Consumer, Mode, Owner, RawStat, Snapshot, GIB};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::egress_routes::test_seams;
use super::routes::Who;

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

const TOTAL: u64 = 1000 * GIB;
/// 50% used.
const OK_FREE: u64 = 500 * GIB;
/// 88% used: warn.
const WARN_FREE: u64 = 120 * GIB;
/// 96% used: refuse.
const REFUSE_FREE: u64 = 40 * GIB;
/// 98.5% used: critical.
const CRITICAL_FREE: u64 = 15 * GIB;

const SEED_CAPS: &str = "2222222222222222222222222222222222222222222222222222222222222222";

struct Fx {
    disk: fake::FakeGuard,
    ws_a: String,
    ws_a_path: PathBuf,
    ws_b: String,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
}

impl Drop for Fx {
    fn drop(&mut self) {
        dg::clear_snapshot();
        dg::reset_scan_control();
    }
}

impl Fx {
    fn free(&self, free: u64) {
        self.disk.set(RawStat::sized(TOTAL, free));
    }
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    dg::clear_snapshot();
    dg::reset_scan_control();
    let root = home.path().to_path_buf();
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    let ws_a_path = std::fs::canonicalize(&root).unwrap().join("ws-a");
    std::fs::create_dir_all(&ws_a_path).unwrap();
    std::fs::create_dir_all(root.join("ws-b")).unwrap();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (ws, p) in [(&ws_a, ws_a_path.clone()), (&ws_b, root.join("ws-b"))] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'Agent', ?2, 'hand-' || substr(?1, 1, 4))", params![ws, p.to_string_lossy()]).unwrap();
        }
        for (app, ws, iso) in [("jobs", &ws_a, "container"), ("plain", &ws_a, "process")] {
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &format!("svc-{app}-{tag}"),
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: iso,
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
             VALUES (?1, 'jobs', 'cmd', ?2, '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
            params![SEED_CAPS, json!({ "caps": ["thread:read"] }).to_string()],
        )
        .unwrap();
    }
    MSGS.lock().unwrap().clear();
    super::alerts::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    let disk = fake::install(&root, "9:9", "/", RawStat::sized(TOTAL, OK_FREE), Some(Mode::On));
    Fx { disk, ws_a, ws_a_path, ws_b, _db: db, _home: home }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn shell(ws: &str) -> Who {
    Who::Shell { project_id: ws.into(), session_id: format!("shell-{ws}") }
}

fn login(role: Role) -> Who {
    Who::Login { username: format!("{role:?}").to_ascii_lowercase(), role }
}

fn switch(f: &Fx, on: bool) {
    k2_core::workspace::settings::set_agents_can_manage_skin(&f.ws_a_path.to_string_lossy(), on).unwrap();
}

fn status(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split_whitespace().next().unwrap().parse().unwrap()
}

fn call(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    (status(&r), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::routes::handle(path, who.clone(), b"", &p);
    (status(&r), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn approve(req: &str) -> J {
    let tok = test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(req), "owner", "local-auth");
    super::approvals::handle("/cli/app/approve", &Who::Owner, json!({ "request": req, "confirmToken": tok }).to_string().as_bytes(), &HashMap::new())
        .unwrap_or_else(|e| panic!("approve {req}: {} {}", e.code, e.hint))
}

fn app_folder(f: &Fx, name: &str) -> PathBuf {
    let d = f.ws_a_path.join(name);
    std::fs::create_dir_all(&d).unwrap();
    let manifest = json!({
        "schema": 1, "kind": "app", "id": name, "name": name, "version": "1.0.0", "caps": [],
        "host": { "mode": "cmd", "start": ["python3", "server.py"], "health": { "path": "/healthz" }, "drainSecs": 2 },
    });
    std::fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(d.join("server.py"), "print('hi')\n").unwrap();
    d
}

fn seed_count() -> i64 {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row("SELECT COUNT(*) FROM seeds", [], |r| r.get(0)).unwrap()
}

fn audit_events(app: &str) -> Vec<(String, J)> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, app, 100)
        .unwrap()
        .into_iter()
        .map(|r| (r.event, r.payload))
        .collect()
}

fn agent_owner(handle: &str, ws: &str) -> Owner {
    Owner {
        kind: "agent".into(),
        agent: Some(format!("Agent {handle}")),
        handle: Some(handle.into()),
        address: Some(format!("{handle}::box.k2.dev")),
        workspace_id: Some(ws.into()),
    }
}

/// A scan result: the agent `nsi` owns a big folder.
fn publish_scan(f: &Fx) {
    dg::publish_snapshot(Snapshot {
        consumers: vec![
            Consumer {
                kind: "workspace".into(),
                id: f.ws_a.clone(),
                owner: agent_owner("nsi", &f.ws_a),
                disk: Some("9:9".into()),
                disk_bytes: Some(104_000_000_000),
                parts: [("uploads".to_string(), 29_000_000_000u64), ("node_modules".to_string(), 2_100_000_000)].into_iter().collect(),
                folder: Some(f.ws_a_path.to_string_lossy().to_string()),
                scanned_at: Some(1),
                ..Consumer::default()
            },
            Consumer {
                kind: "app".into(),
                id: "jobs".into(),
                path: Some("walled".into()),
                owner: agent_owner("nsi", &f.ws_a),
                disk: Some("9:9".into()),
                disk_bytes: Some(6_200_000_000),
                ..Consumer::default()
            },
        ],
        k2_will_clear: Some(dg::WillClear { seeds_bytes: 2_600_000_000, seeds: 14, next_sweep_at: None }),
        scan: dg::ScanTimes { workspaces_at: Some(1), apps_at: Some(1), databases_at: Some(1), ..Default::default() },
    });
}

const NEW_KEYS: &[&str] = &["diskWarning", "memoryWarning", "diskOverride"];

fn has_no_guard_keys(v: &J) -> bool {
    NEW_KEYS.iter().all(|k| v.get(*k).is_none())
}

// ── 7: refusals create nothing ───────────────────────────────────────

#[test]
fn at_refuse_new_work_gets_disk_low_and_nothing_is_created() {
    let f = fx("refuse");
    publish_scan(&f);
    let folder = app_folder(&f, "fresh");
    f.free(REFUSE_FREE);
    let seeds = seed_count();

    let (st, v) = call("/cli/app/build", &Who::Owner, json!({ "folder": folder }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("96% full"), "{hint}");
    assert!(hint.contains("k2 msg nsi::box.k2.dev"), "the hint names who to ask: {hint}");
    assert!(hint.contains("--disk-override"), "{hint}");
    assert!(hint.contains("2.6 GB of old Seeds"), "{hint}");
    assert_eq!(v["error"]["level"], "refuse");
    assert_eq!(v["error"]["disk"]["mount"], "/");
    assert_eq!(v["error"]["biggest"][0]["owner"]["handle"], "nsi");
    assert!(v["error"]["biggest"][0]["owner"].get("workspaceId").is_none(), "{v}");
    assert_eq!(seed_count(), seeds, "no Seed was stored");

    let (st, v) = call("/cli/app/install", &Who::Owner, json!({ "app": "fresh", "workspace": f.ws_a }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert!(store::get_app(&conn, "fresh").unwrap().is_none(), "no App row");
    }
    assert!(!k2_core::deploy::app_dir("fresh").exists(), "no App folder");

    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "seed": SEED_CAPS }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "folder": folder }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    assert_eq!(seed_count(), seeds, "a folder deploy built no Seed");

    std::fs::write(f.ws_a_path.join("rows.csv"), "a,b\n").unwrap();
    let (st, v) = call("/cli/app/data/put", &Who::Owner, json!({ "app": "plain", "from": f.ws_a_path.join("rows.csv") }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    assert!(!k2_core::deploy::app_dir("plain").join("data").join("rows.csv").exists());

    // A new --cmd service (k2 publish run).
    let params: HashMap<String, String> = [
        ("project", f.ws_a_path.to_string_lossy().to_string()),
        ("name", "helper".to_string()),
        ("cmd", "python3 -m http.server".to_string()),
        ("port", "18555".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let r = crate::publish_routes::dispatch_post_as("/cli/publish/run", &params, Who::Owner);
    let v: J = serde_json::from_str(&r.body).unwrap();
    assert_eq!((status(&r), code(&v)), (409, "disk_low"), "{v}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert!(k2_core::published_services::get_by_project_name(&conn, &f.ws_a, "helper").unwrap().is_none(), "no service row");
    }

    // Every refusal is a guard-log line; the view shows the last ten.
    let refused = dg::recent(dg::RECENT).iter().filter(|l| l["event"] == "refused").count();
    assert!(refused >= 6, "{:?}", dg::recent(dg::RECENT));
}

// ── 8: running work is never blocked ─────────────────────────────────

#[test]
fn at_critical_restarts_rollbacks_stops_and_uninstalls_are_never_refused() {
    let f = fx("critical");
    f.free(CRITICAL_FREE);
    for (path, body) in [
        ("/cli/app/rollback", json!({ "app": "plain" })),
        ("/cli/app/start", json!({ "app": "plain" })),
        ("/cli/app/restart", json!({ "app": "plain" })),
        ("/cli/app/stop", json!({ "app": "plain" })),
        ("/cli/app/park", json!({ "app": "plain" })),
        ("/cli/app/uninstall", json!({ "app": "plain" })),
    ] {
        let (_, v) = call(path, &Who::Owner, body);
        assert_ne!(v["error"]["code"], "disk_low", "{path} must never meet the disk guard: {v}");
    }
    // A re-run of an existing legacy --cmd service is never checked; a
    // new one is.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::published_services::insert(
            &conn,
            &f.ws_a,
            "legacy",
            "python3 -m http.server",
            &f.ws_a_path.to_string_lossy(),
            18556,
            k2_core::published_services::EXPOSE_LOCAL,
            k2_core::published_services::DESIRED_STOPPED,
            k2_core::published_services::KIND_CMD,
            "",
        )
        .unwrap();
    }
    let check = |name: &str| {
        crate::publish_routes::publish_disk_check(
            &Who::Owner,
            &f.ws_a,
            name,
            &f.ws_a_path,
            &crate::publish_routes::CmdRoute::Legacy,
            false,
            None,
        )
    };
    assert!(matches!(check("legacy"), Ok(crate::publish_routes::DiskStep::Go(ref x)) if x == &json!({})));
    match check("brand-new") {
        Err(r) => assert!(r.body.contains("disk_low"), "{}", r.body),
        Ok(_) => panic!("a new service at critical is refused"),
    }
}

// ── 9: the owner's override ──────────────────────────────────────────

#[test]
fn the_owners_override_at_refuse_goes_through_once_and_is_audited() {
    let f = fx("owner-override");
    f.free(REFUSE_FREE);
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "seed": SEED_CAPS, "diskOverride": "hotfix for the login bug" }));
    // Past the guard: the first install's caps hold (S20) answers, with
    // the override on it.
    assert_eq!(st, 202, "{v}");
    assert_eq!(v["kind"], "install", "{v}");
    assert_eq!(v["diskOverride"]["applied"], true, "{v}");
    assert_eq!(v["diskOverride"]["via"], "owner");
    assert_eq!(v["diskOverride"]["reason"], "hotfix for the login bug");
    let audit = audit_events("jobs");
    let (_, payload) = audit.iter().find(|(e, _)| e == "disk_override").unwrap_or_else(|| panic!("no disk_override row: {audit:?}"));
    assert_eq!(payload["reason"], "hotfix for the login bug");
    assert_eq!(payload["level"], "refuse");
    assert!(dg::recent(dg::RECENT).iter().any(|l| l["event"] == "override" && l["by"] == "owner"));
    // Once: the next deploy without the flag is refused again.
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "seed": SEED_CAPS }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    // An empty reason isn't an override; an over-long one is refused.
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "seed": SEED_CAPS, "diskOverride": "   " }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "seed": SEED_CAPS, "diskOverride": "x".repeat(501) }));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
}

// ── 10: an agent's override waits for the owner ──────────────────────

#[test]
fn an_agents_override_is_held_and_replays_when_the_owner_approves() {
    let f = fx("agent-override");
    switch(&f, true);
    f.free(REFUSE_FREE);
    let (st, v) = call("/cli/app/deploy", &agent(&f.ws_a), json!({ "app": "jobs", "seed": SEED_CAPS, "diskOverride": "the fix is urgent" }));
    assert_eq!((st, v["status"].as_str(), v["kind"].as_str()), (202, Some("held"), Some("manage")), "{v}");
    let reasons: Vec<String> = v["reasons"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect();
    assert!(reasons.iter().any(|r| r.contains("96% full") && r.contains("the box keeps 10 GiB")), "{reasons:?}");
    assert!(reasons.iter().any(|r| r == "reason: the fix is urgent"), "{reasons:?}");
    assert_eq!(v["disk"]["mount"], "/");
    let id = v["request"]["id"].as_str().unwrap().to_string();
    // The owner approves in person: it replays as the agent, past the guard.
    let out = approve(&id);
    assert_eq!(out["status"], "approved", "{out}");
    assert_eq!(out["result"]["applied"], "/cli/app/deploy", "{out}");
    assert_eq!(out["result"]["answer"]["diskOverride"]["via"], "approved", "{out}");
    assert!(dg::recent(dg::RECENT).iter().any(|l| l["event"] == "held"));

    // Without the switch, an agent's deploy is held (S42) without any
    // override; replayed into a full disk, the guard refuses it.
    switch(&f, false);
    let (st, v) = call("/cli/app/deploy", &agent(&f.ws_a), json!({ "app": "jobs", "seed": SEED_CAPS }));
    assert_eq!((st, v["kind"].as_str()), (202, Some("manage")), "{v}");
    let out = approve(v["request"]["id"].as_str().unwrap());
    assert_eq!(out["status"], "failed", "{out}");
    assert!(out["error"]["hint"].as_str().unwrap().contains("disk_low"), "{out}");
}

// ── 11: critical, and people's held overrides ────────────────────────

#[test]
fn at_critical_every_override_waits_and_peoples_requests_replay_as_the_owner() {
    let f = fx("people");
    f.free(CRITICAL_FREE);
    // The owner token at critical: held, then replayed as the owner.
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "seed": SEED_CAPS, "diskOverride": "must ship" }));
    assert_eq!((st, v["kind"].as_str()), (202, Some("manage")), "{v}");
    assert!(v["hint"].as_str().unwrap().contains("critical"), "{v}");
    assert_eq!(v["request"]["requestedBy"], "owner", "{v}");
    let out = approve(v["request"]["id"].as_str().unwrap());
    assert_eq!((out["status"].as_str(), out["result"]["applied"].as_str()), (Some("approved"), Some("/cli/app/deploy")), "{out}");

    // An Admin login at refuse: held, replayed as the owner.
    f.free(REFUSE_FREE);
    let (st, v) = call("/cli/app/deploy", &login(Role::Admin), json!({ "app": "jobs", "seed": SEED_CAPS, "diskOverride": "admin ask" }));
    assert_eq!((st, v["kind"].as_str()), (202, Some("manage")), "{v}");
    let out = approve(v["request"]["id"].as_str().unwrap());
    assert_eq!((out["status"].as_str(), out["result"]["applied"].as_str()), (Some("approved"), Some("/cli/app/deploy")), "{out}");

    // A K2 shell tab: held, never applied at once; its install replays as
    // the owner with the tab's workspace.
    let (st, v) = call("/cli/app/install", &shell(&f.ws_a), json!({ "app": "tabapp", "diskOverride": "from a tab" }));
    assert_eq!((st, v["kind"].as_str()), (202, Some("manage")), "{v}");
    assert_eq!(v["request"]["body"]["body"]["workspace"], f.ws_a.as_str(), "{v}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert!(store::get_app(&conn, "tabapp").unwrap().is_none(), "held, not applied");
    }
}

// ── 12 (daemon side): mode off is today's behaviour ──────────────────

#[test]
fn mode_off_and_ok_disks_add_nothing_warn_adds_a_warning() {
    let f = fx("modes");
    let folder = app_folder(&f, "quiet");
    // ok: nothing added.
    let (st, v) = call("/cli/app/build", &Who::Owner, json!({ "folder": folder }));
    assert_eq!(st, 200, "{v}");
    assert!(has_no_guard_keys(&v), "{v}");
    // Mode off at critical: exactly as before (no refusal, no new keys).
    f.free(CRITICAL_FREE);
    f.disk.set_mode(Some(Mode::Off));
    let (st, v) = call("/cli/app/build", &Who::Owner, json!({ "folder": folder }));
    assert_eq!(st, 200, "{v}");
    assert!(has_no_guard_keys(&v), "{v}");
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({ "app": "jobs", "seed": SEED_CAPS }));
    assert_eq!((st, v["kind"].as_str()), (202, Some("install")), "{v}");
    assert!(has_no_guard_keys(&v), "{v}");
    // Warn line: it goes through with a diskWarning.
    f.disk.set_mode(Some(Mode::On));
    f.free(WARN_FREE);
    let (st, v) = call("/cli/app/build", &Who::Owner, json!({ "folder": folder }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["diskWarning"]["level"], "warn", "{v}");
    assert_eq!(v["diskWarning"]["wouldRefuse"], false);
    assert!(v["diskWarning"]["hint"].as_str().unwrap().contains("88% full"), "{v}");
    // Mode warn at refuse: a warning that says what on would do.
    f.disk.set_mode(Some(Mode::Warn));
    f.free(REFUSE_FREE);
    let (st, v) = call("/cli/app/build", &Who::Owner, json!({ "folder": folder }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["diskWarning"]["wouldRefuse"], true, "{v}");
    assert!(v["diskWarning"]["hint"].as_str().unwrap().contains("would have been refused"), "{v}");
}

// ── 14: who sees the view ────────────────────────────────────────────

#[test]
fn the_view_shows_folders_to_people_only_and_refuses_app_passes_and_members() {
    let f = fx("view");
    publish_scan(&f);
    f.free(WARN_FREE);
    let (st, v) = get("/cli/app/resources", &Who::Owner, &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["guard"]["mode"], "off", "the setting's mode (the fake only decides checks)");
    assert_eq!(v["guard"]["lines"]["refusePct"], 95);
    let disk = v["disks"].as_array().unwrap().iter().find(|d| d["id"] == "9:9").unwrap_or_else(|| panic!("{v}"));
    assert_eq!(disk["level"], "warn");
    assert!(disk["holds"].as_array().unwrap().iter().any(|h| h == "k2Home"), "{disk}");
    assert!(disk["holds"].as_array().unwrap().iter().any(|h| h == "workspaces"), "{disk}");
    assert!(disk["notMeasuredBytes"].is_u64(), "{disk}");
    let ws_row = |v: &J| v["consumers"].as_array().unwrap().iter().find(|c| c["kind"] == "workspace").cloned().unwrap();
    let row = ws_row(&v);
    assert_eq!(row["folder"], f.ws_a_path.to_string_lossy().as_ref());
    assert_eq!(row["owner"]["workspaceId"], f.ws_a.as_str());
    assert_eq!(row["parts"]["uploads"], 29_000_000_000u64);
    // Apps the scan hasn't measured still have a row, unknown as null.
    let plain = v["consumers"].as_array().unwrap().iter().find(|c| c["id"] == "plain").cloned().unwrap();
    assert!(plain["diskBytes"].is_null(), "{plain}");
    assert_eq!(v["byOwner"][0]["owner"]["handle"], "nsi");
    assert_eq!(v["byOwner"][0]["diskBytes"], 110_200_000_000u64);
    assert!(v["suggestion"].as_str().unwrap().contains("k2 msg nsi::box.k2.dev"), "{v}");
    assert_eq!(v["k2WillClear"]["seedsBytes"], 2_600_000_000u64);

    // An Admin login sees folders too; agents and shell tabs never.
    let (_, v) = get("/cli/app/resources", &login(Role::Admin), &[]);
    assert!(ws_row(&v)["folder"].is_string());
    for who in [agent(&f.ws_b), shell(&f.ws_a)] {
        let (st, v) = get("/cli/app/resources", &who, &[]);
        assert_eq!(st, 200, "{v}");
        let row = ws_row(&v);
        assert!(row.get("folder").is_none(), "{who:?}: {row}");
        assert!(row["owner"].get("workspaceId").is_none(), "{who:?}: {row}");
        assert_eq!(row["owner"]["address"], "nsi::box.k2.dev");
        assert!(v["byOwner"][0]["owner"].get("workspaceId").is_none(), "{v}");
        assert!(!v.to_string().contains(&f.ws_a_path.to_string_lossy().to_string()), "{who:?} sees no path: {v}");
        assert!(!v.to_string().contains(&f.ws_a), "{who:?} sees no workspace id: {v}");
    }
    // App passes and Members never.
    let (st, v) = get("/cli/app/resources", &Who::Wrong, &[]);
    assert_eq!((st, code(&v)), (403, "wrong_credential"), "{v}");
    let (st, v) = get("/cli/app/resources", &login(Role::Member), &[]);
    assert_eq!((st, code(&v)), (403, "role_required"), "{v}");

    // One App's row (k2 app inspect --resources); an unknown App is 404.
    let (st, v) = get("/cli/app/resources", &agent(&f.ws_a), &[("app", "jobs")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["consumers"].as_array().unwrap().len(), 1);
    assert_eq!(v["consumers"][0]["diskBytes"], 6_200_000_000u64);
    let (st, v) = get("/cli/app/resources", &Who::Owner, &[("app", "nope")]);
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    let (_, v) = get("/cli/app/resources", &Who::Owner, &[("owner", "nsi")]);
    assert!(v["consumers"].as_array().unwrap().iter().all(|c| c["owner"]["handle"] == "nsi"), "{v}");

    // The box view's disk usage comes from the same scan.
    let (_, v) = get("/cli/app/list", &Who::Owner, &[("box", "1")]);
    let jobs = v["apps"].as_array().unwrap().iter().find(|a| a["appId"] == "jobs").cloned().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(jobs["disk"]["usedBytes"], 6_200_000_000u64, "{jobs}");
    let _ = &f.ws_b;
}

// ── 15: the settings are the owner's ─────────────────────────────────

#[test]
fn only_the_owner_changes_the_lines_and_the_next_check_uses_them() {
    let f = fx("settings");
    let folder = app_folder(&f, "lines");
    for who in [agent(&f.ws_a), shell(&f.ws_a), login(Role::Admin)] {
        let (st, v) = call("/cli/app/resources/settings", &who, json!({ "refusePct": 90 }));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("k2 app resources settings --refuse 90"), "{v}");
    }
    assert_eq!(dg::load_settings(), dg::Settings::default(), "nothing saved");
    // A bad line names its rule.
    let (st, v) = call("/cli/app/resources/settings", &Who::Owner, json!({ "warnPct": 97 }));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("warn < refuse < critical"), "{v}");
    let (st, v) = call("/cli/app/resources/settings", &Who::Owner, json!({ "refuse": 90 }));
    assert_eq!((st, code(&v)), (400, "bad_request"), "a typo is refused: {v}");
    // The owner (an Owner login too) saves; the settings decide the mode.
    let (st, v) = call("/cli/app/resources/settings", &login(Role::Owner), json!({ "mode": "on", "refusePct": 90 }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["changed"], true);
    assert_eq!(v["guard"]["lines"]["refusePct"], 90);
    assert_eq!(dg::load_settings().lines.refuse_pct, 90);
    assert!(dg::recent(dg::RECENT).iter().any(|l| l["event"] == "settings" && l["by"] == "owner:owner"));
    // 91% used: under the default 95, past the owner's 90.
    f.disk.set_mode(None);
    f.free(90 * GIB);
    let (st, v) = call("/cli/app/build", &Who::Owner, json!({ "folder": folder }));
    assert_eq!((st, code(&v)), (409, "disk_low"), "{v}");
    let (st, v) = call("/cli/app/resources/settings", &Who::Owner, json!({ "mode": "off" }));
    assert_eq!(st, 200, "{v}");
    let (st, v) = call("/cli/app/build", &Who::Owner, json!({ "folder": folder }));
    assert_eq!(st, 200, "{v}");
    assert!(has_no_guard_keys(&v), "{v}");
    // An empty body answers the current settings without writing.
    let (st, v) = call("/cli/app/resources/settings", &Who::Owner, json!({}));
    assert_eq!((st, v["changed"].as_bool()), (200, Some(false)), "{v}");
    assert_eq!(v["guard"]["mode"], "off");
}

// ── rescan ───────────────────────────────────────────────────────────

#[test]
fn rescan_answers_without_a_scanner_and_is_rate_limited_with_one() {
    let f = fx("rescan");
    let (st, v) = call("/cli/app/resources/rescan", &agent(&f.ws_a), json!({}));
    assert_eq!((st, v["scanner"].as_bool()), (200, Some(false)), "{v}");
    dg::register_scanner();
    let (_, v) = call("/cli/app/resources/rescan", &agent(&f.ws_a), json!({}));
    assert_eq!(v["started"], true, "{v}");
    let (_, v) = call("/cli/app/resources/rescan", &Who::Owner, json!({}));
    assert_eq!(v["running"], true, "{v}");
    let (st, v) = call("/cli/app/resources/rescan", &Who::Wrong, json!({}));
    assert_eq!((st, code(&v)), (403, "wrong_credential"), "{v}");
}

// ── C6 through the guest door: no names, no numbers ──────────────────

#[test]
fn a_guest_upload_past_the_reserve_is_no_space_naming_nobody() {
    let f = fx("guest");
    f.free(10 * GIB + 8 * dg::MIB);
    let dir = f.ws_a_path.join("inbox");
    std::fs::create_dir_all(&dir).unwrap();
    let part = k2_core::fs_guest_upload::guest_part_name("guest-1", &f.ws_a, &dir, "big.bin", "up-1");
    let err = k2_core::fs_guest_upload::write_guest_upload_chunk(&dir, &part, "big.bin", 0, &[0u8; 1024], false, Some(64 * dg::MIB))
        .expect_err("past the reserve");
    let msg = err.to_string();
    assert!(matches!(err, k2_core::fs_guest_upload::GuestChunkError::NoSpace(_)), "{err:?}");
    assert_eq!(msg, dg::GUEST_NO_SPACE);
    assert!(!msg.chars().any(|c| c.is_ascii_digit()), "{msg}");
    assert!(!msg.contains("nsi") && !msg.contains('/'), "{msg}");
    // The owner's upload gets the plain message with the numbers.
    let e = k2_core::fs_commands::write_upload(&dir.to_string_lossy(), "big.bin", &vec![0u8; 16 * 1024 * 1024]).expect_err("past the reserve");
    assert!(e.contains("this server keeps 10 GiB free"), "{e}");
    // Under the reserve both go through.
    f.free(OK_FREE);
    k2_core::fs_commands::write_upload(&dir.to_string_lossy(), "small.bin", b"ok").expect("room to spare");
}

// ── 16: route policy and the agent socket ────────────────────────────

#[test]
fn the_routes_are_classified_post_only_where_they_change_things_and_agent_verbs() {
    use crate::routes::route_policy::{get_refused, lookup};
    assert!(!get_refused("/cli/app/resources", ""));
    assert!(get_refused("/cli/app/resources/rescan", ""));
    assert!(get_refused("/cli/app/resources/settings", ""));
    for p in super::resources::ROUTES {
        assert!(lookup(p).is_some(), "{p} is in the route policy");
        assert!(crate::session_token::is_agent_verb(p), "{p} is an agent verb");
        assert!(super::routes::is_route(p), "{p} is an App route (both doors)");
    }
    for p in super::resources::POST_ROUTES {
        assert!(super::routes::is_post_route(p), "{p} is POST-only");
    }
    assert!(!super::routes::is_post_route("/cli/app/resources"));
    assert!(k2_core::deploy::RESERVED_APP_IDS.contains(&"resources"));
}
