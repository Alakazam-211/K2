//! `k2 push log` (0.46.3) — headless: the real daemon binary, no app, a
//! scratch HOME. `GET /cli/push/log` answers the owner with the attempt
//! ring, refuses anyone else (403) and a POST (405); `k2 push log
//! [--json] [--device <id>]` reaches it and passes the filter through.
//! (A fresh daemon has sent no pushes, so the ring is empty here; the
//! records themselves are covered by k2-core's push tests and the route
//! test in `push_routes`.)
//!
//! Spawn guards as in `heartbeat_next_fire_cli_integration`: temp HOME, an
//! EMPTY `K2_TEST_AGENT_SHIM_DIR`, no heartbeat self-heal, no watchdog.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch_home() -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "k2-push-log-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(home.join(".k2")).unwrap();
    home
}

fn read_trimmed(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap().trim().to_string()
}

/// `curl` → (HTTP status, body).
fn curl(args: &[&str]) -> (u16, String) {
    let out = Command::new("curl")
        .args(["-sS", "--max-time", "10", "-w", "\n%{http_code}"])
        .args(args)
        .output()
        .expect("run curl");
    assert!(out.status.success(), "curl {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).expect("utf8 body");
    let (body, code) = text.rsplit_once('\n').expect("status line");
    (code.trim().parse().expect("status"), body.to_string())
}

fn k2_cli(home: &Path, workspace: &Path, args: &[&str]) -> Output {
    let cli = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    Command::new("bash")
        .arg(&cli)
        .args(args)
        // A clean env: the outer shell may be a K2 agent cell whose
        // K2_*/K2SO_* hook vars would point the CLI at the real daemon.
        .env_clear()
        .env("PATH", std::env::var("PATH").expect("PATH"))
        .env("HOME", home)
        .env("K2_PROJECT_PATH", workspace)
        .current_dir(workspace)
        .output()
        .expect("run cli/k2")
}

fn ok_stdout(out: Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

#[tokio::test(flavor = "current_thread")]
async fn headless_push_log_route_and_cli() {
    let home = scratch_home();
    let k2_dir = home.join(".k2");
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let workspace = home.join("ws-push-log");
    std::fs::create_dir_all(&workspace).unwrap();

    let _rt = k2_core::test_env::DaemonRuntime::new(&home);
    let _daemon = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &home)
            .envs(_rt.envs())
            .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    while !(k2_dir.join("heartbeat.port").exists() && k2_dir.join("heartbeat.token").exists()) {
        assert!(Instant::now() < deadline, "daemon never wrote heartbeat.port/token");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let port = read_trimmed(&k2_dir.join("heartbeat.port"));
    let token = read_trimmed(&k2_dir.join("heartbeat.token"));
    let base = format!("http://127.0.0.1:{port}");
    loop {
        let out = Command::new("curl")
            .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "2"])
            .arg(format!("{base}/cli/push/log?token={token}"))
            .output()
            .expect("curl");
        if String::from_utf8_lossy(&out.stdout) == "200" {
            break;
        }
        assert!(Instant::now() < deadline, "daemon never became ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Route: the owner reads the ring.
    let (code, body) = curl(&[&format!("{base}/cli/push/log?token={token}")]);
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("log JSON");
    assert_eq!(v["ok"], true);
    assert_eq!(v["cap"], 200);
    assert!(v["device"].is_null());
    assert!(v["attempts"].is_array(), "{v}");
    // The filter is cut to 8 characters.
    let (code, body) = curl(&[&format!("{base}/cli/push/log?token={token}&device=abcdefgh-the-rest")]);
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("log JSON");
    assert_eq!(v["device"], "abcdefgh");
    assert_eq!(v["attempts"], serde_json::json!([]));
    // No token / a wrong token: refused.
    let (code, _) = curl(&[&format!("{base}/cli/push/log")]);
    assert_eq!(code, 403);
    let (code, _) = curl(&[&format!("{base}/cli/push/log?token=not-the-owner")]);
    assert_eq!(code, 403);
    // Read-only: a POST is 405.
    let (code, _) = curl(&["-X", "POST", "--data-raw", "{}", &format!("{base}/cli/push/log?token={token}")]);
    assert_eq!(code, 405);

    // CLI: --json passes the body through; --device reaches the route.
    let json = ok_stdout(k2_cli(&home, &workspace, &["push", "log", "--json"]), "k2 push log --json");
    let v: serde_json::Value = serde_json::from_str(&json).expect("k2 push log --json");
    assert_eq!(v["ok"], true);
    assert_eq!(v["cap"], 200);
    let json = ok_stdout(
        k2_cli(&home, &workspace, &["push", "log", "--device", "1234abcd-phone", "--json"]),
        "k2 push log --device --json",
    );
    let v: serde_json::Value = serde_json::from_str(&json).expect("k2 push log --device --json");
    assert_eq!(v["device"], "1234abcd");
    let json = ok_stdout(k2_cli(&home, &workspace, &["push", "log", "--json", "--device=feedf00d"]), "--device=");
    let v: serde_json::Value = serde_json::from_str(&json).expect("--device= json");
    assert_eq!(v["device"], "feedf00d");
    // Human view on an empty ring.
    let text = ok_stdout(k2_cli(&home, &workspace, &["push", "log", "--device", "1234abcd"]), "k2 push log --device");
    assert_eq!(text.trim(), "no push attempts since the daemon started for device 1234abcd");
    // Usage errors exit 2.
    let bad = k2_cli(&home, &workspace, &["push", "log", "--bogus"]);
    assert_eq!(bad.status.code(), Some(2), "stderr={}", String::from_utf8_lossy(&bad.stderr));
    let bad = k2_cli(&home, &workspace, &["push", "log", "--device"]);
    assert_eq!(bad.status.code(), Some(2), "stderr={}", String::from_utf8_lossy(&bad.stderr));
    // The help names the verb.
    let help = ok_stdout(k2_cli(&home, &workspace, &["push", "help"]), "k2 push help");
    assert!(help.contains("k2 push log [--json] [--device <id>]"), "{help}");

    drop(_daemon);
    let _ = std::fs::remove_dir_all(&home);
}
