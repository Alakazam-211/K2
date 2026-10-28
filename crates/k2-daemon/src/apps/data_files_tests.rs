//! `k2 app data ls|get|rm|mv` (0.46.4) through `/cli/app/*`, in process:
//! who may read and tidy an App's data folder (as `data put`), what is
//! refused (another workspace's agent, a Member, an App pass, a bad
//! token), path containment (`..`, absolute paths, symlinks), the caps, the
//! held `manage` request for rm/mv without switch 1, the audit lines, and
//! that the disk guard never stops them. The unwalled process App reads
//! the folder directly; a walled App that isn't running says so.

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::connect_users::Role;
use k2_core::deploy::store;
use k2_core::disk_guard::{self as dg, fake, Mode, RawStat, GIB};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::access::Level;
use super::data_files::{CHUNK, MAX_GET};
use super::routes::Who;

struct Fx {
    disk: fake::FakeGuard,
    ws_a: String,
    ws_a_path: PathBuf,
    ws_b: String,
    data: PathBuf,
    #[cfg_attr(not(unix), allow(dead_code))]
    outside: PathBuf,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
}

impl Drop for Fx {
    fn drop(&mut self) {
        dg::clear_snapshot();
        dg::reset_scan_control();
    }
}

const TOTAL: u64 = 1000 * GIB;

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    dg::clear_snapshot();
    dg::reset_scan_control();
    let root = dunce::canonicalize(&home.path()).unwrap();
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    let ws_a_path = root.join("ws-a");
    std::fs::create_dir_all(&ws_a_path).unwrap();
    std::fs::create_dir_all(root.join("ws-b")).unwrap();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (ws, p) in [(&ws_a, ws_a_path.clone()), (&ws_b, root.join("ws-b"))] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, p.to_string_lossy()]).unwrap();
        }
        for (app, ws, iso) in [("plain", &ws_a, "process"), ("jobs", &ws_a, "container"), ("other", &ws_b, "process")] {
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
    }
    // What the App's pages wrote.
    let data = k2_core::deploy::app_dir("plain").join("data");
    std::fs::create_dir_all(data.join("var/files/sub")).unwrap();
    std::fs::write(data.join("var/answers.json"), br#"{"pin":"ok","q1":"yes"}"#).unwrap();
    std::fs::write(data.join("var/files/upload.pdf"), vec![9u8; 100]).unwrap();
    std::fs::write(data.join("var/files/sub/note.txt"), "n").unwrap();
    let outside = root.join("ws-b/secret.txt");
    std::fs::write(&outside, "another workspace's file").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, data.join("var/leak")).unwrap();
        std::os::unix::fs::symlink(root.join("ws-b"), data.join("up")).unwrap();
    }
    // 50% used: the guard is on and quiet until a test fills the disk.
    let disk = fake::install(&root, "9:9", "/", RawStat::sized(TOTAL, 500 * GIB), Some(Mode::On));
    Fx { disk, ws_a, ws_a_path, ws_b, data, outside, _db: db, _home: home }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

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

fn parse(path: &str, r: crate::cli_response::CliResponse) -> (u16, J) {
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn post(path: &str, who: &Who, body: J) -> (u16, J) {
    parse(path, super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new()))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    parse(path, super::routes::handle(path, who.clone(), b"", &p))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn b64(v: &J) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap_or_else(|| panic!("no data: {v}"))).unwrap()
}

fn audit_events(app: &str) -> Vec<(String, String, J)> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut st = conn.prepare("SELECT event, actor, payload_json FROM deploy_audit WHERE subject_id = ?1 ORDER BY seq").unwrap();
    st.query_map(params![app], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .unwrap()
        .map(|r| {
            let (e, a, p) = r.unwrap();
            (e, a, serde_json::from_str(&p).unwrap())
        })
        .filter(|(e, _, _)| e.starts_with("data_"))
        .collect()
}

