use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering as AtomOrd};
use std::sync::Arc as StdArc;

/// K2 Cores field report (2026-10-28): a service that exits at start
/// says why in `k2 publish ps` (its exit code and last log line); one
/// someone stopped, or that serves, says nothing; an exit code from an
/// older run isn't this start's.
#[test]
fn down_reason_names_the_exit_and_the_last_log_line() {
    crate::test_support::with_temp_home(|| {
        let row = |status_err: Option<&str>, code: Option<i64>, exited: Option<i64>| ps::PublishedService {
            id: "svc-mpc".into(),
            project_id: "ws-cores".into(),
            name: "mpc-progress".into(),
            cmd: "node server.js".into(),
            cwd: "/".into(),
            port: 8791,
            expose: "tunnel".into(),
            desired: "running".into(),
            pid: None,
            last_exit_code: code,
            last_started_at: Some(100),
            last_exited_at: exited,
            error: status_err.map(str::to_string),
            created_at: 1,
            updated_at: 1,
            kind: KIND_CMD.into(),
            skin_root: String::new(),
            frame_ancestors: String::new(),
            probe_timeout_ms: None,
            hostname_pending: false,
            dev_mem_mb: None,
            dev_cpus: None,
            dev_idle_stopped_at: None,
        };
        // No log yet: the exit alone.
        assert_eq!(down_reason(&row(None, Some(2), Some(101)), STATUS_EXITED).as_deref(), Some("its process exited with code 2"));
        let path = log_path("ws-cores", "mpc-progress");
        fs::create_dir_all(path.parent().expect("parent")).expect("log dir");
        fs::write(&path, "starting\nMPC_PROGRESS_PIN is not set\n\n").expect("log");
        let want = Some("its process exited with code 2; its last log line: MPC_PROGRESS_PIN is not set");
        assert_eq!(down_reason(&row(None, Some(2), Some(101)), STATUS_EXITED).as_deref(), want);
        // A start that failed (probe) and was rolled back: stopped + error.
        let failed = row(Some("probe failed: the process exited before 127.0.0.1:8791 accepted"), Some(2), Some(101));
        assert_eq!(down_reason(&failed, STATUS_STOPPED).as_deref(), want);
        // The exit code is an older run's: the error, not "exit 2".
        let stale = row(Some("pro_required"), Some(2), Some(50));
        assert_eq!(down_reason(&stale, STATUS_STOPPED).as_deref(), Some("pro_required; its last log line: MPC_PROGRESS_PIN is not set"));
        assert_eq!(
            down_reason(&row(None, None, None), STATUS_UNHEALTHY).as_deref(),
            Some("it runs but doesn't answer on port 8791; its last log line: MPC_PROGRESS_PIN is not set")
        );
        // Serving, starting, or stopped by someone: no reason.
        for st in [STATUS_RUNNING, STATUS_STARTING, STATUS_STOPPED] {
            assert_eq!(down_reason(&row(None, Some(143), Some(101)), st), None, "{st}");
        }
        // A long last line is cut.
        fs::write(&path, format!("{}\n", "x".repeat(1000))).expect("log");
        let r = down_reason(&row(None, Some(1), Some(101)), STATUS_EXITED).expect("reason");
        assert_eq!(r.chars().count(), "its process exited with code 1; its last log line: ".len() + REASON_MAX_CHARS + 1, "{r}");
    });
}

/// S24-8: the boot check recognises a helper bound to its App and one
/// from before per-App logins (no `--app`, or another App's id).
#[test]
fn argv_binds_app_reads_both_spellings() {
    let id = "0f5e7c1a-3b2d-4e6f-8a9b-1c2d3e4f5a6b";
    let argv = |extra: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = ["k2-daemon", "--skin-gateway", "--listen", "127.0.0.1:9"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    };
    assert!(argv_binds_app(&argv(&["--app", id]), id));
    assert!(argv_binds_app(&argv(&[&format!("--app={id}")]), id));
    assert!(!argv_binds_app(&argv(&[]), id), "pre-0.45.5 helper");
    assert!(!argv_binds_app(&argv(&["--app", "other"]), id));
    assert!(!argv_binds_app(&argv(&["--app"]), id), "dangling flag");
    let real = crate::skin_gateway::helper_argv(
        9,
        8,
        None,
        &k2_core::frame_policy::FramePolicy::app_default(),
        id,
    );
    assert!(argv_binds_app(&real, id), "{real:?}");
}

/// S24-8 on this platform: the argv reader sees a live process's args.
#[cfg(unix)]
#[test]
fn process_argv_reads_a_live_child() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    // `spawn` returns as soon as the child's exec has swapped its memory
    // (the vfork release), before the kernel has set the new argv
    // bounds: `/proc/<pid>/cmdline` reads empty for a few ms (seen on
    // every Linux soak run, 2026-10-14). Before exec it can also read as
    // the forked copy of this test binary's own argv (seen in the 0.46.6
    // gates), so poll until it is neither. Production only reads helpers
    // reattached from an earlier daemon life, long past that window.
    let own: Vec<String> = std::env::args().collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last = None;
    let argv = loop {
        if let Some(argv) = process_argv(child.id() as i32) {
            if argv != own {
                break Some(argv);
            }
            last = Some(argv);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let _ = child.kill();
    let _ = child.wait();
    let argv = argv.unwrap_or_else(|| panic!("the child's own argv within 5 s; last read: {last:?}"));
    assert!(argv.iter().any(|a| a == "30"), "{argv:?}");
}

fn unique(prefix: &str) -> String {
    format!(
        "{}-{}-{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn make_project(path: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, "pub", path],
    )
    .expect("insert project");
    id
}

fn cleanup(project_id: &str, name: &str) {
    let _ = stop(project_id, name);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = ps::delete(&conn, project_id, name);
    let _ = k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, name);
}

fn listener_cmd(port: u16) -> String {
    format!(
        "{py} -c \"import socket,itertools; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',{port})); s.listen(8); [s.accept()[0].close() for _ in itertools.count()]\"",
        py = k2_core::test_env::PYTHON
    )
}

/// A port for a published service the test's child binds later. Not
/// `bind(:0)`-then-drop: under parallel load the OS re-issued that
/// ephemeral port to another test before the child bound it ("bind
/// clash: … already accepting connections", 0.46.3). A per-process
/// unique port outside the ephemeral range instead.
fn free_port() -> u16 {
    k2_core::test_env::loopback_test_port()
}

fn python_ok() -> bool {
    Command::new(k2_core::test_env::PYTHON)
        .arg("-c")
        .arg("print(1)")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct MockCp {
    hits: StdArc<AtomicUsize>,
    _join: thread::JoinHandle<()>,
    /// Control-plane base override under the ONE shared env lock;
    /// restored on drop.
    _cp_base: k2_core::test_env::EnvVar,
}

fn start_mock_cp(status: u16, body: &'static str, methods_ok: &'static str) -> MockCp {
    let hits = StdArc::new(AtomicUsize::new(0));
    let hits2 = StdArc::clone(&hits);
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("mock cp");
    let port = listener.local_addr().unwrap().port();
    let join = thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut sock, _)) => {
                    hits2.fetch_add(1, AtomOrd::SeqCst);
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf);
                    let req = String::from_utf8_lossy(&buf);
                    let allow = methods_ok == "*" || req.starts_with(methods_ok);
                    let (code, payload) = if allow {
                        (status, body)
                    } else {
                        (500, "{\"error\":\"unexpected\"}")
                    };
                    let resp = format!(
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = sock.write_all(resp.as_bytes());
                }
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }
    });
    let cp_base = k2_core::test_env::EnvVar::set(
        subdomains::CONTROL_PLANE_BASE_ENV,
        format!("http://127.0.0.1:{port}"),
    );
    MockCp {
        hits,
        _join: join,
        _cp_base: cp_base,
    }
}

