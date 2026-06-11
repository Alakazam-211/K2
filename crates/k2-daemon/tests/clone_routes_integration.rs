//! K2 Connect "Clone to" P2 — daemon route integration tests.
//!
//! Drives REAL HTTP through the REAL dispatcher (`test_harness::start`)
//! for `/cli/clone/unpack` + `/cli/clone/bundle`, asserting the full
//! dispatch + isolated `token_ok` gate + handler stack:
//!
//!  - garbage / missing token → 403 (the isolated clone gate)
//!  - owner token + a real bundle → 200, files land at the recomputed
//!    paths, and a project row is registered with the manifest's settings
//!
//! Hermetic: `$HOME` is redirected to a fresh tempdir so the remote-home
//! placement (`~/.claude/projects/<slug>/`) and the in-memory DB never
//! touch the real machine. Mirrors the harness in
//! `auth_routes_integration.rs`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use k2_core::clone;
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-clone-p2";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set read timeout");

    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().ok();

    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((_s, _b, true)) = try_parse(&raw) {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status: {text:?}"));
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

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir -p");
    }
    std::fs::write(path, contents).expect("write file");
}

/// Build a synthetic bundle on disk (source workspace + memory + a live
/// session) with `agent_mode='manager'` settings baked into the manifest.
/// Returns the bundle path. Everything lives under `root`.
fn build_bundle_with_manager_settings(root: &Path) -> PathBuf {
    let project = root.join("source").join("Cloned WS");
    let src_home = root.join("src-home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&src_home).unwrap();

    write(&project.join("README.md"), "# Cloned WS\ndocs\n");
    write(&project.join(".k2so/PROJECT.md"), "# Project\n");

    let canon = std::fs::canonicalize(&project).unwrap();
    let slug = k2_core::chat_history::claude_project_hash(&canon.to_string_lossy());
    let slug_dir = src_home.join(".claude").join("projects").join(&slug);
    write(&slug_dir.join("memory/MEMORY.md"), "## Memory\n");
    write(
        &slug_dir.join("55555555-5555-5555-5555-555555555555.jsonl"),
        "{\"type\":\"live\"}\n",
    );

    let opts = clone::CloneOptions {
        home_override: Some(src_home.clone()),
        ..Default::default()
    };
    let inv = clone::inventory(&canon.to_string_lossy(), opts.clone()).unwrap();
    let settings = clone::WorkspaceSettings {
        agent_mode: "manager".to_string(),
        agent_enabled: true,
        heartbeat_enabled: true,
        name: "Cloned WS".to_string(),
        color: "#abcdef".to_string(),
        worktree_mode: 1,
    };
    let bundle = root.join("bundle.tar.gz");
    clone::build_bundle(
        &inv,
        &opts,
        "2026-06-05T00:00:00Z".to_string(),
        Some(settings),
        &bundle,
    )
    .unwrap();
    bundle
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clone_unpack_rejects_missing_and_garbage_token() {
    let _g = lock();
    let prev = std::env::var_os("HOME");
    let tmp = std::env::temp_dir().join(format!("k2so-clone-it-403-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();

    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let body = r#"{"bundle_path":"/x.tar.gz","dest_parent":"/tmp"}"#;

    // No token → 403.
    let r = http(d.port, "POST", "/cli/clone/unpack", Some(body));
    assert_eq!(r.status, 403, "no token must 403; body={}", r.body);

    // Garbage token → 403.
    let r = http(
        d.port,
        "POST",
        "/cli/clone/unpack?token=not-the-token",
        Some(body),
    );
    assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);

    // Same gate on the bundle route.
    let r = http(
        d.port,
        "POST",
        "/cli/clone/bundle?token=not-the-token",
        Some(r#"{"project_path":"/x"}"#),
    );
    assert_eq!(r.status, 403, "garbage token must 403 on bundle; body={}", r.body);

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clone_unpack_owner_token_extracts_registers_and_configures() {
    let _g = lock();
    let prev = std::env::var_os("HOME");
    let tmp = std::env::temp_dir().join(format!("k2so-clone-it-ok-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    // HOME is the REMOTE home — memory/sessions land under tmp/.claude/projects.
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();

    let bundle = build_bundle_with_manager_settings(&tmp);
    let dest_parent = tmp.join("dest");
    std::fs::create_dir_all(&dest_parent).unwrap();

    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let body = serde_json::json!({
        "bundle_path": bundle.to_string_lossy(),
        "dest_parent": dest_parent.to_string_lossy(),
    })
    .to_string();

    let r = http(
        d.port,
        "POST",
        &format!("/cli/clone/unpack?token={OWNER_TOKEN}"),
        Some(&body),
    );
    assert_eq!(r.status, 200, "owner token unpack must 200; body={}", r.body);

    let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON response");
    let dest_path = v["dest_path"].as_str().expect("dest_path string");
    assert_eq!(
        Path::new(dest_path),
        dest_parent.join("Cloned WS"),
        "dest named after source workspace"
    );

    // Files at dest.
    assert!(Path::new(dest_path).join("README.md").is_file());

    // Memory/session under recomputed slug.
    let remote_slug = k2_core::chat_history::claude_project_hash(dest_path);
    let slug_dir = tmp.join(".claude").join("projects").join(&remote_slug);
    assert!(
        slug_dir.join("memory/MEMORY.md").is_file(),
        "memory under recomputed slug {}",
        slug_dir.display()
    );
    assert!(slug_dir
        .join("55555555-5555-5555-5555-555555555555.jsonl")
        .is_file());

    // Project registered + settings applied.
    let proj = &v["project"];
    assert_eq!(proj["path"].as_str(), Some(dest_path));
    assert_eq!(proj["agentMode"].as_str(), Some("manager"));
    assert_eq!(proj["color"].as_str(), Some("#abcdef"));
    assert_eq!(proj["name"].as_str(), Some("Cloned WS"));
    assert_eq!(proj["heartbeatEnabled"].as_i64(), Some(1));
    assert_eq!(proj["worktreeMode"].as_i64(), Some(1));

    // cleanup the registered row.
    if let Some(id) = proj["id"].as_str() {
        let _ = k2_core::projects_ops::projects_delete(id);
    }

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