/// The App's own agent (switch 1) reads its answers back, lists, renames
/// and removes; each call leaves an audit line.
#[test]
fn the_apps_agent_reads_lists_moves_and_removes_its_data() {
    let f = fx("own");
    set_level(&f.ws_a, Level::Build);
    let a = agent(&f.ws_a);

    let (st, v) = get("/cli/app/data/ls", &a, &[("app", "plain")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["kind"], "dir");
    let names: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"var"), "{v}");
    assert_eq!(v["truncated"], false);

    let (st, v) = get("/cli/app/data/ls", &a, &[("app", "plain"), ("path", "/data/var/files")]);
    assert_eq!(st, 200, "{v}");
    let e: Vec<(String, String, J)> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap().to_string(), e["type"].as_str().unwrap().to_string(), e["size"].clone()))
        .collect();
    assert_eq!(e, vec![("sub".into(), "dir".into(), J::Null), ("upload.pdf".into(), "file".into(), json!(100))]);

    let (st, v) = get("/cli/app/data/get", &a, &[("app", "plain"), ("path", "var/answers.json")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(b64(&v), br#"{"pin":"ok","q1":"yes"}"#);
    assert_eq!((v["size"].as_u64(), v["offset"].as_u64(), v["eof"].as_bool()), (Some(23), Some(0), Some(true)), "{v}");
    // The App's own spelling of the path works too.
    let (st, v) = get("/cli/app/data/get", &a, &[("app", "plain"), ("path", "/data/var/answers.json")]);
    assert_eq!((st, b64(&v)), (200, br#"{"pin":"ok","q1":"yes"}"#.to_vec()));

    let (st, v) = post("/cli/app/data/mv", &a, json!({"app": "plain", "from": "var/answers.json", "to": "done/2026-10/answers.json"}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["replaced"], false);
    assert!(f.data.join("done/2026-10/answers.json").is_file());
    assert!(!f.data.join("var/answers.json").exists());

    let (st, v) = post("/cli/app/data/rm", &a, json!({"app": "plain", "path": "done/2026-10/answers.json"}));
    assert_eq!((st, v["removed"].as_str()), (200, Some("file")), "{v}");
    assert!(!f.data.join("done/2026-10/answers.json").exists());

    let ev = audit_events("plain");
    let names: Vec<&str> = ev.iter().map(|(e, _, _)| e.as_str()).collect();
    assert_eq!(names, vec!["data_ls", "data_ls", "data_get", "data_get", "data_mv", "data_rm"]);
    let who = super::routes::actor(&a);
    assert!(who.starts_with("agent:"), "{who}");
    assert!(ev.iter().all(|(_, by, _)| *by == who), "{ev:?}");
    assert_eq!(ev[2].2["bytes"], 23, "{:?}", ev[2]);
    assert!(ev[4].2["to"].as_str().unwrap().ends_with("done/2026-10/answers.json"), "{:?}", ev[4]);
}

/// Who may: the owner, an Admin login, a shell tab, the App's agent with
/// switch 1. Who may not: another workspace's agent (even with switch 2),
/// a Member, an App pass / API key, no token. Without switch 1 the App's
/// agent can't read (refused) and its rm/mv wait for the owner (held,
/// nothing changed).
#[test]
fn who_may_read_and_tidy_an_apps_data() {
    let f = fx("who");
    let shell = Who::Shell { project_id: f.ws_a.clone(), session_id: "shell-1".into() };
    for who in [Who::Owner, Who::Login { username: "ada".into(), role: Role::Admin }, shell] {
        let (st, v) = get("/cli/app/data/get", &who, &[("app", "plain"), ("path", "var/files/sub/note.txt")]);
        assert_eq!((st, b64(&v)), (200, b"n".to_vec()), "{who:?}: {v}");
        let (st, v) = get("/cli/app/data/ls", &who, &[("app", "plain"), ("path", "var")]);
        assert_eq!(st, 200, "{who:?}: {v}");
    }

    // Another workspace's agent: the App isn't there for it, every verb.
    set_level(&f.ws_b, Level::Full);
    let b = agent(&f.ws_b);
    for (st, v) in [
        get("/cli/app/data/ls", &b, &[("app", "plain")]),
        get("/cli/app/data/get", &b, &[("app", "plain"), ("path", "var/answers.json")]),
        post("/cli/app/data/rm", &b, json!({"app": "plain", "path": "var/answers.json"})),
        post("/cli/app/data/mv", &b, json!({"app": "plain", "from": "var/answers.json", "to": "x"})),
    ] {
        assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    }

    // A Member, an App pass (wrong credential), no token.
    for (who, want) in [
        (Who::Login { username: "mo".into(), role: Role::Member }, "member_read_only"),
        (Who::Wrong, "wrong_credential"),
        (Who::Invalid, "forbidden"),
    ] {
        for (st, v) in [
            get("/cli/app/data/ls", &who, &[("app", "plain")]),
            get("/cli/app/data/get", &who, &[("app", "plain"), ("path", "var/answers.json")]),
            post("/cli/app/data/rm", &who, json!({"app": "plain", "path": "var/answers.json"})),
            post("/cli/app/data/mv", &who, json!({"app": "plain", "from": "var/answers.json", "to": "x"})),
        ] {
            assert_eq!(st, 403, "{who:?}: {v}");
            assert_eq!(code(&v), want, "{who:?}: {v}");
        }
    }

    // The App's own agent without switch 1.
    set_level(&f.ws_a, Level::Off);
    let a = agent(&f.ws_a);
    for (st, v) in [
        get("/cli/app/data/ls", &a, &[("app", "plain")]),
        get("/cli/app/data/get", &a, &[("app", "plain"), ("path", "var/answers.json")]),
    ] {
        assert_eq!((st, code(&v)), (403, "apps_manage_off"), "{v}");
    }
    for (path, body) in [
        ("/cli/app/data/rm", json!({"app": "plain", "path": "var/answers.json"})),
        ("/cli/app/data/mv", json!({"app": "plain", "from": "var/answers.json", "to": "moved.json"})),
    ] {
        let (st, v) = post(path, &a, body);
        assert_eq!((st, v["status"].as_str(), v["kind"].as_str()), (202, Some("held"), Some("manage")), "{path}: {v}");
    }
    assert!(f.data.join("var/answers.json").is_file(), "nothing removed or moved while held");
    assert!(!f.data.join("moved.json").exists());
    assert!(audit_events("plain").iter().all(|(e, _, _)| e != "data_rm" && e != "data_mv"), "no change audited for a held request");
}

/// `..`, other absolute paths and symlinks never reach outside the data
/// folder, on any verb.
#[cfg(unix)]
#[test]
fn paths_never_leave_the_data_folder() {
    let f = fx("paths");
    let o = Who::Owner;
    for p in ["../../ws-b/secret.txt", "/etc/passwd", "var/../../x", "/data/../etc/passwd", "a\nb"] {
        let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", p)]);
        assert_eq!((st, code(&v)), (400, "bad_request"), "{p:?}: {v}");
        let (st, v) = get("/cli/app/data/ls", &o, &[("app", "plain"), ("path", p)]);
        assert_eq!((st, code(&v)), (400, "bad_request"), "{p:?}: {v}");
        let (st, v) = post("/cli/app/data/rm", &o, json!({"app": "plain", "path": p, "recursive": true}));
        assert_eq!((st, code(&v)), (400, "bad_request"), "{p:?}: {v}");
        let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "var/answers.json", "to": p}));
        assert_eq!((st, code(&v)), (400, "bad_request"), "{p:?}: {v}");
    }
    // The data folder itself is never removed or moved.
    for p in ["", "/data", "."] {
        let (st, v) = post("/cli/app/data/rm", &o, json!({"app": "plain", "path": p, "recursive": true}));
        assert_eq!((st, code(&v)), (400, "bad_request"), "{p:?}: {v}");
    }
    assert!(f.data.join("var/answers.json").is_file());

    // A symlink, at the end or on the way.
    for p in ["var/leak", "up/secret.txt"] {
        let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", p)]);
        assert_eq!((st, code(&v)), (403, "data_symlink"), "{p}: {v}");
        assert!(!v.to_string().contains("another workspace"), "{v}");
    }
    let (st, v) = get("/cli/app/data/ls", &o, &[("app", "plain"), ("path", "up")]);
    assert_eq!((st, v["kind"].as_str()), (200, Some("link")), "a link lists as itself: {v}");
    let (st, v) = get("/cli/app/data/ls", &o, &[("app", "plain"), ("path", "up/")]);
    assert_eq!((st, v["kind"].as_str()), (200, Some("link")), "{v}");
    let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "var/answers.json", "to": "up/stolen.json"}));
    assert_eq!((st, code(&v)), (403, "data_symlink"), "{v}");
    assert!(!f.outside.parent().unwrap().join("stolen.json").exists());
    let (st, v) = post("/cli/app/data/rm", &o, json!({"app": "plain", "path": "up/secret.txt"}));
    assert_eq!((st, code(&v)), (403, "data_symlink"), "{v}");
    // rm on a link removes the link only.
    let (st, v) = post("/cli/app/data/rm", &o, json!({"app": "plain", "path": "var/leak"}));
    assert_eq!((st, v["removed"].as_str()), (200, Some("link")), "{v}");
    assert_eq!(std::fs::read_to_string(&f.outside).unwrap(), "another workspace's file");
    let _ = &f.ws_b;
}