#[test]
fn spawn_probe_and_stop_kills_tree() {
    if !python_ok() {
        panic!("python3 is required for published-service runtime tests");
    }
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let dir = _home.path().join("ws");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("tree");
    let port = free_port();
    // Grandchild: python listener in the background of the shell.
    let cmd = format!("{} & wait", listener_cmd(port));
    let spec = RunSpec {
        project_id: project_id.clone(),
        name: name.clone(),
        cmd,
        cwd: dir,
        port,
        no_tunnel: true,
        replace_spec: false,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: false,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    };
    let json = run(spec).expect("run");
    assert_eq!(
        json.status, STATUS_RUNNING,
        "probe must succeed; pid={:?} port_up={}",
        json.pid,
        port_accepts(port)
    );
    assert!(json.pid.is_some());
    assert!(json.url.is_none(), "no-tunnel has no public URL");
    let pid = json.pid.unwrap() as i32;
    assert!(pid_alive(pid), "shell still alive");
    assert!(
        crate::v2_session_map::snapshot()
            .iter()
            .all(|(_, s)| s.child_pid() != Some(pid)),
        "published pid must not be in v2_session_map"
    );
    stop(&project_id, &name).expect("stop");
    thread::sleep(Duration::from_millis(300));
    assert!(!pid_alive(pid), "stop must kill the process group");
    assert!(!port_accepts(port), "listener grandchild must die too");
    let after = load_row(&project_id, &name).unwrap();
    assert_eq!(after.desired, DESIRED_STOPPED);
    cleanup(&project_id, &name);
}

#[test]
fn no_tunnel_never_hits_control_plane() {
    if !python_ok() {
        panic!("python3 is required for published-service runtime tests");
    }
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let mock = start_mock_cp(200, r#"{"subdomains":[]}"#, "*");
    let dir = _home.path().join("ws2");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("local");
    let port = free_port();
    let json = run(RunSpec {
        project_id: project_id.clone(),
        name: name.clone(),
        cmd: listener_cmd(port),
        cwd: dir,
        port,
        no_tunnel: true,
        replace_spec: false,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: false,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    })
    .expect("run --no-tunnel");
    assert_eq!(json.expose, EXPOSE_LOCAL);
    assert_eq!(json.status, STATUS_RUNNING, "no-tunnel must actually listen");
    assert_eq!(
        mock.hits.load(AtomOrd::SeqCst),
        0,
        "--no-tunnel must never dial connect.k2.dev (GET or POST)"
    );
    cleanup(&project_id, &name);
}

/// AR6: a publish run (plain or `--skin`) on a label another
/// workspace holds is refused with a structured 409 before any row,
/// process or control-plane call; the owner keeps the label.
#[test]
fn run_refuses_label_owned_by_other_workspace_before_any_write() {
    let _home = crate::test_support::TempHome::new();
    let cfg = serde_json::json!({ "token": "tok_test", "subdomain": "rosson", "e2e": true });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    let mock = start_mock_cp(
        200,
        r#"{"subdomains":[{"label":"rosson","primary":true,"connected":true,"tier":"pro"}]}"#,
        "*",
    );
    let dir_a = _home.path().join("ws-own-a");
    let dir_b = _home.path().join("ws-own-b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();
    let a = make_project(&dir_a.to_string_lossy());
    let b = make_project(&dir_b.to_string_lossy());
    let name = unique("owned");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        SubdomainWorkspace::claim(&conn, &name, &a).expect("A claims");
    }
    let marker = dir_b.join("spawned");
    for skin in [false, true] {
        let err = run(RunSpec {
            project_id: b.clone(),
            name: name.clone(),
            cmd: if skin { String::new() } else { format!("echo hi > {}", marker.display()) },
            cwd: dir_b.clone(),
            port: 9,
            no_tunnel: false,
            replace_spec: false,
            skin,
            skin_root: String::new(),
            cwd_explicit: false,
            transfer: false,
            probe_timeout: ProbeTimeout::Keep,
            dev: None,
        })
        .expect_err("B must not take A's label");
        assert_eq!(err.status, "409 Conflict", "skin={skin} msg={}", err.message);
        let body = err.body.as_ref().expect("structured body");
        assert_eq!(body["code"], crate::label_owner::CODE, "skin={skin}");
        // N6: the holder's agent, never its folder path.
        assert!(body["owner"]["agent"].is_string(), "skin={skin} {body}");
        assert!(body["owner"].get("path").is_none(), "skin={skin} {body}");
        assert!(!marker.exists(), "must not spawn (skin={skin})");
        assert!(load_row(&b, &name).is_none(), "no service row for B (skin={skin})");
    }
    // An agent/member asking for a transfer is refused too.
    let err = run(RunSpec {
        project_id: b.clone(),
        name: name.clone(),
        cmd: "true".into(),
        cwd: dir_b.clone(),
        port: 9,
        no_tunnel: false,
        replace_spec: false,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: true,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    })
    .expect_err("non-owner transfer must fail");
    assert_eq!(err.status, "403 Forbidden", "msg={}", err.message);
    assert_eq!(mock.hits.load(AtomOrd::SeqCst), 0, "no control-plane call on refusal");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(
            SubdomainWorkspace::owner_of(&conn, &name).unwrap().as_deref(),
            Some(a.as_str()),
            "A keeps its label"
        );
        let _ = SubdomainWorkspace::unclaim(&conn, &name);
    }
}

/// 0.46.3: `publish run --port <the daemon's own port>` is refused for
/// an agent and for the owner, tunnel or not, before any row, process
/// or control-plane call; hostname attach refuses it too.
#[test]
fn run_refuses_the_daemon_port_for_everyone() {
    let _home = crate::test_support::TempHome::new();
    let cfg = serde_json::json!({ "token": "tok_test", "subdomain": "rosson", "e2e": true });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    let mock = start_mock_cp(
        200,
        r#"{"subdomains":[{"label":"rosson","primary":true,"connected":true,"tier":"pro"}]}"#,
        "*",
    );
    let hold = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let daemon_port = hold.local_addr().unwrap().port();
    let _reg = crate::splice_guard::register_daemon_port(daemon_port);
    let dir = _home.path().join("ws-self");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("selfie");
    let marker = dir.join("spawned");
    let callers = [
        crate::label_owner::Caller::Agent,
        crate::label_owner::Caller::Manager { actor: "owner".into() },
    ];
    for caller in callers {
        for no_tunnel in [false, true] {
            let err = crate::label_owner::with_caller(caller.clone(), || {
                run(RunSpec {
                    project_id: project_id.clone(),
                    name: name.clone(),
                    cmd: format!("echo hi > {}", marker.display()),
                    cwd: dir.clone(),
                    port: daemon_port,
                    no_tunnel,
                    replace_spec: false,
                    skin: false,
                    skin_root: String::new(),
                    cwd_explicit: false,
                    transfer: false,
                    probe_timeout: ProbeTimeout::Keep,
                    dev: None,
                })
            })
            .expect_err("the daemon's own port must be refused");
            assert_eq!(err.status, "400 Bad Request", "{caller:?} no_tunnel={no_tunnel}: {}", err.message);
            let body = err.body.as_ref().expect("structured body");
            assert_eq!(body["code"], "target_is_daemon", "{caller:?}: {body}");
            assert!(!marker.exists(), "must not spawn ({caller:?})");
            assert!(load_row(&project_id, &name).is_none(), "no row ({caller:?})");
        }
        let err = crate::label_owner::with_caller(caller.clone(), || {
            attach_hostname(&project_id, &name, daemon_port, "tok_test", "rosson", false)
        })
        .expect_err("attach refuses the daemon port");
        assert!(err.contains("target_is_daemon"), "{caller:?}: {err}");
    }
    assert_eq!(mock.hits.load(AtomOrd::SeqCst), 0, "no control-plane call on refusal");
}

