//! App runtime update continuity R19/R37/R36 (0.46.4, Rosson Q8: the
//! listener hand-over ships in 0.46.4; T26's shape without a VM).
//!
//! The real daemon binary, headless, in a scratch HOME (no supervisor), with
//! a runtime App on the process engine (`K2_APPS_PROCESS=1`, python3):
//!
//! 1. A request held open on the App's **front port** (`GET /slow=10`) and a
//!    prober opening a new front-port connection every 50 ms.
//! 2. `POST /cli/daemon/handoff`: the successor takes the front socket over
//!    (R19) instead of moving the App to a random port.
//! 3. The held request completes (the old daemon drains front connections
//!    too, R37), the prober saw zero failed connections, `apps.front_port`
//!    is unchanged (R36/D14), and once the old daemon has exited the same
//!    port keeps serving from the successor.
//!
//! Spawn guards as `handoff_keeps_app_connections`. Needs `python3`.
// The daemon hand-off passes listening sockets by fd (Unix); Windows restart
// hand-off comes with the ptyd/restart port (0.47).
#![cfg(unix)]

#[path = "support/app_fixture.rs"]
#[allow(dead_code)]
mod app_fixture;

use std::io::{Read, Write};
use std::net::TcpStream as StdTcp;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Reaper {
    child: Option<Child>,
    pids: Vec<i32>,
    home: Option<PathBuf>,
}

impl Drop for Reaper {
    fn drop(&mut self) {
        for p in &self.pids {
            unsafe { libc::kill(*p, libc::SIGKILL) };
        }
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        // The App outlives its daemons by design, and this test's too.
        if let Some(home) = &self.home {
            app_fixture::reap_apps_under(home);
        }
    }
}

fn read_trimmed(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display())).trim().to_string()
}

/// One HTTP/1.1 request to the daemon's main listener; (status, body).
fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = StdTcp::connect(("127.0.0.1", port)).unwrap_or_else(|e| panic!("connect daemon: {e}"));
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let body = body.unwrap_or("");
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

