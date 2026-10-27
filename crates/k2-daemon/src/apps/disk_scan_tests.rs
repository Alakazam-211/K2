//! Box usage, measuring (prd-app-resources-disk-guard-v1 A1; PRD tests
//! 19 daemon side and 20): the folder walk, the engine client that asks
//! only an engine listing the verb, Postgres sizes through the helper's
//! allowed argv, `/proc` memory, and the rows with their owners.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_core::deploy::store;
use rusqlite::params;

use super::*;

fn far() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn write(p: &Path, n: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![7u8; n]).unwrap();
}

/// Test 20: a hard link counts once, a symlink is never followed, a
/// nested workspace is left to its own row, parts are per top-level
/// child, and a passed deadline marks the result partial.
#[test]
fn the_walk_counts_like_du_and_each_byte_once() {
    let home = crate::test_support::TempHome::new();
    let root = home.path().join("ws");
    write(&root.join("uploads/a.bin"), 256 * 1024);
    write(&root.join("node_modules/x/index.js"), 64 * 1024);
    std::fs::hard_link(root.join("uploads/a.bin"), root.join("uploads/a-link.bin")).unwrap();
    k2_core::test_env::symlink(std::path::MAIN_SEPARATOR_STR, root.join("to-root")).unwrap();
    write(&root.join("inner/big.bin"), 512 * 1024);
    write(&root.join("README.md"), 100);
    let skip = [root.join("inner")];
    let f = walk(&root, &WalkOpts { skip: &skip, deadline: far() }, 42).expect("the root exists");
    assert!(!f.partial);
    assert_eq!(f.scanned_at, 42);
    let up = f.parts["uploads"];
    assert!((256 * 1024..512 * 1024).contains(&up), "the hard link counted once: {up}");
    assert!(f.parts["node_modules"] >= 64 * 1024);
    assert!(!f.parts.contains_key("inner"), "the nested workspace is its own row: {:?}", f.parts);
    assert!(f.parts["to-root"] < 64 * 1024, "the symlink is counted as itself, not followed");
    assert!(f.bytes < 512 * 1024 + 256 * 1024, "the skipped folder adds nothing: {}", f.bytes);
    let sum: u64 = f.parts.values().sum();
    assert!(f.bytes >= sum, "the total holds every part");
    // A deadline already gone: partial, with what it had.
    let p = walk(&root, &WalkOpts { skip: &[], deadline: Instant::now() }, 1).unwrap();
    assert!(p.partial);
    assert!(walk(&home.path().join("nope"), &WalkOpts { skip: &[], deadline: far() }, 1).is_none());
    // The view keeps the biggest three.
    let top = top_parts(&f.parts, 2);
    assert_eq!(top.keys().cloned().collect::<Vec<_>>(), vec!["node_modules".to_string(), "uploads".to_string()]);
}