/// Security review D: when the control plane says a label exists
/// (`label_taken`), an agent repoints it only when its own workspace
/// holds it, whatever the (here empty) cached hostname map says; the
/// owner and the agent's own label go on to the repoint.
#[test]
fn an_agent_never_repoints_an_existing_label_its_workspace_doesnt_hold() {
    let _home = crate::test_support::TempHome::new();
    let cfg = serde_json::json!({ "token": "tok_test", "subdomain": "rosson", "e2e": true });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    let mock = start_mock_cp(409, r#"{"error":"label_taken"}"#, "*");
    let dir = _home.path().join("ws-taken");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let hold = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = hold.local_addr().unwrap().port();
    // The cached map has never heard of it (stale): only the control
    // plane's answer says it exists.
    let handmade = unique("handmade");
    assert!(!subdomains::current().targets.contains_key(&handmade), "the cached map knows nothing of it");
    let err = crate::label_owner::with_caller(crate::label_owner::Caller::Agent, || {
        attach_hostname(&project_id, &handmade, port, "tok_test", "rosson", false)
    })
    .expect_err("an agent never repoints a label nobody holds");
    assert!(err.starts_with(AGENT_LABEL_EXISTS), "{err}");
    assert_eq!(mock.hits.load(AtomOrd::SeqCst), 1, "the create only, never the repoint");
    // The owner goes on to the repoint (the mock refuses that too).
    let _ = crate::label_owner::with_caller(crate::label_owner::Caller::Manager { actor: "owner".into() }, || {
        attach_hostname(&project_id, &handmade, port, "tok_test", "rosson", false)
    });
    assert_eq!(mock.hits.load(AtomOrd::SeqCst), 3, "the owner's create and repoint");
    // The agent's own workspace's label goes on to the repoint too.
    let mine = unique("mine");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        SubdomainWorkspace::claim(&conn, &mine, &project_id).expect("claim");
    }
    let _ = crate::label_owner::with_caller(crate::label_owner::Caller::Agent, || {
        attach_hostname(&project_id, &mine, port, "tok_test", "rosson", false)
    });
    assert_eq!(mock.hits.load(AtomOrd::SeqCst), 5, "its own label: create and repoint");
}

/// AR6: `rm` of a same-named service never drops another workspace's
/// label attribution nor deletes its control-plane hostname.
#[test]
fn rm_never_releases_another_workspaces_label() {
    let _home = crate::test_support::TempHome::new();
    let cfg = serde_json::json!({ "token": "tok_test", "subdomain": "rosson", "e2e": true });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    let mock = start_mock_cp(200, r#"{"removed":"x"}"#, "*");
    let dir_a = _home.path().join("ws-rm-a");
    let dir_b = _home.path().join("ws-rm-b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();
    let a = make_project(&dir_a.to_string_lossy());
    let b = make_project(&dir_b.to_string_lossy());
    let name = unique("rmown");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        SubdomainWorkspace::claim(&conn, &name, &a).expect("A claims");
        ps::insert(
            &conn,
            &b,
            &name,
            "true",
            &dir_b.to_string_lossy(),
            9,
            EXPOSE_TUNNEL,
            DESIRED_STOPPED,
            KIND_CMD,
            "",
        )
        .expect("B row");
    }
    rm(&b, &name, false).expect("rm B's own row");
    assert!(load_row(&b, &name).is_none(), "B's row is gone");
    assert_eq!(mock.hits.load(AtomOrd::SeqCst), 0, "A's hostname must not be deleted");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(
            SubdomainWorkspace::owner_of(&conn, &name).unwrap().as_deref(),
            Some(a.as_str()),
            "A keeps its label"
        );
        let _ = SubdomainWorkspace::unclaim(&conn, &name);
    }
}

#[test]
fn nested_without_pro_fails_before_spawn() {
    let _home = crate::test_support::TempHome::new();
    // Persist a token so the gate reaches the Pro check (and not
    // "no token") — still no spawn.
    let cfg = serde_json::json!({
        "token": "tok_test",
        "subdomain": "rosson",
        "e2e": true
    });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    let _mock = start_mock_cp(
        200,
        r#"{"subdomains":[{"label":"rosson","primary":true,"connected":true,"tier":"free"}]}"#,
        "GET ",
    );
    let dir = _home.path().join("ws3");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("nopro");
    let marker = dir.join("spawned");
    let cmd = format!("echo hi > {}", marker.display());
    let err = run(RunSpec {
        project_id: project_id.clone(),
        name: name.clone(),
        cmd,
        cwd: dir.clone(),
        port: 9,
        no_tunnel: false,
        replace_spec: false,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: false,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    })
    .expect_err("not Pro must fail");
    assert!(
        err.message.contains("pro_required") || err.message.contains("dashboard"),
        "got: {}",
        err.message
    );
    assert!(
        err.message.contains("--no-tunnel"),
        "hint --no-tunnel: {}",
        err.message
    );
    assert!(!marker.exists(), "must not spawn");
    assert!(
        load_row(&project_id, &name).is_none(),
        "no row on pre-spawn fail"
    );
    cleanup(&project_id, &name);
}

#[test]
fn hostname_fail_does_not_leave_desired_running() {
    if !python_ok() {
        panic!("python3 is required for published-service runtime tests");
    }
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let _claim_tries = k2_core::test_env::EnvVar::set("K2_PUBLISH_CLAIM_TRIES", "2");
    let cfg = serde_json::json!({
        "token": "tok_test",
        "subdomain": "rosson",
        "e2e": true
    });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    // GET = Pro; POST create fails.
    let hits = StdArc::new(AtomicUsize::new(0));
    let hits2 = StdArc::clone(&hits);
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("mock");
    let port_cp = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok((mut sock, _)) = listener.accept() {
                hits2.fetch_add(1, AtomOrd::SeqCst);
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let (code, body) = if req.starts_with("GET ") {
                    (
                        200,
                        r#"{"subdomains":[{"label":"rosson","primary":true,"connected":true,"tier":"pro"}]}"#,
                    )
                } else {
                    (403, r#"{"error":"pro_required"}"#)
                };
                let resp = format!(
                    "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            } else {
                thread::sleep(Duration::from_millis(20));
            }
        }
    });
    let cp_base = k2_core::test_env::EnvVar::set(
        subdomains::CONTROL_PLANE_BASE_ENV,
        format!("http://127.0.0.1:{port_cp}"),
    );

    let dir = _home.path().join("ws4");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("hostfail");
    let port = free_port();
    let err = run(RunSpec {
        project_id: project_id.clone(),
        name: name.clone(),
        cmd: listener_cmd(port),
        cwd: dir,
        port,
        no_tunnel: false,
        replace_spec: false,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: false,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    })
    .expect_err("hostname fail");
    drop(cp_base);
    assert!(
        !err.message.contains("probe failed"),
        "must get past probe to the hostname step: {}",
        err.message
    );
    assert!(!err.message.is_empty());
    assert!(!port_accepts(port), "child must be stopped");
    if let Some(row) = load_row(&project_id, &name) {
        assert_ne!(row.desired, DESIRED_RUNNING, "must not persist desired=running");
        assert_eq!(row.expose, EXPOSE_TUNNEL);
        assert!(row.pid.is_none());
    }
    // Simulated boot must not start it local-only.
    boot_desired_running();
    assert!(
        !port_accepts(port),
        "boot must not resurrect hostname-failed as local"
    );
    cleanup(&project_id, &name);
}

#[test]
fn run_same_name_while_running_is_400() {
    if !python_ok() {
        panic!("python3 is required");
    }
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let dir = _home.path().join("ws5");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("dup");
    let port = free_port();
    let spec = || RunSpec {
        project_id: project_id.clone(),
        name: name.clone(),
        cmd: listener_cmd(port),
        cwd: dir.clone(),
        port,
        no_tunnel: true,
        replace_spec: false,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: false,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    };
    run(spec()).expect("first run");
    let err = run(spec()).expect_err("second must fail");
    assert!(err.message.contains("stop first"), "got: {}", err.message);
    cleanup(&project_id, &name);
}

