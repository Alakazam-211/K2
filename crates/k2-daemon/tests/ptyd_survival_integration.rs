//! k2-ptyd T1 survival harness (PRD prd-k2-ptyd-v1 §12 "Daemon survival").
//!
//! The REAL daemon binary, headless (no client, no supervisor), with the
//! switch on (`K2_PTYD=1`), in a scratch HOME. A fake agent
//! (`k2-ptyd-test-agent survive`) prints `seq=<n>` lines, calls
//! `GET /cli/whoami` over its `K2_HOOK_SOCK` every second, records any
//! byte it receives on stdin and any SIGHUP.
//!
//! The daemon is restarted five times mid-output — SIGTERM with no stop
//! intent, `kill -9` (a crash), `POST /cli/daemon/restart`, SIGTERM again,
//! `kill -9` again — and after each one the test asserts (L3, L5):
//! - the SAME agent pid is alive and never got a SIGHUP;
//! - the session is registered again under its SessionId (its screen reads
//!   back through `/cli/terminal/read?id=`);
//! - no `seq` line is missing across the restart (replay from the ring);
//! - zero unsolicited bytes reached the agent's stdin (P75);
//! - the agent's hook token and cell socket still work (a 200 after the
//!   restart, P31, P44).
//!
//! Then `POST /cli/daemon/stop` (L2, P52): the agent and ptyd are gone.
//!
//! Needs the ptyd binaries: `cargo build -p k2-ptyd --bins` (k2-ptyd and
//! k2-ptyd-test-agent next to this test's target dir), or set
//! `K2_PTYD_BIN` / `K2_PTYD_TEST_AGENT`. Missing binaries fail loudly.
//! Never touches the real `~/.k2`, the keychain, a running daemon or its
//! ptyd: everything lives under the scratch HOME and only pids this test
//! started are killed.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// This test (and so the daemon binary next to it, same cargo invocation)
/// was built with `K2_PTYD_EMBED`: the daemon must launch its embedded
/// k2-ptyd from `~/.k2/bin/ptyd/`, and the harness proves it.
const EMBEDDED: bool = option_env!("K2_PTYD_EMBED").is_some();

fn target_dir_bin(name: &str) -> PathBuf {
    // target/<profile>/deps/ptyd_survival_integration-<hash> → target/<profile>/<name>
    let exe = std::env::current_exe().expect("current_exe");
    exe.parent().and_then(|d| d.parent()).map(|d| d.join(name)).expect("target dir")
}

fn bin(env: &str, name: &str) -> PathBuf {
    let p = std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| target_dir_bin(name));
    assert!(p.is_file(), "{name} not found at {} — run `cargo build -p k2-ptyd --bins` first or set {env}", p.display());
    p
}

fn alive(pid: i32) -> bool {
    // SAFETY: existence probe only.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    // kill(0) succeeded: the pid exists (same uid). Only rule out a zombie.
    // ps may show nothing for a live process: k2-ptyd and the daemon are
    // non-dumpable, so with /proc mounted hidepid=invisible (K2 compute's
    // job sandbox) their /proc entries are hidden from non-root.
    let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().expect("ps");
    !String::from_utf8_lossy(&out.stdout).trim_start().starts_with('Z')
}

fn parent_pid(pid: i32) -> i32 {
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or_else(|_| panic!("ppid of {pid}: {out:?}"))
}

fn wait_for<T>(what: &str, within: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(t.elapsed() < within, "timed out after {within:?} waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One HTTP/1.1 request to the daemon; (status, body).
fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => return (0, String::new()),
    };
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
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
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

struct Harness {
    home: PathBuf,
    k2: PathBuf,
    daemon: Option<Child>,
    boots: usize,
    agent_pid: Option<i32>,
    ptyd_pid: Option<i32>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // Fail loudly WITH the evidence: ptyd's log and the daemon logs.
            let tail = |p: PathBuf| {
                let s = std::fs::read_to_string(&p).unwrap_or_else(|e| format!("({e})"));
                let lines: Vec<&str> = s.lines().collect();
                let from = lines.len().saturating_sub(60);
                eprintln!("──── {} (last {} lines) ────\n{}", p.display(), lines.len() - from, lines[from..].join("\n"));
            };
            tail(self.k2.join("logs/ptyd.log"));
            for b in 1..=self.boots {
                tail(self.home.join(format!("daemon-{b}.log")));
            }
        }
        if let Some(c) = self.daemon.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        for p in [self.agent_pid, self.ptyd_pid].into_iter().flatten() {
            // SAFETY: pids this test started (its agent, its ptyd).
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
    }
}