#[test]
fn k2_parts_use_the_views_words() {
    let raw: BTreeMap<String, u64> = [
        ("apps", 10u64),
        ("k2.db", 5),
        ("k2.db-wal", 1),
        ("k2-threads.redb", 2),
        ("publish-logs", 3),
        ("models", 7),
        ("bin", 1),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let p = k2_parts(&raw);
    assert_eq!(p["apps"], 10);
    assert_eq!(p["db"], 8);
    assert_eq!(p["publishLogs"], 3);
    assert_eq!(p["other"], 8);
}

/// `psql -tA` through the helper: the argv stays inside what the helper
/// allows, rows parse, and a failure is recorded, not hidden.
#[test]
fn postgres_sizes_come_from_one_allowed_helper_call() {
    struct Ops {
        out: Result<String, String>,
        seen: std::sync::Mutex<Vec<(Vec<String>, String)>>,
    }
    impl crate::sql::sysops::SystemOps for Ops {
        fn write_file(&self, _: &str, _: &[u8], _: u32) -> Result<(), String> {
            panic!("no writes")
        }
        fn read_file(&self, _: &str) -> Result<Vec<u8>, String> {
            panic!("no reads")
        }
        fn create_dir_all(&self, _: &str) -> Result<(), String> {
            panic!("no mkdir")
        }
        fn remove_path(&self, _: &str) -> Result<(), String> {
            panic!("no rm")
        }
        fn path_exists(&self, _: &str) -> bool {
            false
        }
        fn systemctl(&self, _: &[&str]) -> Result<String, String> {
            panic!("no systemctl")
        }
        fn systemctl_query(&self, _: &[&str]) -> String {
            panic!("no systemctl")
        }
        fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
            assert_eq!(args[0], "psql");
            assert!(crate::sql::sysops::helper_psql_argv_ok(&args[1..]), "{args:?}");
            self.seen
                .lock()
                .unwrap()
                .push((args.iter().map(|s| s.to_string()).collect(), String::from_utf8(stdin.unwrap().to_vec()).unwrap()));
            self.out.clone()
        }
        fn run_cmd(&self, _: &str, _: &[&str], _: &[(&str, &str)], _: Option<&[u8]>) -> Result<Vec<u8>, String> {
            panic!("no client tools")
        }
        fn sleep_ms(&self, _: u64) {}
    }
    let ops = Ops {
        out: Ok("db|jobs_db|58200000000\ndb|postgres|7000000\nwal||1100000000\ndir|/var/lib/postgresql/16/main|0\n".into()),
        seen: Default::default(),
    };
    assert_eq!(db_scan_with(&ops, false, 9), None, "no sidecar running: no call");
    assert!(ops.seen.lock().unwrap().is_empty());
    let s = db_scan_with(&ops, true, 9).unwrap();
    assert_eq!(s.databases["jobs_db"], 58_200_000_000);
    assert_eq!(s.databases["postgres"], 7_000_000);
    assert_eq!(s.wal_bytes, 1_100_000_000);
    assert_eq!(s.data_directory.as_deref(), Some("/var/lib/postgresql/16/main"));
    assert_eq!(s.at, 9);
    assert!(s.error.is_none());
    let seen = ops.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].1.contains("pg_database_size") && seen[0].1.contains("pg_ls_waldir") && seen[0].1.contains("data_directory"));
    let bad = Ops { out: Err("helper psql: exit 2: nope".into()), seen: Default::default() };
    let e = db_scan_with(&bad, true, 9).unwrap();
    assert_eq!(e.error.as_deref(), Some("helper psql: exit 2: nope"));
    assert!(e.databases.is_empty());
}