#[test]
fn skin_and_cmd_are_mutually_exclusive() {
    let _home = crate::test_support::TempHome::new();
    let dir = _home.path().join("ws-skin-xor");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let err = run(RunSpec {
        project_id: project_id.clone(),
        name: unique("xor"),
        cmd: "echo hi".into(),
        cwd: dir.clone(),
        port: 9,
        no_tunnel: true,
        replace_spec: false,
        skin: true,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: false,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    })
    .expect_err("cmd+skin");
    assert!(
        err.message.contains("--cmd and --skin are mutually exclusive"),
        "got: {}",
        err.message
    );
    let err = run(RunSpec {
        project_id,
        name: unique("cwdxor"),
        cmd: String::new(),
        cwd: dir,
        port: 9,
        no_tunnel: true,
        replace_spec: false,
        skin: true,
        skin_root: String::new(),
        cwd_explicit: true,
        transfer: false,
        probe_timeout: ProbeTimeout::Keep,
        dev: None,
    })
    .expect_err("cwd+skin");
    assert!(
        err.message.contains("--cwd and --skin are mutually exclusive"),
        "got: {}",
        err.message
    );
}

// ── Startup grace (`--probe-timeout`, found 2026-10-16 by Sew/Hearth) ──

/// A command that opens `port` only after `delay_s` seconds (a build
/// before the server binds).
fn late_listener_cmd(port: u16, delay_s: f32) -> String {
    if cfg!(windows) {
        // cmd.exe has no `sleep`/`exec`: python waits, then binds.
        format!(
            "{py} -c \"import time,socket,itertools; time.sleep({delay_s}); s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',{port})); s.listen(8); [s.accept()[0].close() for _ in itertools.count()]\"",
            py = k2_core::test_env::PYTHON
        )
    } else {
        format!("sleep {delay_s}; exec {}", listener_cmd(port))
    }
}

/// A service command that runs about `secs` and never listens.
fn idle_cmd(secs: u32) -> String {
    if cfg!(windows) {
        format!("ping -n {} 127.0.0.1 >nul", secs + 1)
    } else {
        format!("sleep {secs}")
    }
}

fn grace_spec(project_id: &str, name: &str, cmd: String, dir: &Path, port: u16, no_tunnel: bool, pt: ProbeTimeout) -> RunSpec {
    RunSpec {
        project_id: project_id.to_string(),
        name: name.to_string(),
        cmd,
        cwd: dir.to_path_buf(),
        port,
        no_tunnel,
        replace_spec: false,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: false,
        transfer: false,
        probe_timeout: pt,
        dev: None,
    }
}

// ── `k2 publish run --dev` (DM-1, DM-5, DM-6) ────────────────────────

/// A dev service's row, on the local lane so the start needs no control
/// plane. `run`'s own path (with a hostname) is in the headless CLI test.
fn dev_row(dir: &Path, project_id: &str, name: &str, cmd: &str, port: u16) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ps::insert(&conn, project_id, name, cmd, &dir.to_string_lossy(), port, EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "").expect("insert");
    ps::set_dev_caps(&conn, project_id, name, Some((1024, 1.0))).expect("caps");
    sv::set_dev(&conn, name, true, Some(600), "test").expect("dev");
}

fn dev_status(project_id: &str, name: &str) -> ServiceJson {
    to_json(&load_row(project_id, name).expect("row"))
}

/// DM-1: the live folder, loopback, and the address its hot-reload client
/// needs, whatever a service's env file says. DM-5: `ps` says how it runs.
#[test]
fn a_dev_server_runs_in_the_live_folder_on_loopback_and_says_how_it_runs() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let cfg = serde_json::json!({ "token": "tok_test", "subdomain": "rosson", "e2e": true });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    let dir = _home.path().join("ws-dev");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("devenv");
    let port = free_port();
    // A service command is the OS's shell line: sh on unix, cmd.exe on Windows.
    #[cfg(not(windows))]
    let cmd = format!("env > seen-env.txt; pwd > seen-cwd.txt; {}", listener_cmd(port));
    #[cfg(windows)]
    let cmd = format!("set > seen-env.txt & cd > seen-cwd.txt & {}", listener_cmd(port));
    dev_row(&dir, &project_id, &name, &cmd, port);
    let json = start_inner(&project_id, &name, false, false).expect("start");
    assert_eq!(json.status, STATUS_RUNNING, "{json:?}");
    let env = fs::read_to_string(dir.join("seen-env.txt")).expect("the server ran in the folder");
    assert!(env.lines().any(|l| l == "HOST=127.0.0.1"), "{env}");
    assert!(env.lines().any(|l| l == format!("PORT={port}")), "{env}");
    assert!(env.lines().any(|l| l == format!("K2_DEV_PUBLIC_HOST={name}.rosson.k2.dev")), "{env}");
    assert!(env.lines().any(|l| l == "K2_DEV_PUBLIC_PORT=443"), "{env}");
    assert!(env.lines().any(|l| l == "K2_DEV_PUBLIC_PROTOCOL=wss"), "{env}");
    let cwd = fs::read_to_string(dir.join("seen-cwd.txt")).expect("cwd");
    assert_eq!(dunce::canonicalize(cwd.trim()).unwrap(), dunce::canonicalize(&dir).unwrap(), "the live folder, never a copy");

    let dev = json.dev.expect("dev details");
    assert_eq!(dev.lane, "dev: live folder, not walled");
    assert_eq!(dev.idle_secs, Some(600));
    assert_eq!(dev.idle_stopped_at, None);
    #[cfg(target_os = "macos")]
    assert_eq!(dev.caps, "no caps (macOS has none)");
    // Linux: either read back from the scope, or "no caps" with why: never
    // what was merely asked for.
    #[cfg(target_os = "linux")]
    assert!(dev.caps == "1 GiB, 1 CPU" || dev.caps.starts_with("no caps ("), "{}", dev.caps);
    cleanup(&project_id, &name);
}

/// DM-5: an ordinary `--cmd` service gets none of it.
#[test]
fn a_plain_service_has_no_dev_details() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let dir = _home.path().join("ws-plain");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("plain");
    let port = free_port();
    let json = run(grace_spec(&project_id, &name, format!("env > seen-env.txt; {}", listener_cmd(port)), &dir, port, true, ProbeTimeout::Keep)).expect("run");
    assert!(json.dev.is_none(), "{:?}", json.dev);
    let env = fs::read_to_string(dir.join("seen-env.txt")).expect("env");
    assert!(!env.contains("K2_DEV_PUBLIC_HOST"), "{env}");
    assert_eq!(dev_wake(&name), DevWake::NotDev);
    cleanup(&project_id, &name);
}

/// `run` refuses what `--dev` can't be, before anything starts.
#[test]
fn dev_refuses_skin_and_no_tunnel_before_anything_starts() {
    let _home = crate::test_support::TempHome::new();
    let dir = _home.path().join("ws-refuse");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("refuse");
    let marker = dir.join("spawned");
    let mut spec = grace_spec(&project_id, &name, format!("touch {}", marker.display()), &dir, free_port(), true, ProbeTimeout::Keep);
    spec.dev = Some(crate::publish_dev::DevSpec::default());
    let err = run(spec).expect_err("--dev --no-tunnel");
    assert_eq!(err.body.as_ref().expect("coded")["code"], "dev_needs_tunnel", "{}", err.message);

    let mut spec = grace_spec(&project_id, &name, String::new(), &dir, free_port(), false, ProbeTimeout::Keep);
    spec.dev = Some(crate::publish_dev::DevSpec::default());
    spec.skin = true;
    let err = run(spec).expect_err("--dev --skin");
    assert_eq!(err.body.as_ref().expect("coded")["code"], "dev_needs_cmd", "{}", err.message);
    assert!(!marker.exists(), "nothing started");
    assert!(load_row(&project_id, &name).is_none(), "no row");
}

