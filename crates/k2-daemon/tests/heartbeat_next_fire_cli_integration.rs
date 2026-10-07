//! Heartbeat S3 (HB24) — headless: the real daemon binary, no app, a
//! scratch HOME. `/cli/heartbeat/list` returns `nextFireAt` and
//! `waitReason`; `k2 heartbeat list` prints them in its `Next` and
//! `Waiting` columns and `k2 heartbeat show` prints them as lines.
//!
//! Spawn guards: temp HOME (S1's transport guard refuses a HOME that is
//! not the password-database home), `K2_HEARTBEAT_NO_SELF_HEAL=1`, and an
//! EMPTY `K2_TEST_AGENT_SHIM_DIR` so no real agent CLI can start.

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

fn scratch_home() -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "k2-hb-s3-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(home.join(".k2")).unwrap();
    home
}

fn read_trimmed(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap().trim().to_string()
}

fn curl(args: &[&str]) -> String {
    let out = Command::new("curl")
        .args(["-sS", "--max-time", "10"])
        .args(args)
        .output()
        .expect("run curl");
    assert!(out.status.success(), "curl {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf8 body")
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn k2_cli(home: &Path, workspace: &str, args: &[&str]) -> String {
    let cli = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let out = Command::new("bash")
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
        .expect("run cli/k2");
    assert!(
        out.status.success(),
        "k2 {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

#[tokio::test(flavor = "current_thread")]
async fn headless_list_route_and_cli_show_next_fire_and_wait_reason() {
    let home = scratch_home();
    let k2_dir = home.join(".k2");
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let workspace = home.join("ws-s3");
    std::fs::create_dir_all(&workspace).unwrap();
    let ws = workspace.to_string_lossy().into_owned();

    let _daemon = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &home)
            .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );

    // Port + token, then wait for the readiness gate.
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
            .arg(format!("{base}/cli/heartbeat/scheduler-status?token={token}"))
            .output()
            .expect("curl");
        if String::from_utf8_lossy(&out.stdout) == "200" {
            break;
        }
        assert!(Instant::now() < deadline, "daemon never became ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Register the workspace, then add a 15-minute heartbeat.
    let add_ws = curl(&[
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
        "--data",
        &format!(r#"{{"path":"{ws}","seedWiki":false,"seedAgentsMd":false}}"#),
        &format!("{base}/cli/projects/add-without-git?token={token}"),
    ]);
    assert!(!add_ws.contains("\"error\""), "workspace add failed: {add_ws}");
    let spec = urlencode(r#"{"every_seconds":900}"#);
    // 0.44.4: heartbeat/add is POST-only (params stay in the query).
    let add_hb = curl(&[
        "-X",
        "POST",
        "--data-raw",
        "",
        &format!(
            "{base}/cli/heartbeat/add?token={token}&project={}&name=drive&frequency=hourly&spec={spec}",
            urlencode(&ws)
        ),
    ]);
    assert!(add_hb.contains("\"name\":\"drive\""), "heartbeat add failed: {add_hb}");

    // Route: the stored next fire and a named reason.
    let list = curl(&[&format!(
        "{base}/cli/heartbeat/list?token={token}&project={}",
        urlencode(&ws)
    )]);
    let rows: serde_json::Value = serde_json::from_str(&list).expect("list JSON");
    let row = rows
        .as_array()
        .expect("list is an array")
        .iter()
        .find(|r| r["name"] == "drive")
        .expect("drive listed");
    let created = row["createdAt"].as_i64().expect("createdAt");
    let expected_next = chrono::DateTime::from_timestamp(created + 900, 0)
        .expect("timestamp")
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    assert_eq!(row["nextFireAt"], serde_json::json!(expected_next), "row: {row}");
    let reason = row["waitReason"].as_str().expect("waitReason is set at create");
    assert!(
        k2_core::heartbeats::wait::ALL_REASONS.contains(&reason),
        "reason {reason} is in the HB20 vocabulary"
    );
    assert!(row["waitSince"].is_string(), "waitSince set: {row}");

    // CLI: `k2 heartbeat list` columns.
    let table = k2_cli(&home, &ws, &["heartbeat", "list"]);
    let header = table.lines().next().expect("header line");
    assert!(header.contains("Next") && header.contains("Waiting"), "header: {header}");
    let drive_line = table
        .lines()
        .find(|l| l.starts_with("drive"))
        .unwrap_or_else(|| panic!("no drive row in:\n{table}"));
    let local_next = chrono::DateTime::parse_from_rfc3339(&expected_next)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    assert!(drive_line.contains(&local_next), "Next column {local_next} in: {drive_line}");
    assert!(drive_line.contains(reason), "Waiting column {reason} in: {drive_line}");

    // CLI: `k2 heartbeat show` lines; `--json` passes the fields through.
    let show = k2_cli(&home, &ws, &["heartbeat", "show", "drive"]);
    assert!(
        show.contains(&format!("Next fire:      {expected_next}")),
        "show output:\n{show}"
    );
    assert!(show.contains(&format!("Waiting:        {reason}")), "show output:\n{show}");
    let json = k2_cli(&home, &ws, &["heartbeat", "show", "drive", "--json"]);
    let shown: serde_json::Value = serde_json::from_str(&json).expect("show --json");
    assert_eq!(shown["nextFireAt"], serde_json::json!(expected_next));
    assert_eq!(shown["waitReason"], serde_json::json!(reason));

    // AH32: instructions alone go through the daemon (no client-side file
    // write); show reads them back (AH9); status names who edited (AH18).
    let edited = k2_cli(&home, &ws, &["heartbeat", "edit", "drive", "--instructions", "Sweep the drive."]);
    assert!(edited.contains("Wake instructions written by the daemon: drive"), "edit output:\n{edited}");
    let show = k2_cli(&home, &ws, &["heartbeat", "show", "drive"]);
    assert!(show.contains("Instructions:\n  Sweep the drive."), "show output:\n{show}");
    let status = k2_cli(&home, &ws, &["heartbeat", "status", "drive"]);
    let line = status
        .lines()
        .find(|l| l.contains("instructions edited"))
        .unwrap_or_else(|| panic!("no instructions-edited row in status:\n{status}"));
    assert!(line.contains("changed"), "{line}");
    assert!(line.contains("owner"), "the owner token is named: {line}");

    drop(_daemon);
    let _ = std::fs::remove_dir_all(&home);
}