impl Harness {
    fn new() -> Harness {
        for k in std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("K2_") || k.starts_with("K2SO_")).collect::<Vec<_>>() {
            std::env::remove_var(k);
        }
        // Short path: ptyd's control socket and the cell sockets live under it.
        let home = PathBuf::from(format!("/tmp/k2sv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".k2")).unwrap();
        std::fs::create_dir_all(home.join("ws")).unwrap();
        std::fs::create_dir_all(home.join("xdg")).unwrap();
        std::fs::create_dir_all(home.join("tmp")).unwrap();
        let k2 = home.join(".k2");
        Harness { home, k2, daemon: None, boots: 0, agent_pid: None, ptyd_pid: None }
    }

    fn start(&mut self) -> u16 {
        self.boots += 1;
        let log = std::fs::File::create(self.home.join(format!("daemon-{}.log", self.boots))).unwrap();
        let agent = bin("K2_PTYD_TEST_AGENT", "k2-ptyd-test-agent");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_k2-daemon"));
        // A daemon built with K2_PTYD_EMBED (the release Linux and standalone
        // macOS daemons, P84) runs the k2-ptyd it carries: no K2_PTYD_BIN.
        if !EMBEDDED {
            cmd.env("K2_PTYD_BIN", bin("K2_PTYD_BIN", "k2-ptyd"));
        }
        let child = cmd
            .env("HOME", &self.home)
            .env("XDG_RUNTIME_DIR", self.home.join("xdg"))
            .env("TMPDIR", self.home.join("tmp"))
            .env("K2_PTYD", "1")
            .env("K2_TEST_AGENT_SHIM_DIR", agent.parent().unwrap())
            .env("K2_TUNNEL_LEASE", "off")
            .env("K2_CONNECT_BASE", "http://127.0.0.1:1")
            .env("K2_CERT_BROKER_URL", "http://127.0.0.1:1")
            .env("K2_HANDOFF_DROPIN", "off")
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2_LLM_ACCOUNTS_LOOP", "0")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .env_remove("INVOCATION_ID")
            .env_remove("NOTIFY_SOCKET")
            .env_remove("XPC_SERVICE_NAME")
            .env_remove("K2_AIRGAP")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("spawn k2-daemon");
        self.daemon = Some(child);
        let port = wait_for("daemon.port", Duration::from_secs(30), || {
            std::fs::read_to_string(self.k2.join("daemon.port")).ok()?.trim().parse::<u16>().ok()
        });
        wait_for("/boot-status phase=ready", Duration::from_secs(60), || {
            if let Some(c) = self.daemon.as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    panic!("daemon boot {} exited early: {st} (log {})", self.boots, self.home.join(format!("daemon-{}.log", self.boots)).display());
                }
            }
            let (s, b) = http(port, "GET", "/boot-status", "");
            (s == 200 && b.contains("\"ready\"")).then_some(())
        });
        if self.ptyd_pid.is_none() {
            self.ptyd_pid = std::fs::read_to_string(self.k2.join("run/ptyd/lock")).ok().and_then(|s| s.trim().parse().ok());
        }
        port
    }

    fn daemon_pid(&self) -> i32 {
        self.daemon.as_ref().expect("daemon").id() as i32
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.k2.join("daemon.token")).expect("daemon.token").trim().to_string()
    }

    fn wait_daemon_exit(&mut self, within: Duration) {
        let mut c = self.daemon.take().expect("daemon");
        let t = Instant::now();
        loop {
            if c.try_wait().expect("try_wait").is_some() {
                return;
            }
            if t.elapsed() > within {
                let _ = c.kill();
                let _ = c.wait();
                panic!("daemon did not exit within {within:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn seqs_of(lines: &[String]) -> Vec<u64> {
    let mut v = Vec::new();
    for l in lines {
        for tok in l.split_whitespace() {
            if let Some(n) = tok.strip_prefix("seq=").and_then(|n| n.parse().ok()) {
                v.push(n);
            }
        }
    }
    v
}

/// The session's screen + scrollback through the daemon (proves it is
/// registered under its SessionId).
fn read_screen(port: u16, token: &str, sid: &str) -> Vec<String> {
    let (s, body) = http(port, "GET", &format!("/cli/terminal/read?token={token}&id={sid}&lines=3000"), "");
    assert_eq!(s, 200, "terminal read of session {sid}: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("terminal read body {body}: {e}"));
    v["lines"].as_array().expect("lines").iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect()
}

/// `seq` numbers are consecutive in what the Term holds and reach past
/// `past`. Returns the highest.
fn assert_no_gap(port: u16, token: &str, sid: &str, past: u64, what: &str) -> u64 {
    let max = wait_for(&format!("seq past {past} after {what}"), Duration::from_secs(15), || {
        let seqs = seqs_of(&read_screen(port, token, sid));
        let max = *seqs.iter().max()?;
        (max > past + 5).then_some(max)
    });
    let seqs = seqs_of(&read_screen(port, token, sid));
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), seqs.len(), "{what}: a seq line appears twice (replay duplicated bytes)");
    let gaps: Vec<_> = sorted.windows(2).filter(|w| w[1] != w[0] + 1).map(|w| (w[0], w[1])).collect();
    assert!(gaps.is_empty(), "{what}: seq gaps {gaps:?} (bytes lost across the restart)");
    assert!(sorted.first().copied().unwrap_or(u64::MAX) <= past, "{what}: the Term lost the lines before the restart");
    max
}

fn hook_lines(dir: &Path) -> Vec<(u128, String)> {
    std::fs::read_to_string(dir.join("hook.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (t, r) = l.split_once(' ')?;
            Some((t.parse().ok()?, r.to_string()))
        })
        .collect()
}

/// The cell server authenticated the agent's passport and ran the handler:
/// 200, or the handler's own 400 "cannot determine identity" (the scratch
/// workspace isn't a registered project). A refused token is 401/403; a
/// dead socket is `ERR:connect:…`.
fn served(r: &str) -> bool {
    r == "200" || r == "400"
}

/// The workspace the agent runs in (name = its Thread address).
const WS: &str = "survive";

/// Ask the agent to post `text` to its workspace's Thread with its own
/// passport over its cell socket; wait for a 2xx and for the text to read
/// back from the Thread.
fn agent_thread_post(port: u16, token: &str, dir: &Path, text: &str) {
    std::fs::write(dir.join("thread-req"), format!("{WS}\n{text}")).unwrap();
    let status = wait_for(&format!("the agent's Thread post {text}"), Duration::from_secs(15), || {
        let log = std::fs::read_to_string(dir.join("thread.log")).ok()?;
        log.lines().find_map(|l| l.strip_prefix(&format!("{text} ")).map(str::to_string))
    });
    assert!(status.starts_with('2'), "the agent's Thread post {text} returned {status}");
    wait_for(&format!("{text} in the Thread"), Duration::from_secs(10), || {
        let (s, b) = http(port, "GET", &format!("/cli/thread?token={token}&addr={WS}&limit=50"), "");
        (s == 200 && b.contains(text)).then_some(())
    });
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
}

#[derive(Debug, Clone, Copy)]
enum Restart {
    Sigterm,
    Kill9,
    Route,
}

#[test]
fn agents_survive_daemon_restarts_and_end_on_stop() {
    let mut h = Harness::new();
    let port = h.start();
    let token = h.token();
    let dir = h.home.join("agent");
    // Folder name = workspace name = its handle (the Thread address).
    let ws = h.home.join(WS);
    std::fs::create_dir_all(&ws).unwrap();

    // A registered workspace with a file-delivered vault secret, so the
    // agent gets a passport bound to it, a Thread, and a secrets dir.
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&ws.to_string_lossy());
    std::fs::create_dir_all(persona.parent().expect("persona dir")).unwrap();
    std::fs::write(&persona, format!("---\nname: {WS}\n---\n# {WS}\n")).unwrap();
    let (s, b) = http(port, "POST", &format!("/cli/projects/create?token={token}"), &serde_json::json!({"name": WS, "path": ws.to_string_lossy()}).to_string());
    assert_eq!(s, 200, "projects/create: {b}");
    let project_id = serde_json::from_str::<serde_json::Value>(&b).unwrap()["id"].as_str().unwrap_or_else(|| panic!("project id in {b}")).to_string();
    // Its default agent is a custom preset (the fake agent): K2 owns such a
    // workspace's Thread id (GL44), so its Thread works with no model CLI.
    let preset = uuid::Uuid::new_v4().to_string();
    let (s, b) = http(port, "POST", &format!("/cli/presets/create?token={token}"), &serde_json::json!({"id": preset, "label": "survive-agent", "command": "k2-ptyd-test-agent"}).to_string());
    assert_eq!(s, 200, "presets/create: {b}");
    let (s, b) = http(port, "POST", &format!("/cli/projects/update?token={token}"), &serde_json::json!({"id": project_id, "defaultAgent": preset}).to_string());
    assert_eq!(s, 200, "projects/update: {b}");
    let secret = {
        use base64::Engine as _;
        serde_json::json!({"name": "SURVIVE_FILE", "kind": "generic", "scope": "workspace", "workspace": WS, "delivery": "file",
            "hosts": ["api.example.com"], "valueB64": base64::engine::general_purpose::STANDARD.encode("kept-across-restarts")})
    };
    let (s, b) = http(port, "POST", &format!("/cli/vault/set?token={token}"), &secret.to_string());
    assert_eq!(s, 200, "vault/set: {b}");

    // Spawn the fake agent like the renderer opens a tab.
    let body = serde_json::json!({
        "agent_name": "tab-survive",
        "cwd": ws.to_string_lossy(),
        "command": "k2-ptyd-test-agent",
        "args": ["survive", dir.to_string_lossy(), "50"],
        "cols": 100,
        "rows": 30,
    })
    .to_string();
    let (s, resp) = http(port, "POST", &format!("/cli/sessions/v2/spawn?token={token}"), &body);
    assert_eq!(s, 200, "v2 spawn: {resp}");
    let sid = serde_json::from_str::<serde_json::Value>(&resp).unwrap()["sessionId"].as_str().expect("sessionId").to_string();
    let pid: i32 = wait_for("agent pid file", Duration::from_secs(15), || std::fs::read_to_string(dir.join("pid")).ok()?.trim().parse().ok());
    h.agent_pid = Some(pid);
    let ptyd = h.ptyd_pid.expect("ptyd launched (the switch is on)");
    if let Some(embedded) = option_env!("K2_PTYD_EMBED") {
        // P84: the daemon wrote the k2-ptyd it carries to its versioned path
        // (and runs that copy): byte-identical to what the build embedded.
        let want = std::fs::read(embedded).unwrap_or_else(|e| panic!("read {embedded}: {e}"));
        let copies: Vec<PathBuf> = std::fs::read_dir(h.k2.join("bin/ptyd"))
            .expect("~/.k2/bin/ptyd exists")
            .filter_map(|e| e.ok().map(|e| e.path().join("k2-ptyd")))
            .filter(|p| p.is_file())
            .collect();
        assert_eq!(copies.len(), 1, "one versioned k2-ptyd copy: {copies:?}");
        assert!(std::fs::read(&copies[0]).unwrap() == want, "the versioned copy is the embedded k2-ptyd");
    }
    assert_eq!(parent_pid(pid), ptyd, "the agent's parent is k2-ptyd, not the daemon");
    assert_ne!(ptyd, h.daemon_pid());
    wait_for("the agent's first served hook call", Duration::from_secs(15), || hook_lines(&dir).iter().any(|(_, r)| served(r)).then_some(()));
    let mut past = assert_no_gap(port, &token, &sid, 0, "spawn");
    let secrets = PathBuf::from(std::fs::read_to_string(dir.join("secrets_dir")).expect("secrets_dir record"));
    assert!(!secrets.as_os_str().is_empty(), "the agent got no K2_SECRETS_DIR");
    let secret_file = secrets.join("SURVIVE_FILE");
    assert_eq!(std::fs::read_to_string(&secret_file).expect("secret file at spawn"), "kept-across-restarts");
    agent_thread_post(port, &token, &dir, "before-restarts");

    let mut port = port;
    for (i, mode) in [Restart::Sigterm, Restart::Kill9, Restart::Route, Restart::Sigterm, Restart::Kill9].into_iter().enumerate() {
        let what = format!("restart {} ({mode:?})", i + 1);
        match mode {
            // SAFETY: the daemon this test started.
            Restart::Sigterm => unsafe {
                libc::kill(h.daemon_pid(), libc::SIGTERM);
            },
            Restart::Kill9 => unsafe {
                libc::kill(h.daemon_pid(), libc::SIGKILL);
            },
            Restart::Route => {
                let (s, b) = http(port, "POST", &format!("/cli/daemon/restart?token={}", h.token()), "");
                assert_eq!(s, 200, "{what}: {b}");
            }
        }
        h.wait_daemon_exit(Duration::from_secs(20));
        assert!(alive(pid), "{what}: the agent died with the daemon");
        assert!(alive(ptyd), "{what}: ptyd died with the daemon");
        std::thread::sleep(Duration::from_millis(500)); // the agent keeps writing while no daemon is attached
        port = h.start();
        let ready_at = now_ms();
        let token = h.token();
        assert!(alive(pid), "{what}: the agent is gone after the restart");
        assert_eq!(parent_pid(pid), ptyd, "{what}: still the same ptyd's child");
        assert!(!dir.join("sighup").exists(), "{what}: the agent got a SIGHUP");
        past = assert_no_gap(port, &token, &sid, past, &what);
        let stdin = std::fs::read(dir.join("stdin")).unwrap_or_default();
        assert!(stdin.is_empty(), "{what}: unsolicited bytes reached the agent: {stdin:?}");
        wait_for(&format!("{what}: a served hook call after ready"), Duration::from_secs(15), || {
            hook_lines(&dir).iter().any(|(t, r)| *t >= ready_at && served(r)).then_some(())
        });
        let refused: Vec<_> = hook_lines(&dir).into_iter().filter(|(_, r)| r == "401" || r == "403").collect();
        assert!(refused.is_empty(), "{what}: the agent's passport was refused: {refused:?}");
        // The renderer's reattach (empty-command spawn under the same key)
        // finds the adopted session instead of spawning (P43).
        let (s, resp) = http(port, "POST", &format!("/cli/sessions/v2/spawn?token={token}"), &serde_json::json!({"agent_name": "tab-survive", "cwd": ws.to_string_lossy()}).to_string());
        assert_eq!(s, 200, "{what}: reattach: {resp}");
        let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["sessionId"], sid.as_str(), "{what}: reattach returned another session: {resp}");
        assert_eq!(v["reused"], true, "{what}: reattach spawned instead of reusing: {resp}");
        let log = std::fs::read_to_string(h.home.join(format!("daemon-{}.log", h.boots))).unwrap_or_default();
        assert!(log.contains(&format!("adopted session={sid}")), "{what}: the boot log has no adoption line");
        // P36: the vault secrets dir survived (and wasn't swept by the
        // vault's first tick before B1).
        assert_eq!(
            std::fs::read_to_string(&secret_file).unwrap_or_else(|e| panic!("{what}: secret file gone: {e}")),
            "kept-across-restarts"
        );
        // P27: /boot-status says ptyd is on and this boot adopted 1 session.
        let (s, b) = http(port, "GET", &format!("/boot-status?token={token}"), "");
        assert_eq!(s, 200, "{what}: boot-status: {b}");
        let bs: serde_json::Value = serde_json::from_str(&b).unwrap_or_else(|e| panic!("{what}: boot-status JSON {e}: {b}"));
        assert_eq!(bs["ptyd"], "on", "{what}: {b}");
        assert_eq!(bs["adopted"], 1, "{what}: {b}");
        // DA2: the adopted session has an activity row.
        let (s, b) = http(port, "GET", &format!("/cli/activity/snapshot?token={token}"), "");
        assert_eq!(s, 200, "{what}: activity snapshot: {b}");
        assert!(b.contains(&sid), "{what}: no activity row for the adopted session: {b}");
        // The agent's own Thread write (its passport) lands after the restart.
        agent_thread_post(port, &token, &dir, &format!("after-restart-{}", i + 1));
    }

    // P74 (V2): the adopted agent's ancestry is agent → ptyd → init, no
    // daemon pid. A child of it with every K2 variable stripped still
    // isn't a person to the vault socket.
    std::fs::write(dir.join("vault-probe"), "").unwrap();
    let probe = wait_for("the vault person-check probe", Duration::from_secs(15), || {
        std::fs::read_to_string(dir.join("vault-probe.out")).ok().filter(|s| !s.is_empty())
    });
    assert!(
        probe.contains("person_required") && probe.contains("runs inside K2"),
        "P74: an env-stripped process under ptyd must not pass as a person: {probe}"
    );

    // L2: an explicit stop ends the agent (and ptyd, which then holds nothing).
    let (s, b) = http(port, "POST", &format!("/cli/daemon/stop?token={}", h.token()), "");
    assert_eq!(s, 200, "stop: {b}");
    assert!(b.contains("\"stopping\":true"), "{b}");
    h.wait_daemon_exit(Duration::from_secs(20));
    wait_for("the agent to end after a stop", Duration::from_secs(10), || (!alive(pid)).then_some(()));
    wait_for("ptyd to exit after a stop", Duration::from_secs(10), || (!alive(ptyd)).then_some(()));
    assert!(!secrets.exists(), "a stop removes the ended session's secrets dir");
    h.agent_pid = None;
    h.ptyd_pid = None;
    let _ = std::fs::remove_dir_all(&h.home);
}