/// PV-4 / G8: with E2E off K2's check can't see the traffic, so a dev server
/// doesn't start, and `ps` doesn't call a plain site private as if K2 checked.
#[test]
fn e2e_off_refuses_a_dev_server_and_ps_says_private_isnt_enforced() {
    let _home = crate::test_support::TempHome::new();
    let cfg = serde_json::json!({ "token": "tok_test", "subdomain": "rosson", "e2e": false });
    fs::write(k2_core::paths::k2_home_in(_home.path()).join("tunnel.json"), cfg.to_string()).unwrap();
    let dir = _home.path().join("ws-e2e");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("e2eoff");
    let marker = dir.join("spawned");
    let mut spec = grace_spec(&project_id, &name, format!("touch {}", marker.display()), &dir, free_port(), false, ProbeTimeout::Keep);
    spec.dev = Some(crate::publish_dev::DevSpec::default());
    let err = run(spec).expect_err("no gate on this door");
    assert_eq!(err.body.as_ref().expect("coded")["code"], sv::DOOR_UNSUPPORTED, "{}", err.message);
    assert!(!marker.exists(), "nothing started");

    let db = k2_core::db::shared();
    let row = {
        let conn = db.lock();
        ps::insert(&conn, &project_id, &name, "true", &dir.to_string_lossy(), free_port(), EXPOSE_TUNNEL, DESIRED_STOPPED, KIND_CMD, "").expect("insert")
    };
    let v = to_json(&row).visibility.expect("visibility");
    assert_eq!((v.mode.as_str(), v.enforced), ("private", false));
    let conn = db.lock();
    let _ = ps::delete(&conn, &project_id, &name);
}

/// DM-6: stop after idle, show it as idle-stopped (never a crash), wake on a
/// visit that passed the gate, and a person's stop never wakes.
#[test]
fn a_dev_server_stops_when_idle_and_wakes_on_a_visit() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let dir = _home.path().join("ws-idle");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("idle");
    let port = free_port();
    dev_row(&dir, &project_id, &name, &listener_cmd(port), port);
    start_inner(&project_id, &name, false, false).expect("start");
    assert_eq!(dev_wake(&name), DevWake::Up, "running: dial it");

    // Not idle yet: a sweep now leaves it.
    assert!(!dev_idle_sweep(Instant::now()).contains(&name));
    assert_eq!(dev_status(&project_id, &name).status, STATUS_RUNNING);
    // A visit moves the clock: 9 minutes later, still not idle.
    dev_touch(&name);
    assert!(!dev_idle_sweep(Instant::now() + Duration::from_secs(9 * 60)).contains(&name));

    // Past the limit: stopped.
    let stopped = dev_idle_sweep(Instant::now() + Duration::from_secs(11 * 60));
    assert!(stopped.contains(&name), "{stopped:?}");
    assert!(!port_accepts(port), "the process is gone");
    let json = dev_status(&project_id, &name);
    assert_eq!(json.status, STATUS_IDLE_STOPPED, "{json:?}");
    assert_eq!(json.reason, None, "an idle stop is not a failure: {json:?}");
    assert!(json.dev.as_ref().expect("dev").idle_stopped_at.is_some(), "{json:?}");
    let row = load_row(&project_id, &name).expect("row");
    assert_eq!(row.desired, DESIRED_STOPPED, "a restart doesn't bring it back by itself");
    assert_eq!(row.last_exit_code, None, "never counted as a crash");

    // A visit that passed the gate starts it; it never blocks the caller.
    assert_eq!(dev_wake(&name), DevWake::Starting);
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && dev_wake(&name) != DevWake::Up {
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(dev_wake(&name), DevWake::Up, "woke up");
    let json = wait_status(&project_id, &name, STATUS_RUNNING, Duration::from_secs(8));
    assert_eq!(json.status, STATUS_RUNNING, "{json:?}");
    assert_eq!(json.dev.as_ref().expect("dev").idle_stopped_at, None, "{json:?}");

    // A person's stop outranks all of it: no wake, and it says why.
    stop(&project_id, &name).expect("stop");
    assert_eq!(dev_status(&project_id, &name).status, STATUS_STOPPED);
    match dev_wake(&name) {
        DevWake::Failed(why) => assert!(why.contains("stopped"), "{why}"),
        other => panic!("a stopped dev server must not wake: {other:?}"),
    }
    assert!(!port_accepts(port), "still down");
    cleanup(&project_id, &name);
}

/// `--idle off` (the owner's call) never stops.
#[test]
fn idle_off_never_stops() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let dir = _home.path().join("ws-off");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("idleoff");
    let port = free_port();
    dev_row(&dir, &project_id, &name, &listener_cmd(port), port);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        sv::set_dev(&conn, &name, true, Some(0), "test").expect("off");
    }
    start_inner(&project_id, &name, false, false).expect("start");
    assert!(!dev_idle_sweep(Instant::now() + Duration::from_secs(48 * 3600)).contains(&name));
    assert_eq!(dev_status(&project_id, &name).status, STATUS_RUNNING);
    assert_eq!(dev_status(&project_id, &name).dev.expect("dev").idle_secs, None);
    cleanup(&project_id, &name);
}

/// G8: a public site that turns into a dev server is private now; a plain
/// run turns it back into a normal site, which stays private.
#[test]
fn a_public_site_turning_into_a_dev_server_goes_private() {
    let _home = crate::test_support::TempHome::new();
    let name = unique("wasopen");
    let db = k2_core::db::shared();
    {
        let conn = db.lock();
        sv::set_public(&conn, &name, "before", "test").expect("public");
    }
    let spec = crate::publish_dev::DevSpec::default();
    sync_site_visibility(&name, true, Some(&spec)).expect("sync");
    {
        let conn = db.lock();
        let v = sv::effective(&conn, &name).expect("row");
        assert_eq!((v.mode, v.dev, v.idle_secs), (sv::Mode::Private, true, Some(3600)));
    }
    // A dev site someone made public on purpose stays public on a re-run.
    {
        let conn = db.lock();
        sv::set_public(&conn, &name, "demo", "test").expect("public");
    }
    sync_site_visibility(&name, true, Some(&spec)).expect("sync");
    {
        let conn = db.lock();
        assert_eq!(sv::effective(&conn, &name).expect("row").mode, sv::Mode::Public);
        sv::set_private(&conn, &name, "test").expect("private");
    }
    sync_site_visibility(&name, true, None).expect("sync");
    let conn = db.lock();
    let v = sv::effective(&conn, &name).expect("row");
    assert_eq!((v.mode, v.dev), (sv::Mode::Private, false), "back to normal, still private");
}

/// PV-4: `ps` warns for a site that listens on every address, once the check
/// has looked. 0.46.18: a public site too (its raw port skips the front door);
/// only the owner's `expose public` quiets it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ps_warns_when_a_sites_program_listens_beyond_loopback() {
    let _home = crate::test_support::TempHome::new();
    let wide = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("bind");
    let port = wide.local_addr().expect("addr").port();
    let dir = _home.path().join("ws-bind");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("wide");
    let db = k2_core::db::shared();
    let row = {
        let conn = db.lock();
        ps::insert(&conn, &project_id, &name, "true", &dir.to_string_lossy(), port, EXPOSE_TUNNEL, DESIRED_STOPPED, KIND_CMD, "").expect("insert")
    };
    assert!(to_json(&row).bind_warning.is_none(), "not checked yet");
    let exposed = check_bind(&row);
    assert_eq!(exposed.len(), 1, "{exposed:?}");
    let json = to_json(&row);
    assert_eq!(json.visibility.as_ref().expect("visibility").mode, "private");
    let w = json.bind_warning.expect("warned");
    assert!(w.contains(&format!(":{port}")) && w.contains("127.0.0.1"), "{w}");
    {
        let conn = db.lock();
        sv::set_public(&conn, &name, "open", "test").expect("public");
    }
    let w = to_json(&row).bind_warning.expect("a public site's raw port still skips the front door");
    assert!(w.contains("beyond this server") && w.contains("publish expose"), "{w}");
    crate::publish_expose::set(&row.id, true, "owner").expect("expose public");
    assert!(to_json(&row).bind_warning.is_none(), "the owner said it is public on purpose");
    drop(wide);
    let conn = db.lock();
    let _ = ps::delete(&conn, &project_id, &name);
}