fn wait_for(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn front_port_of(dport: u16, token: &str, app: &str) -> u16 {
    let (st, body) = http(dport, "GET", &format!("/cli/app/inspect?app={app}&token={token}"), None);
    assert_eq!(st, 200, "inspect: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    v["app"]["frontPort"].as_u64().unwrap_or_else(|| panic!("no frontPort: {body}")) as u16
}

#[test]
fn handoff_keeps_the_app_front_port_and_its_connections() {
    app_fixture::require_python3();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home: PathBuf = std::env::temp_dir().join(format!("k2-afp-{}-{nanos}", std::process::id()));
    let k2 = home.join(".k2");
    let (run, tmp, shim, ws_dir) = (home.join("run"), home.join("tmp"), home.join("shim"), home.join("ws-afp"));
    for d in [&k2, &run, &tmp, &shim, &ws_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let folder = app_fixture::app_folder(&ws_dir.join("afp-app"), "afp", "1.0.0", "v1");
    let log_path = home.join("daemon.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut reaper = Reaper::default();
    reaper.home = Some(home.clone());
    reaper.child = Some(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &home)
            .env("SHELL", "/bin/false")
            .env("XDG_RUNTIME_DIR", &run)
            .env("TMPDIR", &tmp)
            .env("K2_APPS", "1")
            .env("K2_APPS_PROCESS", "1")
            .env("K2_TUNNEL_LEASE", "off")
            .env("K2_CONNECT_BASE", "http://127.0.0.1:1")
            .env("K2_CERT_BROKER_URL", "http://127.0.0.1:1")
            .env("K2_HANDOFF_DRAIN_SECS", "120")
            .env("K2_HANDOFF_DROPIN", "off")
            .env("K2_TEST_AGENT_SHIM_DIR", &shim)
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .env_remove("INVOCATION_ID")
            .env_remove("NOTIFY_SOCKET")
            .env_remove("XPC_SERVICE_NAME")
            .env_remove("K2_AIRGAP")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("spawn daemon"),
    );
    let old_pid = reaper.child.as_ref().unwrap().id() as i32;
    let dump = |why: &str| -> String {
        let t = std::fs::read_to_string(&log_path).unwrap_or_default();
        let tail: Vec<&str> = t.lines().rev().take(100).collect();
        format!("{why}\n--- daemon.log (tail) ---\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
    };

    wait_for(60, "daemon.port + daemon.token", || k2.join("daemon.port").exists() && k2.join("daemon.token").exists());
    let dport: u16 = read_trimmed(&k2.join("daemon.port")).parse().unwrap();
    let token = read_trimmed(&k2.join("daemon.token"));
    wait_for(60, "boot ready", || http(dport, "GET", &format!("/cli/daemon/handoff/status?token={token}"), None).0 == 200);

    // A workspace and its App, live on the process engine.
    let wsp = ws_dir.to_string_lossy().to_string();
    let (st, body) = http(
        dport,
        "POST",
        &format!("/cli/projects/add-without-git?token={token}"),
        Some(&serde_json::json!({ "path": wsp, "seedWiki": false, "seedAgentsMd": false }).to_string()),
    );
    assert_eq!(st, 200, "{}", dump(&format!("workspace add: {body}")));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let ws_id = v["id"].as_str().or(v["project"]["id"].as_str()).unwrap_or_else(|| panic!("no workspace id: {body}")).to_string();
    let (st, body) = http(
        dport,
        "POST",
        &format!("/cli/app/install?token={token}"),
        Some(&serde_json::json!({ "app": "afp", "workspace": ws_id, "engine": "process" }).to_string()),
    );
    assert_eq!(st, 200, "{}", dump(&format!("install: {body}")));
    let (st, body) = http(
        dport,
        "POST",
        &format!("/cli/app/deploy?token={token}"),
        Some(&serde_json::json!({ "app": "afp", "folder": folder, "wait": 60 }).to_string()),
    );
    assert_eq!(st, 200, "{}", dump(&format!("deploy: {body}")));
    assert!(body.contains("\"result\":\"live\""), "{}", dump(&format!("deploy: {body}")));
    let front = front_port_of(dport, &token, "afp");
    assert_eq!(app_fixture::http_get(front, "/").map(|(s, _)| s), Some(200), "{}", dump("the front port serves"));

    // A request held open on the front port across the hand-off.
    let held = std::thread::spawn(move || app_fixture::http_get(front, "/slow=10"));
    std::thread::sleep(Duration::from_millis(300));
    // The prober: a new front-port connection every 50 ms.
    let stop = Arc::new(AtomicBool::new(false));
    let ok_n = Arc::new(AtomicUsize::new(0));
    let fails = Arc::new(Mutex::new(Vec::<String>::new()));
    let prober = {
        let (stop, ok_n, fails) = (stop.clone(), ok_n.clone(), fails.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match app_fixture::http_get(front, "/") {
                    Some((200, _)) => {
                        ok_n.fetch_add(1, Ordering::SeqCst);
                    }
                    other => fails.lock().unwrap().push(format!("{other:?}")),
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };
    std::thread::sleep(Duration::from_millis(300));

    // ── The hand-off ──────────────────────────────────────────────────
    let (st, body) = http(dport, "POST", &format!("/cli/daemon/handoff?token={token}"), Some(""));
    assert_eq!(st, 202, "{}", dump(&format!("handoff: {body}")));
    let lock = k2.join("daemon.lock");
    wait_for(60, "the successor to own daemon.lock", || {
        std::fs::read_to_string(&lock).ok().and_then(|s| s.trim().parse::<i32>().ok()).is_some_and(|p| p != old_pid)
    });
    let new_pid: i32 = read_trimmed(&lock).parse().unwrap();
    reaper.pids.push(new_pid);
    wait_for(60, "the successor's token", || read_trimmed(&k2.join("daemon.token")) != token);
    let new_token = read_trimmed(&k2.join("daemon.token"));
    wait_for(60, "the successor ready", || {
        let (s, b) = http(dport, "GET", &format!("/cli/daemon/handoff/status?token={new_token}"), None);
        s == 200 && b.contains("\"bootedFromHandoff\":true")
    });

    // While the old daemon still drains (the held request keeps it), new
    // front connections are served: by the successor, through the socket
    // it took over (the old one stopped accepting at quiesce, R37).
    assert!(reaper.child.as_mut().unwrap().try_wait().unwrap().is_none(), "{}", dump("the old daemon must still be draining"));
    let base = ok_n.load(Ordering::SeqCst);
    wait_for(20, "front connections served during the drain", || ok_n.load(Ordering::SeqCst) >= base + 5);
    assert!(reaper.child.as_mut().unwrap().try_wait().unwrap().is_none(), "{}", dump("served while the old daemon drained"));
    // The held request completes through the old daemon (R37).
    let got = held.join().unwrap();
    assert_eq!(got.as_ref().map(|(s, _)| *s), Some(200), "{}", dump(&format!("the held front request: {got:?}")));
    // The front port never moved (R36/D14), and the successor took the
    // socket (R19).
    assert_eq!(front_port_of(dport, &new_token, "afp"), front, "{}", dump("apps.front_port must not move"));
    // (Both daemons log to one file and their lines can interleave: the
    // predecessor's line is checked by its prefix only.)
    let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(log_text.contains("handed over 1 App listener"), "{}", dump("the predecessor handed the front socket over"));

    // The old daemon exits once its last connection closed; the front port
    // keeps serving from the successor alone.
    // The old daemon is this test's child: it has exited once it can be
    // reaped (a zombie still answers kill(0)).
    let old = reaper.child.as_mut().unwrap();
    wait_for(120, "the old daemon to exit", || old.try_wait().ok().flatten().is_some());
    std::thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::SeqCst);
    prober.join().unwrap();
    let fails = fails.lock().unwrap().clone();
    assert!(fails.is_empty(), "{}", dump(&format!("{} failed front-port probe(s) across the hand-off: {fails:?}", fails.len())));
    assert!(ok_n.load(Ordering::SeqCst) > 20, "the prober ran");
    assert_eq!(app_fixture::http_get(front, "/").map(|(s, _)| s), Some(200), "{}", dump("the successor serves the same front port"));
    drop(reaper);
    let _ = std::fs::remove_dir_all(&home);
}
