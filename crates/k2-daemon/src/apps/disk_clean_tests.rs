//! `k2 app resources clean` (prd-app-resources-disk-guard-v1 Slice B,
//! 0.46.4), in process under a temp `$HOME`: what it lists, that a dry run
//! removes nothing, that a real run removes exactly that and never App
//! data, a kept Seed or the current base archive, and who may clean what.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use k2_core::connect_users::Role;
use k2_core::deploy::store;
use k2_core::disk_guard as dg;
use k2_root_proto::engine::{CleanAnswer, CleanImage, CleanLeftover, SeedsGcAnswer};
use rusqlite::params;
use serde_json::{json, Value as J};

use crate::apps::routes::Who;
use super::{Scope, EngineClean};

const DAY: i64 = 86_400;

fn hash(c: char) -> String {
    c.to_string().repeat(64)
}

struct Fx {
    root: PathBuf,
    ws_a: String,
    ws_a_path: PathBuf,
    ws_b: String,
    now: i64,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
}

impl Drop for Fx {
    fn drop(&mut self) {
        dg::clear_snapshot();
    }
}

fn write(p: &Path, n: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![7u8; n]).unwrap();
}

fn age(p: &Path, secs: i64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs as u64);
    std::fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
}

/// Two workspaces; `ws_a` owns the Apps `jobs` (walled) and `plain`
/// (process). On disk: one old unused Seed of `plain` and one of `jobs`'s
/// live Seed, `plain`'s 12 deployments (the two oldest past the keep line),
/// rotated logs (and an egress one), a `--cmd` log of each workspace, App
/// data, build temps old and new, a stale and a current base archive.
fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    dg::clear_snapshot();
    let root = home.path().to_path_buf();
    let k2 = k2_core::paths::k2_home();
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    let ws_a_path = std::fs::canonicalize(&root).unwrap().join("ws-a");
    std::fs::create_dir_all(&ws_a_path).unwrap();
    std::fs::create_dir_all(root.join("ws-b")).unwrap();
    let now = k2_core::deploy::now();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (ws, p) in [(&ws_a, ws_a_path.clone()), (&ws_b, root.join("ws-b"))] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'Agent', ?2, 'hand-' || substr(?1, 1, 4))",
                params![ws, p.to_string_lossy()],
            )
            .unwrap();
        }
        for (app, iso) in [("jobs", "container"), ("plain", "process")] {
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &format!("svc-{app}-{tag}"),
                    seed_id: None,
                    owner_workspace: &ws_a,
                    isolation: iso,
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        // Seeds: `a` unused for 40 days (plain's), `b` live (jobs), `c`
        // unused and old but signed by nobody's App (the owner's build).
        for (h, built_for, signer, used) in [
            (hash('a'), Some("plain"), "agent:hand-ws-a", now - 40 * DAY),
            (hash('b'), Some("jobs"), "agent:hand-ws-a", now - 40 * DAY),
            (hash('c'), None, "owner", now - 40 * DAY),
        ] {
            conn.execute(
                "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at, built_for) \
                 VALUES (?1, 'not-an-app', 'cmd', '{}', '{}', '{\"ok\":true}', ?2, 8192, ?3, ?3, ?4)",
                params![h, signer, used, built_for],
            )
            .unwrap();
            write(&k2_core::deploy::seed::seed_path(&h).join("server.js"), 8192);
        }
        conn.execute("UPDATE apps SET live_seed = ?1 WHERE app_id = 'jobs'", params![hash('b')]).unwrap();
        for i in 1..=12 {
            conn.execute(
                "INSERT INTO deployments (id, app_id, kind, actor, result, started_at, finished_at) VALUES (?1, 'plain', 'deploy', 'owner', 'superseded', ?2, ?2)",
                params![format!("d-{i:02}"), now - 40 * DAY + i],
            )
            .unwrap();
        }
    }
    for i in 1..=12 {
        write(&k2_core::deploy::deployment_log_path("plain", &format!("d-{i:02}")), 2048);
    }
    write(&k2.join("apps/plain/logs/d-12.log.1"), 4096);
    write(&k2.join("apps/plain/logs/egress.log.1"), 4096);
    write(&k2.join("apps/plain/data/db.sqlite"), 65536);
    write(&k2.join("apps/plain/cache/c.bin"), 4096);
    write(&k2.join(format!("publish-logs/{ws_a}/site.log.1")), 4096);
    write(&k2.join(format!("publish-logs/{ws_a}/site.log")), 4096);
    write(&k2.join(format!("publish-logs/{ws_b}/other.log.1")), 4096);
    write(&k2.join("apps/tmp/build-old.tar"), 4096);
    age(&k2.join("apps/tmp/build-old.tar"), 2 * 3600);
    write(&k2.join("apps/tmp/build-now.tar"), 4096);
    write(&k2.join("apps/images/node24-000000000000.oci.tar"), 4096);
    let current = super::current_bases();
    for b in &current {
        write(&k2.join("apps/images").join(super::archive_name(b)), 4096);
    }
    Fx { root, ws_a, ws_a_path, ws_b, now, _db: db, _home: home }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn login(role: Role) -> Who {
    Who::Login { username: format!("{role:?}").to_ascii_lowercase(), role }
}