// ── Where a --cmd service listens (0.46.18) ─────────────────────────────

/// A server that listens on `host`, then accepts forever.
#[cfg(unix)]
fn listener_on_cmd(host: &str, port: u16) -> String {
    format!(
        "{py} -c \"import socket,itertools; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('{host}',{port})); s.listen(8); [s.accept()[0].close() for _ in itertools.count()]\"",
        py = k2_core::test_env::PYTHON
    )
}

#[cfg(unix)]
fn env_lines(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("seen-env.txt")).expect("the service wrote its environment").lines().map(str::to_string).collect()
}

/// A `--cmd` service gets HOST=127.0.0.1 and its PORT (and the other
/// spellings), on the inherited environment (the services from before
/// 0.45.4) and on a clean one; one the owner marked public gets no HOST.
#[cfg(unix)]
#[test]
fn a_cmd_service_gets_host_and_port_unless_the_owner_marked_it_public() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let dir = _home.path().join("ws-hostport");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("hostport");
    let port = free_port();
    let cmd = format!("env > seen-env.txt; exec {}", listener_on_cmd("127.0.0.1", port));
    let json = run(grace_spec(&project_id, &name, cmd, &dir, port, true, ProbeTimeout::Keep)).expect("run");
    assert_eq!(json.status, STATUS_RUNNING, "{json:?}");
    let id = json.id.clone();
    assert!(crate::vault::publish_env::get(&id).mode == "legacy", "a fresh home has no vault yet: the service inherits, like the old ones");
    let env = env_lines(&dir);
    for want in ["HOST=127.0.0.1".to_string(), format!("PORT={port}"), "BINDING=127.0.0.1".to_string(), "UVICORN_HOST=127.0.0.1".to_string()] {
        assert!(env.contains(&want), "{want} missing: {env:?}");
    }
    assert!(!env.iter().any(|l| l.starts_with("HOSTNAME=127.0.0.1")), "never HOSTNAME: {env:?}");
    assert!(json.expose_public.is_none());

    // The owner marks it public; on a clean environment (nothing inherited)
    // its next start has no HOST at all, and keeps its PORT.
    stop(&project_id, &name).expect("stop");
    crate::vault::publish_env::set(&id, Some("clean"), None).expect("clean");
    let shown = set_expose(&project_id, &name, true, "owner").expect("expose public");
    assert_eq!(shown.expose_public.as_ref().map(|p| p.set_by.as_str()), Some("owner"), "{shown:?}");
    let json = start(&project_id, &name).expect("start");
    assert_eq!(json.status, STATUS_RUNNING, "{json:?}");
    let env = env_lines(&dir);
    assert!(!env.iter().any(|l| l.starts_with("HOST=") || l.starts_with("BINDING=")), "public: no loopback HOST: {env:?}");
    assert!(env.contains(&format!("PORT={port}")), "{env:?}");

    // Back to this server only: HOST again at the next start.
    stop(&project_id, &name).expect("stop");
    assert!(set_expose(&project_id, &name, false, "owner").expect("server").expose_public.is_none());
    start(&project_id, &name).expect("start");
    assert!(env_lines(&dir).contains(&"HOST=127.0.0.1".to_string()));
    cleanup(&project_id, &name);
}

/// The bind check after a start: a `--cmd` service listening on every
/// address is flagged in `ps` (with the exact fix), its log and the vault
/// doctor, and keeps running; one on 127.0.0.1 is not; the owner's `expose
/// public` quiets it and is shown.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_cmd_service_on_every_address_is_flagged_and_one_on_loopback_is_not() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "4000");
    let dir = _home.path().join("ws-wide");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());

    let wide = unique("wide");
    let wport = free_port();
    run(grace_spec(&project_id, &wide, format!("exec {}", listener_on_cmd("0.0.0.0", wport)), &dir, wport, true, ProbeTimeout::Keep)).expect("run wide");
    let narrow = unique("narrow");
    let nport = free_port();
    run(grace_spec(&project_id, &narrow, format!("exec {}", listener_on_cmd("127.0.0.1", nport)), &dir, nport, true, ProbeTimeout::Keep)).expect("run narrow");

    let wrow = load_row(&project_id, &wide).expect("row");
    let nrow = load_row(&project_id, &narrow).expect("row");
    assert!(wrow.pid.is_some() && nrow.pid.is_some(), "both running");
    let exposed = check_bind(&wrow);
    // Linux shows 0.0.0.0:<port>; lsof on macOS shows *:<port>.
    assert!(exposed.len() == 1 && exposed[0].ends_with(&format!(":{wport}")), "the wide one's own listener: {exposed:?}");
    assert!(check_bind(&nrow).is_empty(), "127.0.0.1 is this server only");

    let json = to_json(&wrow);
    assert_eq!(json.status, STATUS_RUNNING, "warned, never stopped");
    let w = json.bind_warning.expect("flagged");
    assert!(w.contains(&format!("{wide} listens on {}, beyond this server", exposed[0])), "{w}");
    assert!(w.contains(&format!("HOST=127.0.0.1 and PORT={wport}")), "{w}");
    assert!(w.contains(&format!("publish stop {wide} && ")) && w.contains(&format!("publish start {wide}")), "{w}");
    assert!(w.contains(&format!("publish expose {wide} public")), "{w}");
    assert!(json.visibility.is_none(), "a local service: flagged whatever its visibility");
    assert!(to_json(&nrow).bind_warning.is_none());

    // The service's log, once for this set of addresses.
    bind_check_one(&wrow);
    bind_check_one(&wrow);
    let log = fs::read_to_string(log_path(&project_id, &wide)).expect("log");
    assert_eq!(log.matches("[k2] warning: ").count(), 1, "{log}");
    assert!(log.contains(&format!("{wide} listens on {}", exposed[0])), "{log}");
    bind_check_one(&nrow);
    let nlog = fs::read_to_string(log_path(&project_id, &narrow)).unwrap_or_default();
    assert!(!nlog.contains("[k2] warning"), "{nlog}");

    // The vault doctor, for this workspace.
    let found = bind_doctor_findings(Some(&project_id));
    assert_eq!(found.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec![wide.as_str()], "{found:?}");
    assert!(bind_doctor_findings(Some("another-workspace")).iter().all(|(n, _)| n != &wide));

    // Public on purpose: no warning, shown with who said so.
    let shown = set_expose(&project_id, &wide, true, "owner").expect("expose public");
    assert!(shown.bind_warning.is_none(), "{shown:?}");
    let rec = shown.expose_public.expect("shown");
    assert_eq!(rec.set_by, "owner");
    assert!(rec.set_at > 0);
    let wire = serde_json::to_value(to_json(&wrow)).expect("json");
    assert_eq!(wire["exposePublic"]["setBy"], "owner", "{wire}");
    assert!(wire.get("bindWarning").is_none(), "{wire}");
    assert!(bind_doctor_findings(Some(&project_id)).is_empty());
    let audit = k2_core::auth_audit::tail(50).expect("audit");
    assert!(
        audit.iter().any(|e| e["event"] == "publish-expose" && e["outcome"].as_str().is_some_and(|o| o.contains(&format!("service={wide}")) && o.contains("expose=public"))),
        "audited: {audit:?}"
    );
    // Only a service this workspace has.
    assert!(set_expose(&project_id, "no-such-service", true, "owner").is_err());

    cleanup(&project_id, &wide);
    cleanup(&project_id, &narrow);
}