/// PG B (several clusters, one per major): every K2 cluster is measured,
/// the 5432 one with today's argv and the others with `--cluster <M>`;
/// databases and WAL add up, each database keeps its own cluster's data
/// directory, and one cluster failing never hides the others.
#[test]
fn postgres_sizes_cover_every_k2_cluster() {
    struct Ops {
        fail_17: bool,
        seen: std::sync::Mutex<Vec<Vec<String>>>,
    }
    impl crate::sql::sysops::SystemOps for Ops {
        fn write_file(&self, _: &str, _: &[u8], _: u32) -> Result<(), String> {
            panic!("no writes")
        }
        fn read_file(&self, _: &str) -> Result<Vec<u8>, String> {
            panic!("no reads")
        }
        fn create_dir_all(&self, _: &str) -> Result<(), String> {
            panic!("no mkdir")
        }
        fn remove_path(&self, _: &str) -> Result<(), String> {
            panic!("no rm")
        }
        fn path_exists(&self, _: &str) -> bool {
            false
        }
        fn systemctl(&self, _: &[&str]) -> Result<String, String> {
            panic!("no systemctl")
        }
        fn systemctl_query(&self, _: &[&str]) -> String {
            panic!("no systemctl")
        }
        fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
            assert_eq!(args[0], "psql");
            assert!(crate::sql::sysops::helper_psql_argv_ok(&args[1..]), "{args:?}");
            assert!(String::from_utf8(stdin.expect("sql on stdin").to_vec()).unwrap().contains("pg_database_size"));
            self.seen.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect());
            if args[1] == "--cluster" {
                assert_eq!(args[2], "17", "{args:?}");
                if self.fail_17 {
                    return Err("helper psql: exit 2: no cluster 17".into());
                }
                return Ok("db|jobs_db|300\nwal||20\ndir|/var/lib/postgresql/17/main|0\n".into());
            }
            Ok("db|old_db|100\ndb|postgres|7\nwal||10\ndir|/var/lib/postgresql/16/main|0\n".into())
        }
        fn run_cmd(&self, _: &str, _: &[&str], _: &[(&str, &str)], _: Option<&[u8]>) -> Result<Vec<u8>, String> {
            panic!("no client tools")
        }
        fn sleep_ms(&self, _: u64) {}
    }
    let clusters = [
        crate::sql::cluster::Cluster::layout(16, 5432, "baked"),
        crate::sql::cluster::Cluster::layout(17, 5433, "created"),
    ];
    let ops = Ops { fail_17: false, seen: Default::default() };
    assert_eq!(db_scan_clusters_with(&ops, &clusters, false, 9), None, "no sidecar running: no call");
    let s = db_scan_clusters_with(&ops, &clusters, true, 9).unwrap();
    assert_eq!(s.databases["old_db"], 100);
    assert_eq!(s.databases["jobs_db"], 300);
    assert_eq!(s.databases["postgres"], 7);
    assert_eq!(s.wal_bytes, 30, "WAL of both clusters");
    assert_eq!(s.data_directory.as_deref(), Some("/var/lib/postgresql/16/main"), "the 5432 cluster's");
    assert_eq!(s.data_directory_of("jobs_db"), Some("/var/lib/postgresql/17/main"));
    assert_eq!(s.data_directory_of("old_db"), Some("/var/lib/postgresql/16/main"));
    assert_eq!(s.data_directories(), vec!["/var/lib/postgresql/16/main".to_string(), "/var/lib/postgresql/17/main".to_string()]);
    assert_eq!(s.clusters.len(), 2);
    assert!(s.error.is_none());
    let seen = ops.seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one call per cluster");
    assert_eq!(seen[0][1], "-d", "the 5432 cluster keeps today's argv: {:?}", seen[0]);
    assert_eq!(&seen[1][1..3], ["--cluster", "17"]);
    drop(seen);

    let ops = Ops { fail_17: true, seen: Default::default() };
    let s = db_scan_clusters_with(&ops, &clusters, true, 9).unwrap();
    assert!(s.error.is_none(), "one cluster answered");
    assert_eq!(s.clusters[1].error.as_deref(), Some("helper psql: exit 2: no cluster 17"));
    assert_eq!(s.databases.get("jobs_db"), None);
    assert_eq!(s.databases["old_db"], 100);
}

/// Linux (the A0 contract's untested paths: the mountinfo parse and
/// `libc::major`/`minor`): the probe agrees with `df` and `stat` on the
/// root and on a tmpfs.
#[cfg(target_os = "linux")]
#[test]
fn linux_probe_matches_df_and_stat() {
    for p in ["/", "/dev/shm"] {
        let f = dg::probe(Path::new(p)).unwrap_or_else(|| panic!("probe {p}"));
        let df = std::process::Command::new("df").args(["-B1", "--output=target,fstype,size,avail", p]).output().unwrap();
        assert!(df.status.success(), "df {p}");
        let text = String::from_utf8(df.stdout).unwrap();
        let cols: Vec<&str> = text.lines().nth(1).expect("df row").split_whitespace().collect();
        assert_eq!(f.mount.as_deref(), Some(cols[0]), "{p}: {f:?} vs {text}");
        assert_eq!(f.fs_type.as_deref(), Some(cols[1]), "{p}: {f:?} vs {text}");
        assert_eq!(f.total_bytes, cols[2].parse::<u64>().unwrap(), "{p}");
        let avail: u64 = cols[3].parse().unwrap();
        assert!(f.free_bytes.abs_diff(avail) < 256 * 1024 * 1024, "{p}: free {} vs df {avail}", f.free_bytes);
        let st = std::process::Command::new("stat").args(["-c", "%Hd:%Ld", p]).output().unwrap();
        assert_eq!(f.id, String::from_utf8(st.stdout).unwrap().trim(), "{p}: the device id is st_dev's major:minor");
    }
    // A tracked group puts two paths on one fs once.
    let g = dg::group(&[("a".into(), PathBuf::from("/")), ("b".into(), PathBuf::from("/usr")), ("c".into(), PathBuf::from("/dev/shm"))]);
    assert!(g.len() >= 2 && g.iter().any(|(_, l)| l.contains(&"c".to_string())), "{g:?}");
}