/// rm refuses a folder without --recursive; mv never overwrites without
/// --force and never lands inside a folder; missing paths are 404.
#[test]
fn rm_and_mv_refuse_the_dangerous_cases() {
    let f = fx("rmmv");
    let o = Who::Owner;
    let (st, v) = post("/cli/app/data/rm", &o, json!({"app": "plain", "path": "var/files"}));
    assert_eq!((st, code(&v)), (409, "data_is_folder"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("--recursive"), "{v}");
    assert!(f.data.join("var/files/upload.pdf").is_file());
    let (st, v) = post("/cli/app/data/rm", &o, json!({"app": "plain", "path": "var/files", "recursive": true}));
    assert_eq!((st, v["removed"].as_str()), (200, Some("dir")), "{v}");
    assert!(!f.data.join("var/files").exists());

    std::fs::write(f.data.join("a.txt"), "a").unwrap();
    std::fs::write(f.data.join("b.txt"), "b").unwrap();
    let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "a.txt", "to": "b.txt"}));
    assert_eq!((st, code(&v)), (409, "data_exists"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("--force"), "{v}");
    assert_eq!(std::fs::read_to_string(f.data.join("b.txt")).unwrap(), "b");
    let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "a.txt", "to": "b.txt", "force": true}));
    assert_eq!((st, v["replaced"].as_bool()), (200, Some(true)), "{v}");
    assert_eq!(std::fs::read_to_string(f.data.join("b.txt")).unwrap(), "a");
    let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "b.txt", "to": "var", "force": true}));
    assert_eq!((st, code(&v)), (409, "data_exists"), "never into a folder: {v}");
    let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "var", "to": "var/inner"}));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "b.txt", "to": "b.txt"}));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    let (st, v) = post("/cli/app/data/mv", &o, json!({"app": "plain", "from": "nope", "to": "z"}));
    assert_eq!((st, code(&v)), (404, "data_not_found"), "{v}");
    let (st, v) = post("/cli/app/data/rm", &o, json!({"app": "plain", "path": "nope"}));
    assert_eq!((st, code(&v)), (404, "data_not_found"), "{v}");
    let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", "var")]);
    assert_eq!((st, code(&v)), (409, "data_is_folder"), "{v}");
    let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain")]);
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
}

