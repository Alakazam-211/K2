//! `POST /cli/sandbox/open` — owner token only. On this Mac build
//! `can_sandbox()` is false, so an accepted request is 409 and creates
//! nothing. A connect-user session, a skin token, and an API key do not pass.
//! `$HOME` is a temp dir so the test daemon does not touch the user's home.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::time::Duration;

use k2_daemon::test_harness;

const OWNER: &str = "owner-token-sandbox-open";

struct Resp {
    status: u16,
    body: String,
}

struct HomeGuard {
    prev: Option<std::ffi::OsString>,
    dir: PathBuf,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_home() -> HomeGuard {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("k2-sandbox-open-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp HOME");
    let prev = std::env::var_os("HOME");
    std::env::set_var("HOME", &dir);
    std::env::remove_var("K2_API");
    std::env::remove_var("K2_SANDBOX_API");
    std::env::remove_var("K2_SANDBOX");
    HomeGuard { prev, dir }
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let req = match body {
        Some(body) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                ) =>
            {
                break
            }
            Err(e) => panic!("read: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("status from {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

fn rows_for(slug: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM sandbox_sessions WHERE workspace_slug = ?1",
        rusqlite::params![slug],
        |r| r.get(0),
    )
    .expect("count")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_token_409s_and_non_owner_tokens_are_rejected() {
    let home = temp_home();
    assert!(
        !k2_daemon::v2_spawn::can_sandbox(),
        "test premise: this build cannot sandbox"
    );
    k2_core::db::init_for_tests();
    let uniq = uuid::Uuid::new_v4();
    let path = format!("/tmp/k2-sbx-http-{uniq}");
    let handle = format!("http{uniq}").replace('-', "");
    let preset = format!("codex-{uniq}");
    let disabled = format!("off-{uniq}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let claude: String = conn
            .query_row(
                "SELECT id FROM agent_presets WHERE label = 'Claude' AND enabled = 1 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("claude preset");
        conn.execute(
            "INSERT INTO projects (id, name, path, handle, default_agent) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), "Pretty HTTP", &path, &handle, claude],
        )
        .expect("project");
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, 'Codex row', 'codex --full-auto', '', 1, 999, 0)",
            rusqlite::params![preset],
        )
        .expect("preset");
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, 'Off row', 'codex --full-auto', '', 0, 999, 0)",
            rusqlite::params![disabled],
        )
        .expect("disabled");
    }
    k2_core::connect_users::add_user("sbxopen", "password123").expect("add connect user");
    let session = k2_core::connect_users::create_session("sbxopen");

    let daemon = test_harness::start(OWNER).await;
    let open = format!("/cli/sandbox/open?token={OWNER}");
    let ok_body = format!(
        r#"{{"project_path":"{path}","preset_id":"{preset}","command":"evil-bin","args":["do-not-run"]}}"#
    );

    let accepted = http(daemon.port, "POST", &open, Some(&ok_body));
    assert_eq!(
        accepted.status, 409,
        "owner must be accepted into the 409; {}",
        accepted.body
    );
    assert!(
        accepted
            .body
            .contains("this daemon cannot sandbox (microVM backend unavailable)"),
        "{}",
        accepted.body
    );
    assert_eq!(rows_for(&handle), 0);
    assert!(!home.dir.join(".k2").join("sandbox-homes").exists());
    assert!(!home.dir.join(".k2").join("sandbox-overlays").exists());
    assert!(!home.dir.join(".k2").join("sandbox-sessions").exists());

    for token in [
        session.as_str(),
        "k2skn_skin-token",
        "k2sk_api-key",
        "not-the-owner",
    ] {
        let denied = http(
            daemon.port,
            "POST",
            &format!("/cli/sandbox/open?token={token}"),
            Some(&ok_body),
        );
        assert_eq!(denied.status, 403, "token {token} body={}", denied.body);
        assert!(
            denied.body.contains("invalid or missing token"),
            "{}",
            denied.body
        );
    }
    assert_eq!(rows_for(&handle), 0);

    let missing = http(
        daemon.port,
        "POST",
        &open,
        Some(&format!(
            r#"{{"project_path":"{path}","preset_id":"no-such"}}"#
        )),
    );
    assert_eq!(missing.status, 400, "{}", missing.body);
    assert_ne!(missing.status, 409);

    let off = http(
        daemon.port,
        "POST",
        &open,
        Some(&format!(
            r#"{{"project_path":"{path}","preset_id":"{disabled}"}}"#
        )),
    );
    assert_eq!(off.status, 400, "{}", off.body);

    let worktree = http(
        daemon.port,
        "POST",
        &open,
        Some(&format!(
            r#"{{"project_path":"{path}/worktrees/feat","preset_id":"{preset}"}}"#
        )),
    );
    assert_eq!(worktree.status, 400, "{}", worktree.body);
    assert!(
        worktree.body.contains("not a registered project"),
        "{}",
        worktree.body
    );

    let public = http(
        daemon.port,
        "POST",
        &format!("/v1/w/{handle}/sessions?token={OWNER}"),
        Some(r#"{"preset_id":"x","command":"evil"}"#),
    );
    assert_eq!(
        public.status, 404,
        "public /v1 door stays absent without K2_SANDBOX_API; {}",
        public.body
    );
    assert_eq!(rows_for(&handle), 0);
    let _ = home;
}