/// Poll `to_json` until `want` or `within` runs out; returns the last JSON.
fn wait_status(project_id: &str, name: &str, want: &str, within: Duration) -> ServiceJson {
    let deadline = Instant::now() + within;
    loop {
        let row = load_row(project_id, name).expect("row");
        let json = to_json(&row);
        if json.status == want || Instant::now() >= deadline {
            return json;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn drain_changed_for(rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>, project_id: &str) -> usize {
    let mut n = 0;
    loop {
        match rx.try_recv() {
            Ok(crate::session_events::SessionEvent::PublishServicesChanged { project_id: p }) if p == project_id => n += 1,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(_) => return n,
        }
    }
}

/// Control-plane mock for a Pro account whose nested `label` routes to
/// `127.0.0.1:<port>`. Counts POST/PUT writes (hostname publishes).
struct ProCp {
    writes: StdArc<AtomicUsize>,
    _cp_base: k2_core::test_env::EnvVar,
}

fn start_pro_cp(label: &str, port: u16) -> ProCp {
    let writes = StdArc::new(AtomicUsize::new(0));
    let writes2 = StdArc::clone(&writes);
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("mock cp");
    let cp_port = listener.local_addr().unwrap().port();
    let get_body = format!(
        r#"{{"subdomains":[{{"label":"rosson","primary":true,"connected":true,"tier":"pro"}},{{"label":"{label}","target":"127.0.0.1:{port}"}}]}}"#
    );
    thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok((mut sock, _)) = listener.accept() {
                sock.set_nonblocking(false).ok();
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let body = if req.starts_with("GET ") {
                    get_body.clone()
                } else {
                    writes2.fetch_add(1, AtomOrd::SeqCst);
                    "{}".to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            } else {
                thread::sleep(Duration::from_millis(20));
            }
        }
    });
    let cp_base = k2_core::test_env::EnvVar::set(
        subdomains::CONTROL_PLANE_BASE_ENV,
        format!("http://127.0.0.1:{cp_port}"),
    );
    ProCp { writes, _cp_base: cp_base }
}

fn write_pro_tunnel(home: &Path) {
    let cfg = serde_json::json!({ "token": "tok_test", "subdomain": "rosson", "e2e": true });
    fs::write(k2_core::paths::k2_home_in(&home).join("tunnel.json"), cfg.to_string()).unwrap();
}

/// Local: binds after the default window (300 ms here) but inside its
/// 6 s grace → the run answers `starting` with the time left, then the
/// background re-probe turns it `running` and emits the changed event.
#[test]
fn local_late_bind_within_grace_ends_running_with_event() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "300");
    let _reprobe = k2_core::test_env::EnvVar::set("K2_PUBLISH_REPROBE_MS", "50");
    let dir = _home.path().join("ws-grace-local");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("gracelocal");
    let port = free_port();
    let json = run(grace_spec(&project_id, &name, late_listener_cmd(port, 1.5), &dir, port, true, ProbeTimeout::Ms(6000)))
        .expect("run returns inside the grace");
    assert_eq!(json.status, STATUS_STARTING, "{json:?}");
    assert_eq!(json.probe_timeout_ms, Some(6000));
    assert_eq!(json.probe_timeout_effective_ms, 6000);
    let left = json.starting_left_ms.expect("startingLeftMs while starting");
    assert!(left > 4000 && left <= 6000, "left={left}");
    assert_eq!(json.desired, DESIRED_RUNNING, "boot must honour a service inside its grace");
    let mut rx = crate::session_events::subscribe();
    let json = wait_status(&project_id, &name, STATUS_RUNNING, Duration::from_secs(8));
    assert_eq!(json.status, STATUS_RUNNING, "late bind inside the grace must end running: {json:?}");
    assert!(json.starting_left_ms.is_none());
    assert!(json.error.is_none(), "{json:?}");
    assert!(
        drain_changed_for(&mut rx, &project_id) >= 1,
        "starting → running must emit PublishServicesChanged"
    );
    let row = load_row(&project_id, &name).unwrap();
    assert_eq!(row.probe_timeout_ms, Some(6000), "grace is stored on the row");
    cleanup(&project_id, &name);
}

/// Local with the default grace: the old "unhealthy forever" bug. Past
/// the grace it reads `unhealthy`, and keeps being re-probed, so a late
/// bind still turns it `running`.
#[test]
fn local_past_grace_is_reprobed_and_turns_running() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "300");
    let _reprobe = k2_core::test_env::EnvVar::set("K2_PUBLISH_REPROBE_MS", "50");
    let _supervise = k2_core::test_env::EnvVar::set("K2_PUBLISH_SUPERVISE_MS", "100");
    let dir = _home.path().join("ws-grace-unhealthy");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("graceunh");
    let port = free_port();
    let json = run(grace_spec(&project_id, &name, late_listener_cmd(port, 1.5), &dir, port, true, ProbeTimeout::Keep))
        .expect("local run keeps the process");
    assert_eq!(json.status, STATUS_UNHEALTHY, "past the default grace: {json:?}");
    assert_eq!(json.probe_timeout_ms, None, "no flag = default");
    assert_eq!(json.probe_timeout_effective_ms, 300, "default follows K2_PUBLISH_PROBE_MS");
    let json = wait_status(&project_id, &name, STATUS_RUNNING, Duration::from_secs(8));
    assert_eq!(json.status, STATUS_RUNNING, "re-probe must flip unhealthy → running: {json:?}");
    cleanup(&project_id, &name);
}

/// Tunnel: inside the grace the hostname is NOT published (no control-
/// plane write, no url, hostnamePending); the first port answer
/// publishes it and the service ends `running` with its url.
#[test]
fn tunnel_late_bind_publishes_hostname_after_first_answer() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "300");
    let _reprobe = k2_core::test_env::EnvVar::set("K2_PUBLISH_REPROBE_MS", "50");
    let _claim_tries = k2_core::test_env::EnvVar::set("K2_PUBLISH_CLAIM_TRIES", "4");
    write_pro_tunnel(_home.path());
    let name = unique("gracetun");
    let port = free_port();
    let cp = start_pro_cp(&name, port);
    let dir = _home.path().join("ws-grace-tunnel");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let json = run(grace_spec(&project_id, &name, late_listener_cmd(port, 1.5), &dir, port, false, ProbeTimeout::Ms(8000)))
        .expect("tunnelled run is not rolled back inside its grace");
    assert_eq!(json.status, STATUS_STARTING, "{json:?}");
    assert!(json.hostname_pending, "hostname waits for the first answer: {json:?}");
    assert!(json.url.is_none(), "no public url before the hostname exists: {json:?}");
    assert_eq!(cp.writes.load(AtomOrd::SeqCst), 0, "no control-plane write inside the grace");
    let json = wait_status(&project_id, &name, STATUS_RUNNING, Duration::from_secs(10));
    assert_eq!(json.status, STATUS_RUNNING, "{json:?}");
    // finish_start publishes the hostname, then flips the row.
    let deadline = Instant::now() + Duration::from_secs(5);
    let row = loop {
        let row = load_row(&project_id, &name).unwrap();
        if !row.hostname_pending || Instant::now() >= deadline {
            break row;
        }
        thread::sleep(Duration::from_millis(50));
    };
    assert!(!row.hostname_pending, "pending cleared after the attach");
    assert!(row.error.is_none(), "error: {:?}", row.error);
    assert_eq!(row.desired, DESIRED_RUNNING);
    assert!(cp.writes.load(AtomOrd::SeqCst) >= 1, "hostname published after the port answered");
    let json = to_json(&row);
    assert_eq!(json.url.as_deref(), Some(format!("https://{name}.rosson.k2.dev").as_str()));
    cleanup(&project_id, &name);
}