fn status(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split_whitespace().next().unwrap().parse().unwrap()
}

fn clean(who: &Who, body: J) -> (u16, J) {
    let r = super::super::routes::handle(super::ROUTE, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    (status(&r), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

fn ids(v: &J, kind: &str) -> Vec<String> {
    let mut out: Vec<String> = v["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items: {v}"))
        .iter()
        .filter(|i| i["kind"] == kind)
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn k2(p: &str) -> PathBuf {
    k2_core::paths::k2_home().join(p)
}

fn untouched(f: &Fx) {
    let _ = f;
    for p in ["apps/plain/data/db.sqlite", "apps/plain/cache/c.bin", "apps/plain/logs/egress.log.1", "apps/tmp/build-now.tar"] {
        assert!(k2(p).is_file(), "{p} must never be cleaned");
    }
    assert!(k2_core::deploy::seed::seed_path(&hash('b')).is_dir(), "the live Seed stays");
    for b in super::current_bases() {
        assert!(k2(&format!("apps/images/{}", super::archive_name(&b))).is_file(), "the current base archive stays");
    }
}

#[test]
fn a_dry_run_lists_what_k2_made_and_removes_nothing() {
    let f = fx("dry");
    let (st, v) = clean(&Who::Owner, json!({ "dryRun": true }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["dryRun"], true);
    assert_eq!(v["scope"], "box");
    assert_eq!(ids(&v, "seed"), vec![short('a'), short('c')], "unused 30 days, not live: {v}");
    assert_eq!(ids(&v, "deploymentLog"), vec!["apps/plain/logs/d-01.log", "apps/plain/logs/d-02.log"], "past the last 10: {v}");
    assert_eq!(
        ids(&v, "rotatedLog"),
        vec![
            "apps/plain/logs/d-12.log.1".to_string(),
            format!("publish-logs/{}/site.log.1", f.ws_a),
            format!("publish-logs/{}/other.log.1", f.ws_b),
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>(),
        "never egress.log.1: {v}"
    );
    assert_eq!(ids(&v, "buildTemp"), vec!["apps/tmp/build-old.tar"], "only an hour-old temp: {v}");
    assert_eq!(ids(&v, "imageArchive"), vec!["apps/images/node24-000000000000.oci.tar"], "only a base K2 no longer pins: {v}");
    assert!(v["items"].as_array().unwrap().iter().all(|i| i["removed"] == false), "{v}");
    assert!(v["totals"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(v["totals"]["freedBytes"], 0);
    assert!(v["never"].as_str().unwrap().contains("App data"));
    let all = v.to_string();
    assert!(!all.contains(&f.root.to_string_lossy().to_string()), "no absolute path in the answer: {all}");
    // Nothing moved.
    assert!(k2_core::deploy::seed::seed_path(&hash('a')).is_dir());
    assert!(k2("apps/plain/logs/d-01.log").is_file());
    assert!(k2("apps/tmp/build-old.tar").is_file());
    untouched(&f);
    assert!(!dg::recent(10).iter().any(|l| l["event"] == "clean"), "a dry run logs nothing");
}

fn short(c: char) -> String {
    hash(c)[..12].to_string()
}

#[test]
fn a_real_clean_removes_exactly_that_and_never_app_data() {
    let f = fx("real");
    let (_, plan) = clean(&Who::Owner, json!({ "dryRun": true }));
    let (st, v) = clean(&login(Role::Admin), json!({}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["dryRun"], false);
    for kind in ["seed", "deploymentLog", "rotatedLog", "buildTemp", "imageArchive"] {
        assert_eq!(ids(&v, kind), ids(&plan, kind), "{kind}: the run frees what the dry run listed");
    }
    assert!(v["items"].as_array().unwrap().iter().all(|i| i["removed"] == true && i.get("error").is_none()), "{v}");
    assert_eq!(v["totals"]["freedBytes"], v["totals"]["bytes"]);
    assert!(!k2_core::deploy::seed::seed_path(&hash('a')).exists());
    assert!(!k2("apps/plain/logs/d-01.log").exists() && k2("apps/plain/logs/d-03.log").is_file(), "the last 10 stay");
    assert!(!k2(&format!("publish-logs/{}/site.log.1", f.ws_a)).exists());
    assert!(k2(&format!("publish-logs/{}/site.log", f.ws_a)).is_file(), "the live log stays");
    assert!(!k2("apps/tmp/build-old.tar").exists());
    untouched(&f);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(store::get_seed(&conn, &hash('a')).unwrap().is_none(), "the swept Seed's row goes");
        assert!(store::get_seed(&conn, &hash('b')).unwrap().is_some());
    }
    let log = dg::recent(10);
    let line = log.iter().find(|l| l["event"] == "clean").unwrap_or_else(|| panic!("a clean line: {log:?}"));
    assert_eq!(line["by"], "user:admin");
    // Again: nothing left to clean.
    let (_, again) = clean(&Who::Owner, json!({ "dryRun": true }));
    assert!(again["items"].as_array().unwrap().is_empty(), "{again}");
    assert!(again["hint"].as_str().unwrap().starts_with("Nothing to clean"));
}

#[test]
fn an_agent_cleans_only_its_own_and_needs_the_switch() {
    let f = fx("agent");
    // ws_b owns no App: only its own --cmd log.
    let (st, v) = clean(&agent(&f.ws_b), json!({ "dryRun": true }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["scope"], "workspace");
    let all: Vec<String> = v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(all, vec![format!("publish-logs/{}/other.log.1", f.ws_b)], "{v}");
    // ws_a: its Apps' logs and Seeds; never the box's temps, archives or
    // the owner's Seed.
    let (_, v) = clean(&agent(&f.ws_a), json!({ "dryRun": true }));
    assert_eq!(ids(&v, "seed"), vec![short('a')], "{v}");
    assert_eq!(ids(&v, "deploymentLog").len(), 2);
    assert!(ids(&v, "rotatedLog").iter().all(|i| !i.contains(&f.ws_b)), "{v}");
    assert!(ids(&v, "buildTemp").is_empty() && ids(&v, "imageArchive").is_empty(), "the box's own: {v}");
    assert!(v["engine"]["status"].is_null(), "agents never reach the engine's images");
    // A real clean needs "Build and run Apps".
    let (st, v) = clean(&agent(&f.ws_a), json!({}));
    assert_eq!(st, 403, "{v}");
    assert_eq!(v["error"]["code"], "apps_manage_off");
    assert!(k2_core::deploy::seed::seed_path(&hash('a')).is_dir());
    k2_core::workspace::settings::set_agents_can_manage_skin(&f.ws_a_path.to_string_lossy(), true).unwrap();
    let (st, v) = clean(&agent(&f.ws_a), json!({}));
    assert_eq!(st, 200, "{v}");
    assert!(!k2_core::deploy::seed::seed_path(&hash('a')).exists());
    assert!(k2_core::deploy::seed::seed_path(&hash('c')).is_dir(), "the owner's Seed is not the agent's");
    assert!(k2(&format!("publish-logs/{}/other.log.1", f.ws_b)).is_file(), "another agent's log stays");
    assert!(k2("apps/tmp/build-old.tar").is_file());
    untouched(&f);
}

#[test]
fn readers_only_and_post_only() {
    let _f = fx("who");
    // A Member changes nothing on any App route (a POST), dry run or not.
    let (st, v) = clean(&login(Role::Member), json!({ "dryRun": true }));
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("member_read_only")), "{v}");
    let (st, v) = clean(&Who::Wrong, json!({ "dryRun": true }));
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("wrong_credential")), "{v}");
    assert!(super::super::routes::is_post_route(super::ROUTE));
    assert!(!super::super::routes::is_post_route(super::super::trends::ROUTE));
}

struct FakeEngine(Mutex<Vec<(bool, Vec<String>)>>);

impl EngineClean for FakeEngine {
    fn status(&self) -> String {
        "ok".into()
    }
    fn clean(&self, dry_run: bool, keep: &[String]) -> Result<CleanAnswer, String> {
        self.0.lock().unwrap().push((dry_run, keep.to_vec()));
        Ok(CleanAnswer {
            dry_run,
            images: vec![CleanImage { id: "sha256:abcdef0123456789".into(), names: vec![], bytes: 1000, removed: !dry_run, error: None }],
            leftovers: vec![CleanLeftover { name: "engine/builds/.tar-jobs-b1.tar".into(), bytes: 10, removed: !dry_run }],
            freed_bytes: if dry_run { 0 } else { 1010 },
            images_error: None,
        })
    }
}

struct NoGc;

impl super::super::disk_watch::EngineSeeds for NoGc {
    fn can_gc(&self) -> bool {
        false
    }
    fn hashes(&self) -> Vec<String> {
        vec![]
    }
    fn gc(&self, _: &[String]) -> Result<SeedsGcAnswer, String> {
        panic!("never called without can_gc")
    }
}

#[test]
fn the_box_clean_asks_the_engine_and_keeps_the_current_bases() {
    let f = fx("engine");
    let e = FakeEngine(Mutex::new(vec![]));
    let v = super::run(&Scope::Box, true, f.now, &e, &NoGc, "owner").unwrap_or_else(|r| panic!("{}: {}", r.code, r.hint));
    assert_eq!(v["engine"]["status"], "ok");
    assert_eq!(ids(&v, "image"), vec!["abcdef012345"]);
    assert_eq!(ids(&v, "engineLeftover"), vec!["engine/builds/.tar-jobs-b1.tar"]);
    let calls = e.0.lock().unwrap().clone();
    let want: Vec<String> = super::current_bases().iter().map(|b| b.local()).collect();
    assert_eq!(calls, vec![(true, want)], "a dry run asks for a dry run, keeping K2's current bases");
    let ws = Scope::Workspace { id: f.ws_a.clone(), signer: "agent:x".into() };
    let _ = super::run(&ws, true, f.now, &e, &NoGc, "agent:x").unwrap_or_else(|r| panic!("{}", r.hint));
    assert_eq!(e.0.lock().unwrap().len(), 1, "a workspace clean never reaches the engine");
}