/// get moves a file in CHUNK pieces and refuses one past MAX_GET with
/// 413; a bad offset is a 400.
#[test]
fn get_chunks_and_caps() {
    let f = fx("caps");
    let o = Who::Owner;
    let body: Vec<u8> = (0..(CHUNK + 10)).map(|i| (i % 251) as u8).collect();
    std::fs::write(f.data.join("big.bin"), &body).unwrap();
    let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", "big.bin")]);
    assert_eq!(st, 200, "{}", v["error"]);
    assert_eq!((v["length"].as_u64(), v["eof"].as_bool(), v["chunk"].as_u64()), (Some(CHUNK), Some(false), Some(CHUNK)));
    let mut got = b64(&v);
    let off = CHUNK.to_string();
    let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", "big.bin"), ("offset", &off)]);
    assert_eq!(st, 200, "{}", v["error"]);
    assert_eq!((v["length"].as_u64(), v["eof"].as_bool()), (Some(10), Some(true)));
    got.extend(b64(&v));
    assert_eq!(got, body);
    for bad in ["7", "-1", "x"] {
        let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", "big.bin"), ("offset", bad)]);
        assert_eq!((st, code(&v)), (400, "bad_request"), "{bad}: {v}");
    }
    let past = (CHUNK * 4).to_string();
    let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", "big.bin"), ("offset", &past)]);
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    // Past the cap (a sparse file: no disk used).
    let huge = std::fs::File::create(f.data.join("huge.bin")).unwrap();
    huge.set_len(MAX_GET + 1).unwrap();
    let (st, v) = get("/cli/app/data/get", &o, &[("app", "plain"), ("path", "huge.bin")]);
    assert_eq!((st, code(&v)), (413, "data_too_large"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("256 MiB"), "{v}");
    assert!(v.get("data").is_none());
}

/// A walled App's data is reached only through its running container: a
/// stopped one says so (and nothing is read on the host).
#[test]
fn a_walled_app_must_be_running() {
    let f = fx("wall");
    set_level(&f.ws_a, Level::Build);
    let a = agent(&f.ws_a);
    for (st, v) in [
        get("/cli/app/data/ls", &a, &[("app", "jobs")]),
        get("/cli/app/data/get", &a, &[("app", "jobs"), ("path", "var/answers.json")]),
        post("/cli/app/data/rm", &a, json!({"app": "jobs", "path": "x"})),
        post("/cli/app/data/mv", &a, json!({"app": "jobs", "from": "x", "to": "y"})),
    ] {
        assert_eq!((st, code(&v)), (409, "app_not_running"), "{v}");
    }
}

/// The disk guard gates new disk work; reading, renaming and freeing an
/// App's data are none, so they go through past the refuse line (where
/// `data put` stops).
#[test]
fn the_disk_guard_never_stops_reads_or_freeing() {
    let f = fx("disk");
    // 96% used: past the refuse line.
    f.disk.set(RawStat::sized(TOTAL, 40 * GIB));
    dg::clear_snapshot();
    std::fs::write(f.ws_a_path.join("rows.csv"), "a,b\n").unwrap();
    let (st, v) = post("/cli/app/data/put", &Who::Owner, json!({"app": "plain", "from": f.ws_a_path.join("rows.csv")}));
    assert_eq!((st, code(&v)), (409, "disk_low"), "the guard is on: {v}");
    let (st, v) = get("/cli/app/data/ls", &Who::Owner, &[("app", "plain")]);
    assert_eq!(st, 200, "{v}");
    let (st, v) = get("/cli/app/data/get", &Who::Owner, &[("app", "plain"), ("path", "var/answers.json")]);
    assert_eq!(st, 200, "{v}");
    let (st, v) = post("/cli/app/data/mv", &Who::Owner, json!({"app": "plain", "from": "var/answers.json", "to": "kept.json"}));
    assert_eq!(st, 200, "{v}");
    let (st, v) = post("/cli/app/data/rm", &Who::Owner, json!({"app": "plain", "path": "var/files", "recursive": true}));
    assert_eq!(st, 200, "{v}");
    assert!(!f.data.join("var/files").exists());
}

/// rm and mv are POST-only (405 on GET at the door); ls and get are reads.
#[test]
fn rm_and_mv_are_post_only() {
    for p in ["/cli/app/data/rm", "/cli/app/data/mv"] {
        assert!(super::routes::is_route(p) && super::routes::is_post_route(p), "{p}");
    }
    for p in ["/cli/app/data/ls", "/cli/app/data/get"] {
        assert!(super::routes::is_route(p) && !super::routes::is_post_route(p), "{p}");
    }
}
