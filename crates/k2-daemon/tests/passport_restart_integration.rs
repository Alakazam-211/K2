//! When the passport store (`~/.k2/hook-sessions.json`) is not what the agents
//! need it to be at a restart. Same-version restarts with a healthy store are
//! `ptyd_survival_integration`; this file is the rest:
//!
//! - the store is lost, wiped or corrupt while agents live on (2026-11-07: a
//!   `cargo test` with the real HOME overwrote it): each surviving session
//!   heals, with no tab restart;
//! - a revoked session never does;
//! - the keep path puts back a store overwritten while the daemon ran;
//! - a store the previous release wrote loads, and its passports authenticate.
//!
//! The REAL daemon binary, headless, `K2_PTYD=1`, in a scratch HOME, no
//! `K2_*` from the caller. Each passport is the one the agent really holds: a
//! fake agent (`k2-ptyd-test-agent probe`) records its env, this test presents
//! that token over the agent's `K2_HOOK_SOCK` and `K2_PORT` on agent verbs, and
//! runs the real `k2` CLI with that env.
//!
//! Needs the ptyd binaries: `cargo build -p k2-ptyd --bins`, or set
//! `K2_PTYD_BIN` / `K2_PTYD_TEST_AGENT`. Missing binaries fail loudly.
// k2-ptyd is Unix-only until its Windows port.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The workspace the agents run in (name = its Thread address).
const WS: &str = "passports";
const REFUSAL: &str = "Invalid or missing auth token";

/// Harness inputs, read once before `Harness::new` clears every `K2_*`.
fn input(env: &str) -> Option<PathBuf> {
    static INPUTS: std::sync::OnceLock<std::collections::HashMap<&'static str, PathBuf>> = std::sync::OnceLock::new();
    INPUTS
        .get_or_init(|| {
            ["K2_PTYD_BIN", "K2_PTYD_TEST_AGENT"]
                .into_iter()
                .filter_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()).map(|v| (k, PathBuf::from(v))))
                .collect()
        })
        .get(env)
        .cloned()
}

fn bin(env: &str, name: &str) -> PathBuf {
    let p = input(env).unwrap_or_else(|| {
        // target/<profile>/deps/<test>-<hash> → target/<profile>/<name>
        let exe = std::env::current_exe().expect("current_exe");
        exe.parent().and_then(|d| d.parent()).map(|d| d.join(name)).expect("target dir")
    });
    assert!(p.is_file(), "{name} not found at {} — run `cargo build -p k2-ptyd --bins` first or set {env}", p.display());
    p
}

fn alive(pid: i32) -> bool {
    // SAFETY: existence probe only.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    !is_zombie(pid)
}

