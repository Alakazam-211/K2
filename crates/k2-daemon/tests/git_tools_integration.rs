//! First run without git (0.46.4, Rosson 2026-10-26), headless.
//!
//! `K2_GIT_PROBE_PATH` points the daemon's git probe at stub folders, so
//! this test fakes a machine with no usable git and never runs a real
//! install:
//! - macOS: the stub folder stands in for Apple's `/usr/bin` pair
//!   (`K2_TEST_APPLE_SHIM_DIR`, test builds only): a `git` that leaves a
//!   mark when run and an `xcode-select` whose `-p` fails (no tools) and
//!   whose `--install` only logs its arguments (or, once told, refuses
//!   with "already installed").
//! - Linux: the stub folder is empty (no git at all).
//!
//! 1. `GET /cli/git-tools` says git isn't usable, with how to install;
//!    `?format=text` gives the CLI's lines.
//! 2. Init git on Add workspace adds the workspace anyway, marked
//!    `gitWaiting`; a branch git would refuse is refused up front;
//!    nothing ran Apple's shim; nothing wrote ~/.gitconfig.
//! 3. Install: macOS runs `xcode-select --install` (the stub) and shows
//!    `installing`; a quick refusal is a 409 with its words; owner token
//!    only, POST only. Linux refuses with how to.
//! 4. A spawn while git is missing gets `~/.k2/bin/git-wait` first on PATH.
//! 5. git arrives (a real git linked into the probe path): the next spawn
//!    in one waiting workspace and `POST /cli/git-tools/recheck` for the
//!    other run the waiting steps (identity seed, `git init` on the chosen
//!    branch, the first commit).
//!
//! 0.46.5 (first-run review leftovers): a typo'd path is refused and never
//! created (14); an agent's passport reads the state and rechecks, and its
//! install is a teaching owner_only (13); a spawn that finishes the waiting
//! steps emits `ProjectsChanged` (12).
//!
//! Temp HOME; the agent shim stands in for the real CLI.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_core::git_safe;
use k2_core::git_tools;
use k2_core::session::SessionId;
use k2_daemon::session_events::SessionEvent;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use k2_daemon::{test_harness, v2_session_map};

const OWNER: &str = "git-tools-owner-token";

struct Resp {
    status: u16,
    body: serde_json::Value,
    raw: String,
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, rest)) = text.split_once("\r\n\r\n") {
            let len = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()));
            if len.is_some_and(|n| rest.len() >= n) {
                break;
            }
        }
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("read {method} {path}: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no HTTP response to {method} {path}: {text:?}"));
    let status: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("status code");
    let body = serde_json::from_str(rest).unwrap_or(serde_json::Value::Null);
    Resp { status, body, raw: rest.to_string() }
}

