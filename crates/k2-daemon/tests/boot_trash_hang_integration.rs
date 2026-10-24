//! 0.46.4 — boot never blocks on the Trash. Headless: the real daemon
//! binary, a scratch HOME, no app.
//!
//! The first-boot work→inbox migration sends the old `.k2/work/` folder to
//! the Trash. On macOS that used to be an `osascript` → Finder child, which
//! never returns on a Mac with no GUI session (headless, before login, or
//! waiting on the Finder automation consent prompt): `/boot-status` stayed
//! `migrating` and every route answered 503.
//!
//! Here every trash call in the daemon hangs (`K2_TEST_TRASH_HANG=1`, a
//! test-build hook in `k2_core::safe_delete`). The daemon must still reach
//! `ready` quickly, serve a real route, move the inbox files, and leave the
//! `.k2/work/` folder exactly where it was (Trash, never delete).
//!
//! Spawn guards: temp HOME, own TMPDIR/XDG_RUNTIME_DIR under it,
//! `K2_HEARTBEAT_NO_SELF_HEAL=1`, `K2SO_WATCHDOG_DISABLED=1` and an EMPTY
//! `K2_TEST_AGENT_SHIM_DIR` so no real agent CLI can start.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_trimmed(p: &Path) -> Option<String> {
    let t = std::fs::read_to_string(p).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn curl(args: &[&str]) -> (String, String) {
    let out = Command::new("curl")
        .args(["-sS", "--max-time", "5", "-w", "\n%{http_code}"])
        .args(args)
        .output()
        .expect("run curl");
    let text = String::from_utf8(out.stdout).expect("utf8 body");
    let (body, code) = text.rsplit_once('\n').expect("curl prints the status line");
    (code.to_string(), body.to_string())
}

fn spawn(home: &Path, rt: &k2_core::test_env::DaemonRuntime, hang: bool) -> KillOnDrop {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_k2-daemon"));
    cmd.env("HOME", home)
        .envs(rt.envs())
        .env("K2_TEST_AGENT_SHIM_DIR", home.join("agent-shim-empty"))
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if hang {
        cmd.env(k2_core::safe_delete::TEST_TRASH_HANG_ENV, "1")
            .env(k2_core::safe_delete::TEST_BOOT_TRASH_TIMEOUT_MS_ENV, "1500");
    }
    KillOnDrop(cmd.spawn().expect("spawn k2-daemon"))
}

/// Port + token, then `/boot-status` phase `ready` within `budget`.
/// Returns (port, token, seconds from spawn to ready).
fn wait_ready(home: &Path, started: Instant, budget: Duration) -> (String, String, Duration) {
    let k2 = home.join(".k2");
    let deadline = started + budget;
    let (port, token) = loop {
        if let (Some(p), Some(t)) = (read_trimmed(&k2.join("heartbeat.port")), read_trimmed(&k2.join("heartbeat.token"))) {
            break (p, t);
        }
        assert!(Instant::now() < deadline, "daemon never wrote heartbeat.port/token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut last = String::new();
    loop {
        let (code, body) = curl(&[&format!("http://127.0.0.1:{port}/boot-status")]);
        if code == "200" {
            let v: serde_json::Value = serde_json::from_str(&body).expect("boot-status JSON");
            if v["phase"] == "ready" {
                return (port, token, started.elapsed());
            }
            last = body;
        }
        assert!(
            Instant::now() < deadline,
            "daemon never reached phase ready within {budget:?} (boot blocked on the Trash?): last boot-status {last}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn boot_reaches_ready_when_the_trash_never_answers() {
    let home = Home(std::env::temp_dir().join(format!("k2-boot-trash-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple())));
    std::fs::create_dir_all(home.0.join(".k2")).expect("HOME/.k2");
    std::fs::create_dir_all(home.0.join("agent-shim-empty")).expect("shim dir");
    let ws = home.0.join("ws-trash");
    std::fs::create_dir_all(&ws).expect("workspace dir");
    let ws_s = ws.to_string_lossy().into_owned();
    // Declared before the children so it drops (reaping frpc) after them.
    let rt = k2_core::test_env::DaemonRuntime::new(&home.0);

    // Boot 1 (normal): register the workspace.
    {
        let _d = spawn(&home.0, &rt, false);
        let (port, token, _) = wait_ready(&home.0, Instant::now(), Duration::from_secs(60));
        let (code, body) = curl(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "--data",
            &format!(r#"{{"path":"{ws_s}","seedWiki":false,"seedAgentsMd":false}}"#),
            &format!("http://127.0.0.1:{port}/cli/projects/add-without-git?token={token}"),
        ]);
        assert_eq!(code, "200", "workspace add: {body}");
        assert!(!body.contains("\"error\""), "workspace add failed: {body}");
    }
    for f in ["heartbeat.port", "heartbeat.token", "daemon.port", "daemon.token"] {
        let _ = std::fs::remove_file(home.0.join(".k2").join(f));
    }

    // An upgraded workspace's old layout: `.k2/work/inbox/a.md`, no marker.
    let dot = ws.join(".k2");
    let work = dot.join("work");
    std::fs::create_dir_all(work.join("inbox")).expect("work/inbox");
    std::fs::write(work.join("inbox").join("a.md"), "keep me").expect("work item");
    std::fs::write(work.join("notes.txt"), "not a work item").expect("extra file");
    let marker = dot.join(".work-to-inbox-migration-v1-done");
    if marker.exists() {
        std::fs::remove_file(&marker).expect("clear the migration marker");
    }

    // Boot 2: every trash call hangs. Boot must still reach ready, fast
    // (the bounded wait is 1.5 s here; 10 s in production).
    let started = Instant::now();
    let _d = spawn(&home.0, &rt, true);
    let (port, token, took) = wait_ready(&home.0, started, Duration::from_secs(60));
    assert!(took < Duration::from_secs(45), "ready, but only after {took:?}");

    // A real route answers (not the boot 503).
    let (code, body) = curl(&[&format!("http://127.0.0.1:{port}/cli/heartbeat/scheduler-status?token={token}")]);
    assert_eq!(code, "200", "a real route after ready: {body}");

    // The migration did its moves and wrote its marker…
    assert_eq!(
        std::fs::read_to_string(dot.join("inbox").join("a.md")).expect("moved inbox item"),
        "keep me"
    );
    assert!(marker.is_file(), "migration marker written");
    // …and the folder whose trash never answered is still there, contents
    // intact: Trash, never delete.
    assert!(work.is_dir(), ".k2/work left in place");
    assert_eq!(std::fs::read_to_string(work.join("notes.txt")).expect("extra file kept"), "not a work item");
}