#[test]
fn proc_stat_gives_session_and_rss() {
    // A comm with spaces and parens; session 4242, rss 300 pages.
    let stat = "4250 (node (worker) x) S 4242 4250 4242 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 11 0 777 1000000 300 18446744073709551615 1 1 0 0 0 0 0 0 0";
    assert_eq!(parse_proc_stat(stat), Some((4242, 300)));
    assert_eq!(parse_proc_stat("garbage"), None);
    #[cfg(target_os = "linux")]
    {
        // This test process's own session has resident memory.
        let sid = unsafe { libc::getsid(0) };
        let m = rss_by_session();
        let b = *m.get(&sid).unwrap_or_else(|| panic!("session {sid} not found in /proc"));
        assert!(b > 1024 * 1024, "session {sid}: {b} bytes");
    }
}

/// A fake engine on a socket: answers `version` (with or without the
/// verbs list) and `disk-usage`.
// Server-only feature (Linux boxes): the App engine (k2-app-engine) is a Unix
// socket; not shipped on personal Windows machines.
#[cfg(unix)]
fn fake_engine(dir: &Path, with_verbs: bool, conns: usize) -> (PathBuf, std::thread::JoinHandle<Vec<String>>) {
    use std::os::unix::net::UnixListener;
    let sock = dir.join("e.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let t = std::thread::spawn(move || {
        let mut verbs = Vec::new();
        for _ in 0..conns {
            let (c, _) = l.accept().unwrap();
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            let verb = req["verb"].as_str().unwrap().to_string();
            let mut w = c;
            let ans = match verb.as_str() {
                "version" if with_verbs => r#"{"ok":true,"data":{"protocol":3,"verbs":["disk-usage","seeds-gc"]}}"#.to_string(),
                "version" => r#"{"ok":true,"data":{"protocol":3,"features":["build"]}}"#.to_string(),
                "disk-usage" => r#"{"ok":true,"data":{"apps":{"jobs":{"data":4800,"cache":100,"logs":50,"other":0,"partial":false}},"seeds":{"aa":1300},"images":{"totalBytes":4000,"unusedBytes":1200},"builds":52,"tookMs":9}}"#.to_string(),
                other => format!(r#"{{"ok":false,"code":"bad_request","message":"unknown verb {other}"}}"#),
            };
            w.write_all(ans.as_bytes()).unwrap();
            w.write_all(b"\n").unwrap();
            verbs.push(verb);
        }
        verbs
    });
    (sock, t)
}