fn wait_for(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out after {secs}s waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A real git from this machine's PATH (the probe's own rules, so on a
/// Mac Apple's shim counts only when the tools are installed).
fn real_git() -> PathBuf {
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").expect("PATH")).collect();
    let apple = git_safe::ApplePair { git: "/usr/bin/git".into(), xcode_select: "/usr/bin/xcode-select".into() };
    let apple = cfg!(target_os = "macos").then_some(&apple);
    match git_safe::probe_in(&dirs, apple) {
        git_safe::GitProbe::Ready { program, .. } => program,
        other => panic!("this test needs git on the test machine; the probe says {other:?}"),
    }
}

fn git_out(git: &Path, dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(git)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_git_and_open(port: u16, path: &Path, branch: &str) -> Resp {
    let body = serde_json::json!({ "path": path, "branch": branch, "seedWiki": false, "seedAgentsMd": false }).to_string();
    http(port, "POST", &format!("/cli/projects/init-git-and-open?token={OWNER}"), Some(&body))
}

fn project_id_for(path: &Path) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT id FROM projects WHERE path = ?1", [path.to_string_lossy().to_string()], |r| r.get(0))
        .unwrap_or_else(|e| panic!("{} is not registered: {e}", path.display()))
}

fn spawn_in(path: &Path, shim: &Path, tag: &str) -> String {
    let pid = project_id_for(path);
    let req = SpawnWorkspaceSessionRequest {
        agent_name: format!("git-tools-{tag}"),
        project_id: Some(pid.clone()),
        cwd: path.to_string_lossy().to_string(),
        command: Some(shim.to_string_lossy().to_string()),
        args: Some(vec![]),
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    spawn_agent_session_v2_blocking(req).unwrap_or_else(|e| panic!("spawn in {}: {e}", path.display()));
    pid
}

/// `ProjectsChanged` events waiting on `rx` (drains it).
fn projects_changed(rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>) -> usize {
    use tokio::sync::broadcast::error::TryRecvError;
    let mut n = 0;
    loop {
        match rx.try_recv() {
            Ok(SessionEvent::ProjectsChanged {}) => n += 1,
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return n,
        }
    }
}

fn stop(key: &str) {
    if let Some(s) = v2_session_map::unregister(key) {
        s.kill();
    }
}

#[test]
fn first_run_without_git_adds_the_workspace_and_finishes_git_later() {
    let t = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
    let _enter = rt.enter();
    let home = t.path().canonicalize().expect("canonical home");
    let mac = cfg!(target_os = "macos");
    let git = real_git();

    // The fake machine: no usable git.
    let usr_bin = home.join("usr-bin");
    let ready_bin = home.join("ready-bin");
    std::fs::create_dir_all(&usr_bin).expect("mkdir usr-bin");
    std::fs::create_dir_all(&ready_bin).expect("mkdir ready-bin");
    let shim_ran = home.join("apple-shim-ran");
    let install_log = home.join("xcode-select-install.log");
    let refuse_install = home.join("xcode-select-refuses");
    if mac {
        k2_core::test_env::write_executable(&usr_bin.join("git"), &format!("#!/bin/sh\n: > '{}'\nexit 1\n", shim_ran.display()));
        k2_core::test_env::write_executable(
            &usr_bin.join("xcode-select"),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = --install ]; then\n  if [ -e '{refuse}' ]; then echo 'xcode-select: error: command line tools are already installed, use \"Software Update\" in System Settings to install updates' >&2; exit 1; fi\n  echo \"$@\" >> '{log}'; exit 0\nfi\necho 'xcode-select: error: unable to get active developer directory' >&2\nexit 2\n",
                refuse = refuse_install.display(),
                log = install_log.display()
            ),
        );
    }
    let _probe = k2_core::test_env::EnvVar::set(
        git_safe::PROBE_PATH_ENV,
        std::env::join_paths([&usr_bin, &ready_bin]).expect("join"),
    );
    // macOS: the stub folder is Apple's /usr/bin pair (test builds only).
    let _apple = k2_core::test_env::EnvVar::set(git_safe::TEST_APPLE_DIR_ENV, &usr_bin);
    assert!(!git_safe::refresh().is_ready(), "the stub machine has no usable git");
    let _agent = k2_core::test_env::AgentShim::with_body("printf '%s' \"$PATH\" > \"$PWD/.k2-test-path\"\nexec sleep 30");
    let claude = _agent.dir().join("claude");

    let daemon = rt.block_on(test_harness::start(OWNER));
    let port = daemon.port;

    // 1. The state.
    let st = http(port, "GET", &format!("/cli/git-tools?token={OWNER}"), None);
    assert_eq!(st.status, 200, "{}", st.raw);
    assert_eq!(st.body["state"], "not_installed", "{}", st.raw);
    assert_eq!(st.body["needs"], if mac { "apple_clt" } else { "git" }, "{}", st.raw);
    assert_eq!(st.body["canInstall"], mac, "{}", st.raw);
    assert!(st.body["howTo"].as_array().is_some_and(|a| !a.is_empty()), "{}", st.raw);
    assert_eq!(st.body["waiting"], serde_json::json!([]), "{}", st.raw);
    assert_eq!(http(port, "GET", "/cli/git-tools", None).status, 403, "no token, no state");
    let tx = http(port, "GET", &format!("/cli/git-tools?token={OWNER}&format=text"), None);
    assert_eq!(tx.status, 200, "{}", tx.raw);
    let first = if mac { "git        : not installed (needs Apple's command line tools)" } else { "git        : not installed (needs git)" };
    assert_eq!(tx.raw.lines().next(), Some(first), "{}", tx.raw);

    // 2. Add workspace with "Initialize Git": added anyway, steps wait.
    let ws_a = home.join("site-a");
    let ws_b = home.join("site-b");
    for ws in [&ws_a, &ws_b] {
        std::fs::create_dir_all(ws).expect("mkdir workspace");
    }
    // A name git would refuse is refused now, not deferred forever.
    let r = init_git_and_open(port, &ws_a, "-x");
    assert_eq!(r.status, 400, "{}", r.raw);
    assert!(r.raw.contains("can't start with -"), "{}", r.raw);
    assert_eq!(git_tools::read_deferred(&ws_a), None, "no marker for a refused name");
    let r = init_git_and_open(port, &ws_a, "  trunk ");
    assert_eq!(r.status, 200, "Add workspace must not fail for want of git: {}", r.raw);
    assert_eq!(r.body["gitWaiting"], git_tools::waiting_mark(), "{}", r.raw);
    let r = init_git_and_open(port, &ws_b, "main");
    assert_eq!(r.status, 200, "{}", r.raw);
    for ws in [&ws_a, &ws_b] {
        assert!(!ws.join(".git").exists(), "no git ran: {}", ws.display());
    }
    assert_eq!(git_tools::read_deferred(&ws_a).expect("marker").branch, "trunk");
    let st = http(port, "GET", &format!("/cli/git-tools?token={OWNER}"), None);
    let waiting: Vec<&str> = st.body["waiting"].as_array().expect("waiting").iter().map(|w| w["path"].as_str().expect("path")).collect();
    assert_eq!(waiting.len(), 2, "{}", st.raw);
    assert!(st.body["waiting"][0]["note"].as_str().is_some_and(|n| n == git_tools::waiting_mark()), "{}", st.raw);
    assert_eq!(st.body["waiting"][0]["lastError"], serde_json::Value::Null, "not tried yet: {}", st.raw);
    assert!(!shim_ran.exists(), "K2 ran Apple's git shim (that opens the install dialog)");
    assert!(!home.join(".gitconfig").exists(), "no identity write while git is missing");
    // 2b. A typo'd path is refused, never created (review item 14).
    let typo = home.join("site-aa").join("nested");
    let r = init_git_and_open(port, &typo, "main");
    assert_eq!(r.status, 400, "{}", r.raw);
    assert!(r.raw.contains("There's no folder at"), "{}", r.raw);
    assert!(!home.join("site-aa").exists(), "the typo'd folder was created");

    // 2c. An agent's passport (review item 13): reads the state, may
    // recheck; install is the owner's (a teaching owner_only).
    let ws_a_id = project_id_for(&ws_a);
    let passport = session_token::mint_session_token(
        &SessionId::new(),
        "pane-git-tools",
        HookPrincipal { workspace_uuid: ws_a_id, agent_address: "git-tools-agent".into() },
        session_token::CredMode::ApiKey,
        session_token::Provider::Anthropic,
    );
    let st = http(port, "GET", &format!("/cli/git-tools?token={passport}"), None);
    assert_eq!(st.status, 200, "an agent reads the state: {}", st.raw);
    assert_eq!(st.body["state"], "not_installed", "{}", st.raw);
    let tx = http(port, "GET", &format!("/cli/git-tools?token={passport}&format=text"), None);
    assert_eq!(tx.status, 200, "{}", tx.raw);
    let r = http(port, "POST", &format!("/cli/git-tools/recheck?token={passport}"), Some("{}"));
    assert_eq!(r.status, 200, "an agent may recheck: {}", r.raw);
    assert_eq!(r.body["finished"], serde_json::json!([]), "git still missing: {}", r.raw);
    assert_eq!(http(port, "GET", &format!("/cli/git-tools/recheck?token={passport}"), None).status, 405, "POST only");
    let r = http(port, "POST", &format!("/cli/git-tools/install?token={passport}"), Some("{}"));
    assert_eq!(r.status, 403, "{}", r.raw);
    assert_eq!(r.body["error"], "owner_only", "{}", r.raw);
    assert!(r.body["hint"].as_str().is_some_and(|h| h.contains("owner")), "{}", r.raw);
    if mac {
        assert!(!install_log.exists(), "an agent's passport ran xcode-select --install");
    }
    let forged = format!("{passport}x");
    assert_eq!(http(port, "GET", &format!("/cli/git-tools?token={forged}"), None).status, 403, "a bad passport");
    assert_eq!(
        http(port, "POST", &format!("/cli/git-tools/install?token={forged}"), Some("{}")).body["error"],
        "invalid or missing token",
        "a bad passport is not told it is an agent"
    );

    // 3. Install.
    assert_eq!(http(port, "GET", &format!("/cli/git-tools/install?token={OWNER}"), None).status, 405, "POST only");
    assert_eq!(http(port, "POST", "/cli/git-tools/install", Some("{}")).status, 403, "owner only");
    let r = http(port, "POST", &format!("/cli/git-tools/install?token={OWNER}"), Some("{}"));
    if mac {
        assert_eq!(r.status, 200, "{}", r.raw);
        assert_eq!(r.body["started"], "started", "{}", r.raw);
        assert_eq!(r.body["state"], "installing", "{}", r.raw);
        assert_eq!(std::fs::read_to_string(&install_log).expect("the stub xcode-select --install ran").trim(), "--install");
        // A quick refusal ("already installed") is a 409 with its words,
        // within the ~2 s watch.
        std::fs::write(&refuse_install, "").expect("arm refusal");
        let t0 = Instant::now();
        let r = http(port, "POST", &format!("/cli/git-tools/install?token={OWNER}"), Some("{}"));
        assert_eq!(r.status, 409, "{}", r.raw);
        assert_eq!(r.body["code"], "install_refused", "{}", r.raw);
        assert!(r.body["error"].as_str().is_some_and(|e| e.contains("already installed")), "{}", r.raw);
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        std::fs::remove_file(&refuse_install).expect("disarm refusal");
    } else {
        assert_eq!(r.status, 409, "{}", r.raw);
        assert_eq!(r.body["code"], "not_macos", "{}", r.raw);
        assert!(r.body["howTo"].as_array().is_some_and(|a| !a.is_empty()), "{}", r.raw);
    }

    // 4. A spawn while git is missing: the git-wait shim first on PATH.
    let mut events = k2_daemon::session_events::subscribe();
    let key_a = spawn_in(&ws_a, &claude, "a");
    assert_eq!(projects_changed(&mut events), 0, "a spawn that finished nothing tells no one");
    let path_file = ws_a.join(".k2-test-path");
    wait_for("the agent's PATH", 15, || path_file.exists() && std::fs::metadata(&path_file).is_ok_and(|m| m.len() > 0));
    let child_path = std::fs::read_to_string(&path_file).expect("read PATH");
    // Staged under $HOME as the daemon sees it (not canonicalized).
    let wait_dir = git_tools::wait_shim_dir_at(t.path());
    assert_eq!(child_path.split(':').next(), Some(wait_dir.to_str().expect("utf8")), "PATH: {child_path}");
    let shim_text = std::fs::read_to_string(wait_dir.join("git")).expect("shim staged");
    assert_eq!(shim_text, git_tools::wait_shim_script(&git_tools::agent_note(), git_safe::apple_pair().as_ref()));
    stop(&key_a);
    assert!(!ws_a.join(".git").exists(), "no git steps while git is missing");

    // 5. git arrives.
    std::os::unix::fs::symlink(&git, ready_bin.join("git")).expect("link a real git");
    let probe = git_safe::refresh();
    assert!(probe.is_ready(), "{probe:?}");
    // 5a. The next spawn in workspace A runs its waiting steps first.
    std::fs::remove_file(&path_file).expect("rm PATH file");
    let _ = projects_changed(&mut events);
    let key_a = spawn_in(&ws_a, &claude, "a2");
    assert!(ws_a.join(".git").is_dir(), "spawn ran the deferred git init");
    // Review item 12: clients hear it at once, not on their next poll.
    assert!(projects_changed(&mut events) >= 1, "the spawn that finished the git steps emits ProjectsChanged");
    assert_eq!(git_out(&git, &ws_a, &["rev-parse", "--abbrev-ref", "HEAD"]), "trunk");
    assert_eq!(git_out(&git, &ws_a, &["log", "--format=%s"]), "Initial commit");
    assert_eq!(git_tools::read_deferred(&ws_a), None, "marker gone");
    wait_for("the agent's PATH (git ready)", 15, || path_file.exists() && std::fs::metadata(&path_file).is_ok_and(|m| m.len() > 0));
    let child_path = std::fs::read_to_string(&path_file).expect("read PATH");
    assert!(!child_path.contains("git-wait"), "no shim once git runs: {child_path}");
    stop(&key_a);
    // 5b. Recheck does the rest.
    let r = http(port, "POST", &format!("/cli/git-tools/recheck?token={OWNER}"), Some("{}"));
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!(r.body["state"], "ready", "{}", r.raw);
    assert_eq!(r.body["finished"], serde_json::json!([ws_b.to_string_lossy()]), "{}", r.raw);
    assert_eq!(r.body["waiting"], serde_json::json!([]), "{}", r.raw);
    assert_eq!(git_out(&git, &ws_b, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(git_out(&git, &ws_b, &["log", "--format=%s"]), "Initial commit");
    assert!(!shim_ran.exists(), "Apple's shim never ran");
    // Install once git runs: nothing to do.
    if mac {
        let r = http(port, "POST", &format!("/cli/git-tools/install?token={OWNER}"), Some("{}"));
        assert_eq!(r.body["started"], "already_ready", "{}", r.raw);
    }
    drop(daemon);
}