/// macOS: libproc, not `ps` (setuid, so it can't run under a sandbox).
#[cfg(target_os = "macos")]
fn is_zombie(pid: i32) -> bool {
    match k2_core::mac_proc::bsd_info(pid) {
        Some(info) => info.zombie,
        None => {
            // Reaped since the kill(0) above. Anything else is a failed read.
            // SAFETY: existence probe only.
            assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "libproc can't read live pid {pid}");
            true
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn is_zombie(pid: i32) -> bool {
    let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).trim_start().starts_with('Z')
}

/// macOS: libproc, not `ps` (setuid, so it can't run under a sandbox).
#[cfg(target_os = "macos")]
fn parent_pid(pid: i32) -> i32 {
    k2_core::mac_proc::bsd_info(pid).unwrap_or_else(|| panic!("ppid of {pid}: no bsd info")).ppid
}

#[cfg(not(target_os = "macos"))]
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

/// One HTTP/1.1 exchange over any stream: (status, body).
fn exchange(s: &mut (impl Read + Write), head: &str, body: &str) -> (u16, String) {
    write!(s, "{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("write request");
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

/// The owner's call (`?token=` in the path) to the daemon's loopback port.
fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return (0, String::new());
    };
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    exchange(&mut s, &format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n"), body)
}

/// A passport call over loopback TCP, the way `k2` makes it.
fn over_tcp(port: u16, token: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap_or_else(|e| panic!("TCP connect to the agent's K2_PORT {port}: {e}"));
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    exchange(&mut s, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n"), "")
}

/// A passport call over a per-cell socket, the way `k2` makes it.
fn over_cell(sock: &Path, token: &str, path: &str) -> (u16, String) {
    let mut s = UnixStream::connect(sock).unwrap_or_else(|e| panic!("connect to the cell socket {}: {e}", sock.display()));
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    exchange(&mut s, &format!("GET {path} HTTP/1.1\r\nHost: k2\r\nAuthorization: Bearer {token}\r\n"), "")
}

/// One agent session and the passport it really holds (from its own env).
#[derive(Clone, Debug)]
struct Agent {
    key: String,
    sid: String,
    pid: i32,
    token: String,
    sock: PathBuf,
    port: u16,
    secrets: PathBuf,
    /// The agent's whole environment, as its process holds it.
    env: Vec<(String, String)>,
}

impl Agent {
    /// The file a re-minted passport lands in (never there otherwise).
    fn passport_file(&self) -> PathBuf {
        self.secrets.join(".k2-passport")
    }

    /// Run the real `k2` CLI as this agent: its env, exactly.
    fn k2(&self, args: &[&str]) -> (i32, String, String) {
        let cli = concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli/k2");
        let out = Command::new(k2_core::test_env::bash_program())
            .arg(cli)
            .args(args)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|e| panic!("run k2 {args:?}: {e}"));
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
    }

    /// The same agent presenting `token`.
    fn holding(&self, token: &str) -> Agent {
        Agent { token: token.to_string(), ..self.clone() }
    }

    /// The passport authenticates on agent verbs over BOTH transports.
    /// (That it is this session's: `assert_refused` on a forged secret and on
    /// another cell's socket.)
    fn assert_works(&self, what: &str) {
        let thread = format!("/cli/thread?addr={WS}&limit=1");
        for (via, (s, b)) in [("cell socket", over_cell(&self.sock, &self.token, "/cli/whoami")), ("TCP", over_tcp(self.port, &self.token, "/cli/whoami"))] {
            // A tab running the fake agent can't be classified as canonical
            // or sidecar, so whoami answers 400 past the passport check. Only
            // a refused passport is 401/403.
            let served = s == 200 || (s == 400 && b.contains("cannot determine identity"));
            assert!(served, "{what}: {} passport on /cli/whoami over {via}: {s} {b}", self.key);
        }
        for (via, (s, b)) in [("cell socket", over_cell(&self.sock, &self.token, &thread)), ("TCP", over_tcp(self.port, &self.token, &thread))] {
            assert_eq!(s, 200, "{what}: {} passport on /cli/thread over {via}: {b}", self.key);
        }
    }

    /// The passport is refused over both transports, with the refusal agents
    /// saw (the CLI keys on this text).
    fn assert_refused(&self, what: &str) {
        assert_refused(self.port, Some(&self.sock), &self.token, &format!("{what}: {}", self.key));
    }

    /// The same token with another secret: proves the 200s above came from
    /// the digest check, not from anything looser.
    fn forged(&self) -> String {
        format!("{}.{}", self.sid, "0".repeat(64))
    }
}

fn assert_refused(port: u16, sock: Option<&Path>, token: &str, what: &str) {
    let (s, b) = over_tcp(port, token, "/cli/whoami");
    assert_eq!((s, b.contains(REFUSAL)), (403, true), "{what}: over TCP should be refused: {s} {b}");
    if let Some(sock) = sock {
        let (s, b) = over_cell(sock, token, "/cli/whoami");
        assert_eq!((s, b.contains(REFUSAL)), (403, true), "{what}: over the cell socket should be refused: {s} {b}");
    }
}

struct Harness {
    home: PathBuf,
    k2: PathBuf,
    ws: PathBuf,
    daemon: Option<Child>,
    boots: usize,
    agents: Vec<i32>,
    ptyd_pid: Option<i32>,
    port: u16,
}

impl Drop for Harness {
    fn drop(&mut self) {
        if std::thread::panicking() {
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
            eprintln!("──── hook-sessions.json ────\n{}", std::fs::read_to_string(self.k2.join("hook-sessions.json")).unwrap_or_else(|e| format!("({e})")));
        }
        if let Some(c) = self.daemon.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        for p in self.agents.iter().copied().chain(self.ptyd_pid) {
            // SAFETY: pids this test started (its agents, its ptyd).
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }
}

impl Harness {
    fn new(tag: &str) -> Harness {
        let _ = input("K2_PTYD_BIN"); // capture the inputs before the clear
        for k in std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("K2_") || k.starts_with("K2SO_")).collect::<Vec<_>>() {
            std::env::remove_var(k);
        }
        // Short path: ptyd's control socket and the cell sockets live under it.
        let home = k2_core::test_env::short_tmp(format!("k2pp{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for d in [".k2", "xdg", "tmp", "probes", WS] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        let k2 = home.join(".k2");
        let ws = home.join(WS);
        Harness { home, k2, ws, daemon: None, boots: 0, agents: Vec::new(), ptyd_pid: None, port: 0 }
    }

    fn start(&mut self) {
        self.boots += 1;
        let log = std::fs::File::create(self.home.join(format!("daemon-{}.log", self.boots))).unwrap();
        let agent = bin("K2_PTYD_TEST_AGENT", "k2-ptyd-test-agent");
        let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("K2_PTYD_BIN", bin("K2_PTYD_BIN", "k2-ptyd"))
            .env("K2_PTYD", "1")
            .env("HOME", &self.home)
            .env("XDG_RUNTIME_DIR", self.home.join("xdg"))
            .env("TMPDIR", self.home.join("tmp"))
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
        let port_file = self.k2.join("daemon.port");
        let port = wait_for("daemon.port", Duration::from_secs(30), || std::fs::read_to_string(&port_file).ok()?.trim().parse::<u16>().ok());
        self.port = port;
        wait_for("/boot-status phase=ready", Duration::from_secs(60), || {
            if let Some(c) = self.daemon.as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    panic!("daemon boot {} exited early: {st}", self.boots);
                }
            }
            let (s, b) = http(port, "GET", "/boot-status", "");
            (s == 200 && b.contains("\"ready\"")).then_some(())
        });
        if self.ptyd_pid.is_none() {
            self.ptyd_pid = std::fs::read_to_string(self.k2.join("run/ptyd/lock")).ok().and_then(|s| s.trim().parse().ok());
        }
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.k2.join("daemon.token")).expect("daemon.token").trim().to_string()
    }

    fn log(&self, boot: usize) -> String {
        std::fs::read_to_string(self.home.join(format!("daemon-{boot}.log"))).unwrap_or_default()
    }

    fn sigterm(&mut self) {
        // SAFETY: the daemon this test started.
        unsafe { libc::kill(self.daemon.as_ref().expect("daemon").id() as i32, libc::SIGTERM) };
        self.wait_exit();
    }

    fn kill9(&mut self) {
        // SAFETY: the daemon this test started.
        unsafe { libc::kill(self.daemon.as_ref().expect("daemon").id() as i32, libc::SIGKILL) };
        self.wait_exit();
    }

    fn wait_exit(&mut self) {
        let mut c = self.daemon.take().expect("daemon");
        let t = Instant::now();
        loop {
            if c.try_wait().expect("try_wait").is_some() {
                return;
            }
            if t.elapsed() > Duration::from_secs(30) {
                let _ = c.kill();
                let _ = c.wait();
                panic!("the daemon did not exit within 30s");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A registered workspace whose default agent is the fake one, so its
    /// agents get a passport bound to it and a Thread with no model CLI.
    /// Returns the project id.
    fn register_workspace(&self) -> String {
        let token = self.token();
        let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&self.ws.to_string_lossy());
        std::fs::create_dir_all(persona.parent().expect("persona dir")).unwrap();
        std::fs::write(&persona, format!("---\nname: {WS}\n---\n# {WS}\n")).unwrap();
        let (s, b) = http(self.port, "POST", &format!("/cli/projects/create?token={token}"), &json!({"name": WS, "path": self.ws.to_string_lossy()}).to_string());
        assert_eq!(s, 200, "projects/create: {b}");
        let project_id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap_or_else(|| panic!("project id in {b}")).to_string();
        let preset = uuid::Uuid::new_v4().to_string();
        let (s, b) = http(self.port, "POST", &format!("/cli/presets/create?token={token}"), &json!({"id": preset, "label": "passport-agent", "command": "k2-ptyd-test-agent"}).to_string());
        assert_eq!(s, 200, "presets/create: {b}");
        let (s, b) = http(self.port, "POST", &format!("/cli/projects/update?token={token}"), &json!({"id": project_id, "defaultAgent": preset}).to_string());
        assert_eq!(s, 200, "projects/update: {b}");
        project_id
    }

    /// Spawn the fake agent like the renderer opens a tab; read back the
    /// passport from the agent's own env.
    fn spawn(&mut self, key: &str) -> Agent {
        let probe = self.home.join("probes").join(format!("{key}.json"));
        let body = json!({
            "agent_name": key,
            "cwd": self.ws.to_string_lossy(),
            "command": "k2-ptyd-test-agent",
            "args": ["probe", probe.to_string_lossy()],
            "cols": 100,
            "rows": 30,
        })
        .to_string();
        let (s, resp) = http(self.port, "POST", &format!("/cli/sessions/v2/spawn?token={}", self.token()), &body);
        assert_eq!(s, 200, "v2 spawn {key}: {resp}");
        let sid = serde_json::from_str::<Value>(&resp).unwrap()["sessionId"].as_str().expect("sessionId").to_string();
        let v: Value = wait_for(&format!("{key}'s env probe"), Duration::from_secs(15), || serde_json::from_str(&std::fs::read_to_string(&probe).ok()?).ok());
        let env_of = |k: &str| -> String {
            v["env"]
                .as_array()
                .expect("env pairs")
                .iter()
                .find(|p| p[0] == k)
                .and_then(|p| p[1].as_str())
                .unwrap_or_else(|| panic!("{key}: no {k} in the agent's env"))
                .to_string()
        };
        let agent = Agent {
            key: key.to_string(),
            sid: sid.clone(),
            pid: v["pid"].as_i64().expect("pid") as i32,
            token: env_of("K2_HOOK_TOKEN"),
            sock: PathBuf::from(env_of("K2_HOOK_SOCK")),
            port: env_of("K2_PORT").parse().expect("K2_PORT"),
            secrets: PathBuf::from(env_of("K2_SECRETS_DIR")),
            env: v["env"]
                .as_array()
                .expect("env pairs")
                .iter()
                .map(|p| (p[0].as_str().expect("name").to_string(), p[1].as_str().expect("value").to_string()))
                .collect(),
        };
        assert!(agent.secrets.is_dir(), "{key}: no secrets dir at {}", agent.secrets.display());
        assert!(!agent.passport_file().exists(), "{key}: a fresh session has no re-minted passport");
        self.agents.push(agent.pid);
        assert!(agent.token.starts_with(&format!("{sid}.")), "{key}: K2_HOOK_TOKEN is not this session's passport");
        assert_ne!(agent.token, self.token(), "{key}: the agent holds the owner token");
        let ptyd = self.ptyd_pid.or_else(|| std::fs::read_to_string(self.k2.join("run/ptyd/lock")).ok().and_then(|s| s.trim().parse().ok())).expect("ptyd launched (the switch is on)");
        self.ptyd_pid = Some(ptyd);
        assert_eq!(parent_pid(agent.pid), ptyd, "{key}: the agent's parent is k2-ptyd, not the daemon");
        wait_for(&format!("{key}'s cell socket"), Duration::from_secs(10), || agent.sock.exists().then_some(()));
        agent
    }

    fn store(&self) -> Value {
        let raw = std::fs::read_to_string(self.k2.join("hook-sessions.json")).expect("hook-sessions.json");
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("hook-sessions.json is not JSON ({e}): {raw}"))
    }

    fn boot_status(&self) -> Value {
        let (s, b) = http(self.port, "GET", &format!("/boot-status?token={}", self.token()), "");
        assert_eq!(s, 200, "boot-status: {b}");
        serde_json::from_str(&b).unwrap_or_else(|e| panic!("boot-status JSON {e}: {b}"))
    }

    /// A boot adopted exactly `agents`, under their own ids.
    fn assert_adopted(&self, agents: &[&Agent], what: &str) {
        let bs = self.boot_status();
        assert_eq!(bs["ptyd"], "on", "{what}: {bs}");
        assert_eq!(bs["adopted"], agents.len(), "{what}: {bs}");
        let log = self.log(self.boots);
        for a in agents {
            assert!(log.contains(&format!("adopted session={} ", a.sid)), "{what}: the boot log has no adoption line for {}", a.key);
            assert!(alive(a.pid), "{what}: {} died", a.key);
            assert_eq!(a.port, self.port, "{what}: the daemon moved off the K2_PORT {} holds", a.key);
        }
    }
}

/// Every record and epoch in `before` is in `after`, unchanged (it may hold
/// more), and the kill switch didn't move.
fn assert_store_kept(before: &Value, after: &Value, what: &str) {
    for (sid, rec) in before["records"].as_object().expect("records") {
        assert_eq!(&after["records"][sid], rec, "{what}: the record of {sid} changed");
    }
    for (sid, e) in before["session_epochs"].as_object().expect("session_epochs") {
        assert_eq!(&after["session_epochs"][sid], e, "{what}: the epoch of {sid} changed");
    }
    assert_eq!(after["global_epoch"], before["global_epoch"], "{what}: global_epoch changed");
    assert_eq!(after["version"], before["version"], "{what}: store version changed");
}

const RESTART_LINE: &str = "isn't valid on this daemon";

/// A re-minted passport in `agent`'s own folder: 0600, this session's, not the
/// one its env holds. Returns the token.
fn healed_token(agent: &Agent, what: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let f = agent.passport_file();
    let m = std::fs::symlink_metadata(&f).unwrap_or_else(|e| panic!("{what}: no re-minted passport for {}: {e}", agent.key));
    assert!(m.is_file(), "{what}: {} is not a plain file", f.display());
    assert_eq!(m.permissions().mode() & 0o777, 0o600, "{what}: {} mode", f.display());
    let t = std::fs::read_to_string(&f).unwrap().trim().to_string();
    assert!(t.starts_with(&format!("{}.", agent.sid)), "{what}: {}'s folder holds another session's passport", agent.key);
    assert_ne!(t, agent.token, "{what}: the same passport came back");
    t
}

/// The secret half of a token never reaches a daemon log.
fn assert_no_secret_in_logs(h: &Harness, tokens: &[&str], what: &str) {
    for b in 1..=h.boots {
        let log = h.log(b);
        for t in tokens {
            let secret = t.split_once('.').expect("token shape").1;
            assert!(!log.contains(secret), "{what}: daemon-{b}.log holds a passport's secret");
        }
    }
}

/// The agent's own K2 calls work, through the real CLI, with its env as it is.
fn assert_cli_works(h: &Harness, agent: &Agent, text: &str, what: &str) {
    let (rc, out, err) = agent.k2(&["thread", WS, text]);
    assert_eq!(rc, 0, "{what}: k2 thread as {}: {out} {err}", agent.key);
    assert!(!err.contains(RESTART_LINE), "{what}: told to restart a working tab: {err}");
    let (_, _, err) = agent.k2(&["whoami", "--json"]);
    assert!(!err.contains(RESTART_LINE) && !err.contains(REFUSAL), "{what}: k2 whoami as {}: {err}", agent.key);
    let (s, b) = http(h.port, "GET", &format!("/cli/thread?token={}&addr={WS}&limit=50", h.token()), "");
    assert_eq!(s, 200, "{what}: read the Thread: {b}");
    assert!(b.contains(text), "{what}: {text} did not land in the Thread: {b}");
}

/// The store lost between two daemons while the agents live on, in every way
/// it can be lost. Their env passports are dead; each gets a new one for the
/// same session and principal, in its own folder; the agent's next K2 call
/// works over both transports with no tab restart, and the logs say so.
#[test]
fn a_store_lost_between_daemons_heals_the_surviving_sessions() {
    let mut h = Harness::new("b");
    h.start();
    h.register_workspace();
    let a = h.spawn("tab-a");
    let b = h.spawn("tab-b");
    a.assert_works("before the restart");
    b.assert_works("before the restart");
    let spawned = h.store();
    let store = h.k2.join("hook-sessions.json");

    // ── Wiped: the file a stray test run wrote (the 2026-11-07 shape). ──
    h.sigterm();
    let empty = json!({"version": 1, "records": {}, "session_epochs": {}, "global_epoch": 0});
    std::fs::write(&store, serde_json::to_string_pretty(&empty).unwrap()).unwrap();
    h.start();
    h.assert_adopted(&[&a, &b], "wiped store");
    a.assert_refused("wiped store: the env's passport");
    b.assert_refused("wiped store: the env's passport");
    let (ta, tb) = (healed_token(&a, "wiped store"), healed_token(&b, "wiped store"));
    let (ha, hb) = (a.holding(&ta), b.holding(&tb));
    ha.assert_works("wiped store: re-minted");
    hb.assert_works("wiped store: re-minted");
    let (s, body) = over_cell(&a.sock, &tb, "/cli/whoami");
    assert_eq!(s, 403, "wiped store: b's new passport on a's socket: {body}");
    assert_cli_works(&h, &a, "healed-a-wiped", "wiped store");
    assert_cli_works(&h, &b, "healed-b-wiped", "wiped store");

    // Same session, same principal, same pane as its spawn gave it.
    let healed = h.store();
    for agent in [&a, &b] {
        let (old, new) = (&spawned["records"][&agent.sid], &healed["records"][&agent.sid]);
        assert_eq!(new["principal"], old["principal"], "wiped store: {}'s principal", agent.key);
        assert_eq!(new["pane_id"], old["pane_id"], "wiped store: {}'s pane", agent.key);
        assert_eq!(new["session_id"], old["session_id"], "wiped store: {}'s session", agent.key);
        assert_ne!(new["token_digest"], old["token_digest"], "wiped store: {}'s digest", agent.key);
    }

    // It said so, by id, and carries it in /boot-status.
    let log = h.log(h.boots);
    let line = log.lines().find(|l| l.contains("ERROR adoption found 2 live session(s) with no passport")).unwrap_or_else(|| panic!("no loud line:\n{log}"));
    assert!(line.contains(&a.sid) && line.contains(&b.sid), "the loud line names the sessions: {line}");
    let bs = h.boot_status();
    let reminted = bs["passportsReminted"].to_string();
    assert!(reminted.contains(&a.sid) && reminted.contains(&b.sid), "boot-status: {bs}");
    assert_no_secret_in_logs(&h, &[&ta, &tb, &a.token, &b.token], "wiped store");

    // ── A plain restart afterwards: nothing to heal, nothing changes. ──
    h.sigterm();
    h.start();
    h.assert_adopted(&[&a, &b], "restart after healing");
    assert_eq!(healed_token(&a, "restart after healing"), ta, "a valid passport was minted again");
    ha.assert_works("restart after healing");
    assert_cli_works(&h, &a, "healed-a-again", "restart after healing");
    assert!(!h.log(h.boots).contains("ERROR adoption found"), "a healthy store raised an alarm");
    assert!(h.boot_status().get("passportsReminted").is_none(), "boot-status: nothing was re-minted");
    assert_store_kept(&healed, &h.store(), "restart after healing");

    // ── Deleted, and a crash: the old re-minted one dies, a newer one works. ──
    h.kill9();
    std::fs::remove_file(&store).unwrap();
    h.start();
    h.assert_adopted(&[&a, &b], "deleted store");
    let ta2 = healed_token(&a, "deleted store");
    assert_ne!(ta2, ta, "deleted store: not re-minted");
    ha.assert_refused("deleted store: the previous re-mint");
    a.holding(&ta2).assert_works("deleted store: re-minted");
    assert_cli_works(&h, &a, "healed-a-deleted", "deleted store");

    // ── Corrupt: fails closed, says so loudly, heals the same. ──
    h.kill9();
    std::fs::write(&store, "{ not json").unwrap();
    h.start();
    h.assert_adopted(&[&a, &b], "corrupt store");
    let log = h.log(h.boots);
    assert!(log.contains("ERROR passport store") && log.contains("failing closed"), "no loud store line:\n{log}");
    assert!(h.boot_status()["passportStoreTrouble"].as_str().is_some_and(|t| t.contains("failing closed")), "boot-status: {}", h.boot_status());
    a.holding(&healed_token(&a, "corrupt store")).assert_works("corrupt store: re-minted");
    assert_cli_works(&h, &b, "healed-b-corrupt", "corrupt store");

    // A session spawned on the healed store is an ordinary one.
    let c = h.spawn("tab-c");
    c.assert_works("spawned after healing");
    assert_cli_works(&h, &c, "plain-c", "spawned after healing");
}

/// Revoked means revoked: a live session whose epoch moved, or whose record
/// predates the kill switch, gets no new passport, and is told to restart.
#[test]
fn a_revoked_live_session_is_not_healed() {
    let mut h = Harness::new("r");
    h.start();
    h.register_workspace();
    let by_epoch = h.spawn("tab-a");
    let by_kill_switch = h.spawn("tab-b");
    h.sigterm();

    // Trip both in the store, while no daemon runs.
    let path = h.k2.join("hook-sessions.json");
    let mut store = h.store();
    store["records"].as_object_mut().expect("records").remove(&by_epoch.sid);
    store["session_epochs"][&by_epoch.sid] = json!(1);
    store["global_epoch"] = json!(1);
    store["records"][&by_kill_switch.sid]["global_epoch"] = json!(0);
    std::fs::write(&path, serde_json::to_string_pretty(&store).unwrap()).unwrap();

    h.start();
    h.assert_adopted(&[&by_epoch, &by_kill_switch], "revoked");
    for agent in [&by_epoch, &by_kill_switch] {
        agent.assert_refused("revoked");
        assert!(!agent.passport_file().exists(), "revoked: {} got a new passport", agent.key);
        // An agent verb (whoami may go out on the owner token the HOME holds).
        let (rc, out, err) = agent.k2(&["thread", WS, "from a revoked session"]);
        assert_ne!(rc, 0, "revoked: k2 thread as {} worked: {out}", agent.key);
        assert_eq!(err.matches(RESTART_LINE).count(), 1, "revoked: one line says restart this tab: {err}");
    }
    let log = h.log(h.boots);
    let line = log.lines().find(|l| l.contains("ERROR adoption found 2 live session(s) with no usable passport")).unwrap_or_else(|| panic!("no loud line:\n{log}"));
    assert!(line.contains(&by_epoch.sid) && line.contains(&by_kill_switch.sid), "the loud line names the sessions: {line}");
    assert!(h.boot_status()["passportsUnusable"].to_string().contains(&by_epoch.sid));
    let after = h.store();
    assert_eq!(after["session_epochs"][&by_epoch.sid], 1, "{after}");
    assert_eq!(after["global_epoch"], 1, "{after}");
    assert!(after["records"][&by_epoch.sid].is_null(), "{after}");

    // Not on a later boot either.
    h.sigterm();
    h.start();
    assert!(!by_epoch.passport_file().exists() && !by_kill_switch.passport_file().exists(), "a later boot re-minted");
    by_epoch.assert_refused("revoked, later boot");

    // A new session is unaffected.
    let c = h.spawn("tab-c");
    c.assert_works("spawned after the revokes");
}

/// A daemon that keeps its agents writes its own copy out as it goes: a store
/// overwritten while it ran is put back, and nothing needs healing.
#[test]
fn the_keep_path_restores_a_store_overwritten_while_the_daemon_ran() {
    let mut h = Harness::new("k");
    h.start();
    h.register_workspace();
    let a = h.spawn("tab-a");
    a.assert_works("before the restart");
    let before = h.store();
    let empty = json!({"version": 1, "records": {}, "session_epochs": {}, "global_epoch": 0});
    std::fs::write(h.k2.join("hook-sessions.json"), serde_json::to_string_pretty(&empty).unwrap()).unwrap();
    assert!(h.store()["records"][&a.sid].is_null(), "the store is clobbered");
    h.sigterm();
    assert_store_kept(&before, &h.store(), "after the keep exit");
    h.start();
    h.assert_adopted(&[&a], "after the keep exit");
    a.assert_works("after the keep exit");
    assert_refused(a.port, Some(&a.sock), &a.forged(), "after the keep exit: a forged secret");
    assert!(!a.passport_file().exists(), "nothing needed healing");
    assert!(!h.log(h.boots).contains("ERROR adoption found"), "nothing to alarm about");

    // A session spawned after the restart leaves the kept records alone.
    let b = h.spawn("tab-b");
    b.assert_works("spawned after the restart");
    assert_store_kept(&before, &h.store(), "after a mint");
    a.assert_works("after a mint");
    let (s, body) = over_cell(&a.sock, &b.token, "/cli/whoami");
    assert_eq!(s, 403, "b's passport on a's socket: {body}");
}

/// `~/.k2/hook-sessions.json` exactly as the previous releases wrote it
/// (`STORE_VERSION` 1, `SessionTokenRegistry` / `TokenRecord` field for field:
/// the same in v0.46.8 and v0.46.9, `git show <tag>:crates/k2-daemon/src/session_token.rs`).
/// When the format changes, this stays the old shape: that is the test.
/// `{PROJECT}` is the workspace the test registers. Secrets: `LIVE_SECRET`
/// hashes to the digest of the live record, `OTHER_SECRET` to the others.
const STORE_PREVIOUS_RELEASE: &str = r#"{
  "version": 1,
  "records": {
    "11111111-1111-4111-8111-111111111111": {
      "version": 1,
      "session_id": "11111111-1111-4111-8111-111111111111",
      "pane_id": "tab-old-live",
      "principal": {
        "workspace_uuid": "{PROJECT}",
        "agent_address": "passports"
      },
      "token_digest": "a8ae6e6ee929abea3afcfc5258c8ccd6f85273e0d4626d26c7279f3250f77c8e",
      "created_at": "2026-11-04T20:11:09.123456Z",
      "expires_at": "2126-10-11T20:11:09.123456Z",
      "session_epoch": 2,
      "global_epoch": 3,
      "cred_mode": "api_key",
      "provider": "anthropic"
    },
    "22222222-2222-4222-8222-222222222222": {
      "version": 1,
      "session_id": "22222222-2222-4222-8222-222222222222",
      "pane_id": "tab-old-revoked-epoch",
      "principal": {
        "workspace_uuid": "{PROJECT}",
        "agent_address": "passports"
      },
      "token_digest": "7b9d07f2404b102b3c62fede026097c5ab81668f18414abd8ea560cecb008006",
      "created_at": "2026-11-04T20:11:09.123456Z",
      "expires_at": "2126-10-11T20:11:09.123456Z",
      "session_epoch": 0,
      "global_epoch": 3,
      "cred_mode": "api_key",
      "provider": "anthropic"
    },
    "33333333-3333-4333-8333-333333333333": {
      "version": 1,
      "session_id": "33333333-3333-4333-8333-333333333333",
      "pane_id": "tab-old-before-kill-switch",
      "principal": {
        "workspace_uuid": "{PROJECT}",
        "agent_address": "passports"
      },
      "token_digest": "7b9d07f2404b102b3c62fede026097c5ab81668f18414abd8ea560cecb008006",
      "created_at": "2026-11-04T20:11:09.123456Z",
      "expires_at": "2126-10-11T20:11:09.123456Z",
      "session_epoch": 0,
      "global_epoch": 2,
      "cred_mode": "api_key",
      "provider": "anthropic"
    }
  },
  "session_epochs": {
    "11111111-1111-4111-8111-111111111111": 2,
    "22222222-2222-4222-8222-222222222222": 1,
    "44444444-4444-4444-8444-444444444444": 7
  },
  "global_epoch": 3
}"#;
const LIVE_SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_SECRET: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// An upgrade: the store the previous release wrote loads with its records,
/// epochs and kill-switch state, and the same passports get the same answers.
#[test]
fn a_store_written_by_the_previous_release_keeps_its_passports() {
    let live = format!("11111111-1111-4111-8111-111111111111.{LIVE_SECRET}");
    let forged = format!("11111111-1111-4111-8111-111111111111.{OTHER_SECRET}");
    let revoked = format!("22222222-2222-4222-8222-222222222222.{OTHER_SECRET}");
    let before_kill_switch = format!("33333333-3333-4333-8333-333333333333.{OTHER_SECRET}");
    let thread = format!("/cli/thread?addr={WS}&limit=1");

    let mut h = Harness::new("c");
    h.start();
    let project = h.register_workspace();
    h.sigterm();
    let fixture = STORE_PREVIOUS_RELEASE.replace("{PROJECT}", &project);
    std::fs::write(h.k2.join("hook-sessions.json"), &fixture).unwrap();
    let fixture: Value = serde_json::from_str(&fixture).unwrap();

    h.start();
    let check = |h: &Harness, what: &str| {
        let (s, b) = over_tcp(h.port, &live, &thread);
        assert_eq!(s, 200, "{what}: the live old passport: {b}");
        for (name, token) in [("forged", &forged), ("revoked", &revoked), ("pre-kill-switch", &before_kill_switch)] {
            assert_refused(h.port, None, token, &format!("{what}: the {name} old passport"));
        }
    };
    check(&h, "first boot on the old store");

    // A mint makes this daemon write the store back: nothing old is lost,
    // and every field the old store has is still written under its name.
    let a = h.spawn("tab-a");
    a.assert_works("spawned on the old store");
    let after = h.store();
    assert_store_kept(&fixture, &after, "after a mint on the old store");
    let keys = |v: &Value| v.as_object().expect("object").keys().cloned().collect::<BTreeSet<_>>();
    let old_record = &fixture["records"]["11111111-1111-4111-8111-111111111111"];
    let new_record = &after["records"][&a.sid];
    let missing: Vec<_> = keys(old_record).difference(&keys(new_record)).cloned().collect();
    assert!(missing.is_empty(), "the store no longer writes {missing:?}, so a rollback can't read it: {new_record}");
    assert_eq!(new_record["version"], fixture["version"], "the record version moved");
    assert_eq!(new_record["global_epoch"], fixture["global_epoch"], "a new passport is stamped with the store's kill-switch epoch");

    // And through a keep restart on top.
    h.sigterm();
    h.start();
    h.assert_adopted(&[&a], "restart on the old store");
    a.assert_works("restart on the old store");
    check(&h, "restart on the old store");
    assert_store_kept(&fixture, &h.store(), "after a restart on the old store");
}