/// Test 19 (daemon side): an engine without `verbs` is `old` and never
/// gets the new verb; one that lists it is asked and parsed; none is
/// `missing`.
// Server-only feature (Linux boxes): k2-app-engine's Unix socket.
#[cfg(unix)]
#[test]
fn the_engine_is_asked_only_when_it_lists_the_verb() {
    let d = PathBuf::from(format!("/tmp/k2ds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("old")).unwrap();
    std::fs::create_dir_all(d.join("new")).unwrap();
    let (sock, t) = fake_engine(&d.join("old"), false, 1);
    let s = engine_scan_at(&sock, 5);
    assert_eq!(s.status, "old");
    assert!(s.usage.is_none() && !s.seeds_gc);
    assert_eq!(t.join().unwrap(), vec!["version"], "an old engine never sees disk-usage");
    let (sock, t) = fake_engine(&d.join("new"), true, 2);
    let s = engine_scan_at(&sock, 6);
    assert_eq!(s.status, "ok", "{s:?}");
    assert!(s.seeds_gc);
    let u = s.usage.unwrap();
    assert_eq!(u.apps["jobs"].total(), 4950);
    assert_eq!(u.seeds["aa"], 1300);
    assert_eq!(u.images.unwrap().unused_bytes, 1200);
    assert_eq!(t.join().unwrap(), vec!["version", "disk-usage"]);
    let none = engine_scan_at(&d.join("none.sock"), 7);
    assert_eq!(none.status, "missing");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn the_cache_file_round_trips() {
    let home = crate::test_support::TempHome::new();
    let path = home.path().join(".k2").join(CACHE_FILE);
    assert_eq!(load_cache_file(&path), Cache::default(), "missing = empty");
    let mut c = Cache::default();
    c.workspaces.insert("ws-1".into(), FolderScan { bytes: 9, parts: [("uploads".to_string(), 8)].into(), partial: true, scanned_at: 3 });
    c.apps_at = Some(4);
    c.databases = Some(DbScan { wal_bytes: 5, ..DbScan::default() });
    save_cache_file(&path, &c).unwrap();
    assert_eq!(load_cache_file(&path), c);
    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(load_cache_file(&path), Cache::default(), "unreadable = empty, never a crash");
}

#[test]
fn folders_walk_partial_and_new_first_then_app_workspaces_then_biggest() {
    let projects: Vec<(String, String)> =
        ["a", "b", "c", "d", "e"].iter().map(|w| (w.to_string(), format!("/w/{w}"))).collect();
    let mut prev = BTreeMap::new();
    let f = |bytes, partial| FolderScan { bytes, partial, ..FolderScan::default() };
    prev.insert("a".to_string(), f(10, false));
    prev.insert("b".to_string(), f(500, false));
    prev.insert("c".to_string(), f(1, true));
    prev.insert("e".to_string(), f(50, false));
    let with_apps: BTreeSet<String> = ["e".to_string()].into();
    let order: Vec<String> = folder_order(&projects, &with_apps, &prev).into_iter().map(|(w, _)| w).collect();
    assert_eq!(order, vec!["c", "d", "e", "b", "a"]);
}

/// The rows: every App, `--cmd` service, database and workspace with its
/// owner; the workspace row carries the folder and workspace id (the view
/// strips both for agents); unknown is `None`, never 0; `byOwner` adds an
/// agent's rows up.
#[test]
fn the_snapshot_names_every_rows_owner() {
    let home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let ws_path = home.path().join("AI").join("nsi");
    std::fs::create_dir_all(&ws_path).unwrap();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (id, p) in [("ws-nsi", ws_path.to_string_lossy().to_string()), ("ws-plans", home.path().join("AI/plans").to_string_lossy().to_string())] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?3)", params![id, p, id.trim_start_matches("ws-")]).unwrap();
        }
        store::insert_app(
            &conn,
            &store::NewApp {
                app_id: "jobs",
                service_id: "svc-jobs",
                seed_id: None,
                owner_workspace: "ws-nsi",
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
        store::insert_app(
            &conn,
            &store::NewApp {
                app_id: "walled-one",
                service_id: "svc-w",
                seed_id: None,
                owner_workspace: "",
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES ('svc-x', 'ws-plans', 'plans-api', 'node s.js', '/', 4100, 'local', 'running')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sql_databases (id, project_id, name, status, created_at) VALUES ('d1', 'ws-nsi', 'jobs_db', 'active', 1), ('d2', 'ws-plans', 'gone_db', 'dropped', 1)",
            [],
        )
        .unwrap();
    }
    let mut c = Cache::default();
    c.apps.insert("jobs".into(), FolderScan { bytes: 6000, parts: [("data".to_string(), 4800), ("logs".into(), 50), ("node_modules".into(), 1000)].into(), partial: false, scanned_at: 11 });
    c.workspaces.insert(
        "ws-nsi".into(),
        FolderScan { bytes: 38_400, parts: [("uploads".to_string(), 29_000), ("node_modules".into(), 2_100), (".next".into(), 1_400), ("src".into(), 10)].into(), partial: false, scanned_at: 12 },
    );
    c.workspaces_at = Some(12);
    c.databases = Some(DbScan {
        databases: [("jobs_db".to_string(), 58_200u64), ("postgres".into(), 7)].into(),
        wal_bytes: 1_100,
        data_directory: None,
        at: 13,
        error: None,
        clusters: vec![],
    });
    c.engine = Some(EngineScan { status: "old".into(), at: 14, ..EngineScan::default() });
    let s = build_snapshot(&c, 20);
    let row = |kind: &str, id: &str| s.consumers.iter().find(|r| r.kind == kind && r.id == id).unwrap_or_else(|| panic!("{kind} {id}: {:#?}", s.consumers)).clone();
    let jobs = row("app", "jobs");
    assert_eq!(jobs.path.as_deref(), Some("process"));
    assert_eq!(jobs.owner.kind, "agent");
    assert_eq!(jobs.owner.handle.as_deref(), Some("nsi"));
    assert_eq!(jobs.owner.workspace_id.as_deref(), Some("ws-nsi"));
    assert_eq!(jobs.disk_bytes, Some(6000));
    assert_eq!(jobs.parts["data"], 4800);
    assert_eq!(jobs.parts["other"], 1000, "a folder not named data/cache/logs/home is other");
    assert_eq!(jobs.mem_bytes, None, "not running: unknown, never 0");
    let walled = row("app", "walled-one");
    assert_eq!(walled.path.as_deref(), Some("walled"));
    assert_eq!(walled.disk_bytes, None, "an old engine: walled disk unknown");
    assert_eq!(walled.owner.kind, "server", "parked: the server's");
    let svc = row("service", "plans-api");
    assert_eq!(svc.walled, Some(false));
    assert_eq!(svc.state.as_deref(), Some("stopped"));
    assert_eq!(svc.owner.handle.as_deref(), Some("plans"));
    assert_eq!(svc.disk_bytes, None);
    assert!(s.consumers.iter().all(|r| r.id != "svc-jobs"), "an App's own service is not a --cmd row");
    let db = row("database", "jobs_db");
    assert_eq!(db.disk_bytes, Some(58_200));
    assert_eq!(db.owner.handle.as_deref(), Some("nsi"));
    assert!(s.consumers.iter().all(|r| r.id != "gone_db"), "dropped databases are gone");
    let pg = row("shared", "postgres");
    assert_eq!(pg.disk_bytes, Some(1_107), "WAL + databases no agent owns");
    let ws = row("workspace", "ws-nsi");
    assert_eq!(ws.disk_bytes, Some(38_400));
    assert_eq!(ws.parts.len(), 3, "the three biggest parts");
    assert!(!ws.parts.contains_key("src"));
    assert_eq!(ws.folder.as_deref(), Some(ws_path.to_string_lossy().as_ref()), "people get the folder");
    let agent_view = ws.for_agent();
    assert!(agent_view.folder.is_none() && agent_view.owner.workspace_id.is_none(), "agents never");
    assert_eq!(agent_view.id, "nsi", "an agent sees the handle, never the workspace id");
    let plans_ws = row("workspace", "ws-plans");
    assert_eq!(plans_ws.disk_bytes, None, "not scanned yet: unknown");
    assert_eq!(s.scan.workspaces_at, Some(12));
    assert_eq!(s.scan.databases_at, Some(13));
    assert_eq!(s.scan.engine.as_deref(), Some("old"));
    assert_eq!(s.scan.disk_at, Some(20));
    assert!(s.k2_will_clear.is_none(), "mode off in tests: no sweep, nothing to clear");
    let by = s.by_owner();
    assert_eq!(by[0].owner.handle.as_deref(), Some("nsi"));
    assert_eq!(by[0].disk_bytes, 6000 + 58_200 + 38_400);
    // Smoke r6: K2's internal audit sentinels are not agent folders.
    let ids: Vec<&str> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE id IN ('_orphan', '_broadcast')", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2, "the scoped DB seeds both sentinels, so the filter is exercised");
        s.consumers.iter().filter(|r| r.kind == "workspace").map(|r| r.id.as_str()).collect()
    };
    assert_eq!(ids, vec!["ws-nsi", "ws-plans"], "no _orphan / _broadcast rows");
    let tracked: Vec<String> = tracked_paths().into_iter().map(|(_, p)| p.to_string_lossy().to_string()).collect();
    assert!(tracked.iter().all(|p| p != "_orphan" && p != "_broadcast"), "{tracked:?}");
}