/// Tunnel that never binds: rolled back only when its grace runs out,
/// with an error that names the grace and the flag. No hostname.
#[test]
fn tunnel_never_binds_is_rolled_back_after_its_grace() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "300");
    let _reprobe = k2_core::test_env::EnvVar::set("K2_PUBLISH_REPROBE_MS", "50");
    write_pro_tunnel(_home.path());
    let name = unique("gracenever");
    let port = free_port();
    let cp = start_pro_cp(&name, port);
    let dir = _home.path().join("ws-grace-never");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let started = Instant::now();
    let json = run(grace_spec(&project_id, &name, idle_cmd(30), &dir, port, false, ProbeTimeout::Ms(1500)))
        .expect("inside the grace the run answers starting");
    assert_eq!(json.status, STATUS_STARTING, "{json:?}");
    let pid = json.pid.expect("pid") as i32;
    let deadline = Instant::now() + Duration::from_secs(6);
    let row = loop {
        let row = load_row(&project_id, &name).expect("row stays (stopped + error)");
        if row.desired == DESIRED_STOPPED || Instant::now() >= deadline {
            break row;
        }
        thread::sleep(Duration::from_millis(50));
    };
    assert!(started.elapsed() >= Duration::from_millis(1500), "not before the grace ran out");
    assert_eq!(row.desired, DESIRED_STOPPED, "rolled back after the grace");
    let err = row.error.clone().expect("rollback error");
    assert!(err.contains("never accepted within 1.5s"), "names the grace: {err}");
    assert!(err.contains("raise --probe-timeout"), "names the flag: {err}");
    assert!(!row.hostname_pending);
    assert!(row.pid.is_none());
    thread::sleep(Duration::from_millis(600));
    assert!(!pid_alive(pid), "rollback kills the process");
    assert_eq!(cp.writes.load(AtomOrd::SeqCst), 0, "never published a hostname");
    let json = to_json(&row);
    assert_eq!(json.status, STATUS_STOPPED);
    assert!(json.url.is_none(), "{json:?}");
    cleanup(&project_id, &name);
}

/// Tunnel with a grace no longer than the request window: the sync
/// rollback is unchanged, but its error names the grace.
#[test]
fn tunnel_sync_miss_error_names_the_grace() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "400");
    write_pro_tunnel(_home.path());
    let name = unique("gracesync");
    let port = free_port();
    let _cp = start_pro_cp(&name, port);
    let dir = _home.path().join("ws-grace-sync");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let err = run(grace_spec(&project_id, &name, idle_cmd(30), &dir, port, false, ProbeTimeout::Keep))
        .expect_err("default grace miss rolls back");
    assert!(err.message.contains("never accepted within 400ms"), "{}", err.message);
    assert!(err.message.contains("raise --probe-timeout"), "{}", err.message);
    cleanup(&project_id, &name);
}

/// The flag round-trips through storage: a re-run without it keeps the
/// stored grace, `default` clears it; over 15 min is refused before
/// anything is written.
#[test]
fn probe_timeout_is_stored_kept_reset_and_capped() {
    assert!(python_ok(), "python3 is required for published-service runtime tests");
    let _home = crate::test_support::TempHome::new();
    let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "3000");
    let dir = _home.path().join("ws-grace-store");
    fs::create_dir_all(&dir).unwrap();
    let project_id = make_project(&dir.to_string_lossy());
    let name = unique("gracestore");
    let port = free_port();
    let over = run(grace_spec(&project_id, &name, listener_cmd(port), &dir, port, true, ProbeTimeout::Ms(ps::PROBE_TIMEOUT_MAX_MS + 1)))
        .expect_err("over the cap");
    assert_eq!(over.status, "400 Bad Request");
    assert!(over.message.contains("at most 15m"), "{}", over.message);
    assert!(load_row(&project_id, &name).is_none(), "refused before any row");

    let json = run(grace_spec(&project_id, &name, listener_cmd(port), &dir, port, true, ProbeTimeout::Ms(ps::PROBE_TIMEOUT_MAX_MS)))
        .expect("15m is allowed");
    assert_eq!(json.status, STATUS_RUNNING);
    assert_eq!(json.probe_timeout_ms, Some(ps::PROBE_TIMEOUT_MAX_MS as i64));
    stop(&project_id, &name).expect("stop");
    thread::sleep(Duration::from_millis(300));

    let json = run(grace_spec(&project_id, &name, listener_cmd(port), &dir, port, true, ProbeTimeout::Keep))
        .expect("re-run without the flag");
    assert_eq!(json.probe_timeout_ms, Some(ps::PROBE_TIMEOUT_MAX_MS as i64), "kept");
    stop(&project_id, &name).expect("stop");
    thread::sleep(Duration::from_millis(300));

    let json = run(grace_spec(&project_id, &name, listener_cmd(port), &dir, port, true, ProbeTimeout::Default))
        .expect("re-run with default");
    assert_eq!(json.probe_timeout_ms, None, "default clears it");
    assert_eq!(json.probe_timeout_effective_ms, 3000);
    cleanup(&project_id, &name);
}

#[test]
fn fmt_duration_reads_like_the_flag() {
    assert_eq!(fmt_duration(Duration::from_millis(400)), "400ms");
    assert_eq!(fmt_duration(Duration::from_millis(1500)), "1.5s");
    assert_eq!(fmt_duration(Duration::from_secs(15)), "15s");
    assert_eq!(fmt_duration(Duration::from_secs(90)), "1m30s");
    assert_eq!(fmt_duration(Duration::from_secs(600)), "10m");
    assert_eq!(fmt_duration(Duration::from_secs(252)), "4m12s");
}

#[test]
fn validate_skin_root_rejects_abs_dotdot_and_escape() {
    let tmp = std::env::temp_dir().join(format!("k2-skin-root-{}", std::process::id()));
    let ws = tmp.join("ws");
    let outside = tmp.join("outside");
    fs::create_dir_all(ws.join("ui")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    assert_eq!(validate_skin_root(&ws, "").unwrap(), "");
    assert_eq!(validate_skin_root(&ws, "ui").unwrap(), "ui");
    assert!(validate_skin_root(&ws, "/tmp").unwrap_err().contains("workspace-relative"));
    assert!(validate_skin_root(&ws, "../outside").unwrap_err().contains(".."));
    assert!(validate_skin_root(&ws, "missing").unwrap_err().contains("not a directory"));
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn windows_job_objects_required() {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/c", "ping", "-n", "3", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        win_job::spawn_in_job(&mut cmd).expect("Job Objects are required on Windows");
    }
}

/// Windows: `terminate_and_wait` returns only once every process of
/// the job (cmd.exe and the grandchild it runs) has exited and the job
/// lists none.
#[cfg(windows)]
#[test]
fn a_job_is_empty_when_terminate_and_wait_returns() {
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/c", "ping", "-n", "30", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut child, job) = win_job::spawn_in_job(&mut cmd).expect("spawn in a job");
    let deadline = Instant::now() + Duration::from_secs(10);
    while job.pids().len() < 2 {
        assert!(Instant::now() < deadline, "ping never started under cmd.exe: {:?}", job.pids());
        thread::sleep(Duration::from_millis(20));
    }
    let before = job.pids();
    job.terminate_and_wait(Duration::from_secs(10)).expect("the job empties within the cap");
    assert!(job.pids().is_empty(), "nothing left in the job");
    for p in before {
        assert!(!win_job::pid_alive(p), "pid {p} exited");
    }
    child.wait().expect("reap cmd.exe");
}
