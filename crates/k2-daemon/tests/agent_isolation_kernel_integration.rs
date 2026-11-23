//! Agent Isolation M1 on a real Linux kernel (`plan-agent-isolation-build-v1`
//! §6.3, the five-step demo, plus restart survival, whois, and the bites).
//!
//! The REAL daemon binary, headless, `K2_PTYD=1`, in a scratch HOME, run as
//! the main process's child inside a **delegated systemd system unit**
//! (`Delegate=yes`, `DelegateSubgroup=daemon`: what box prep gives the
//! daemon), so it really splits its unit, makes `agents/a-<workspace id>/`
//! per agent, and its sessions really join those cgroups and run under a
//! real Landlock fence. Nothing about the kernel is faked here: no temp-dir
//! cgroup tree, no stubbed Landlock. The agents are `/bin/sh` scripts the
//! daemon spawns like any tab; each writes what it saw (raw) into its own
//! workspace, and the test reads it back.
//!
//! Every test is `#[ignore]`: it needs that unit. Run them with
//! `scripts/agent-isolation-kernel-tests.sh <box>` (or by hand:
//! `sudo systemd-run --uid=$USER -p Delegate=yes -p DelegateSubgroup=daemon
//! --wait --pipe --setenv=… <test binary> --ignored --test-threads=1
//! --nocapture`). Run anywhere else with `--ignored`, they fail at the
//! preflight, loudly, with what is missing.
//!
//! Inputs (absolute paths; the compile-time ones are used when unset):
//! `K2_ISO_DAEMON_BIN` (k2-daemon), `K2_PTYD_BIN`, `K2_PTYD_TEST_AGENT`,
//! `K2_ISO_CLI` (cli/k2). `K2_ISO_TOOLCHAIN_HOME`: a home with `.cargo` and
//! `.rustup` for the "builds still work" step (linked into the scratch home).
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn input(env: &str) -> Option<PathBuf> {
    static INPUTS: std::sync::OnceLock<HashMap<&'static str, PathBuf>> = std::sync::OnceLock::new();
    INPUTS
        .get_or_init(|| {
            ["K2_ISO_DAEMON_BIN", "K2_PTYD_BIN", "K2_PTYD_TEST_AGENT", "K2_ISO_CLI", "K2_ISO_TOOLCHAIN_HOME", "K2_ISO_PERSIST_HOME", "K2_ISO_EXPECT", "K2_ISO_USER_BUS", "K2_ISO_TMUX_SOCK"]
                .into_iter()
                .filter_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()).map(|v| (k, PathBuf::from(v))))
                .collect()
        })
        .get(env)
        .cloned()
}

fn daemon_bin() -> PathBuf {
    input("K2_ISO_DAEMON_BIN").unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_k2-daemon")))
}

fn ptyd_bin(env: &str, name: &str) -> PathBuf {
    let p = input(env).unwrap_or_else(|| k2_core::test_env::built_workspace_bin("k2-ptyd", name));
    assert!(p.is_file(), "{env}={} is not a file", p.display());
    p
}

fn cli() -> PathBuf {
    input("K2_ISO_CLI").unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli/k2")))
}

fn wait_for<T>(what: &str, within: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(t.elapsed() < within, "timed out after {within:?} waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn exchange(s: &mut (impl Read + Write), head: &str, body: &str) -> (u16, String) {
    write!(s, "{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("write request");
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else { return (0, String::new()) };
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    exchange(&mut s, &format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n"), body)
}

fn over_tcp(port: u16, token: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("TCP connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    exchange(&mut s, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n"), "")
}

fn over_cell(sock: &Path, token: &str, path: &str) -> (u16, String) {
    let mut s = UnixStream::connect(sock).unwrap_or_else(|e| panic!("connect {}: {e}", sock.display()));
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    exchange(&mut s, &format!("GET {path} HTTP/1.1\r\nHost: k2\r\nAuthorization: Bearer {token}\r\n"), "")
}

/// A passport call answered past the passport check (a `/bin/sh` tab can't
/// be classified as canonical or sidecar, so whoami may answer 400 there).
fn served(s: u16, b: &str) -> bool {
    s == 200 || (s == 400 && b.contains("cannot determine identity"))
}

fn cgroup_of(pid: i32) -> String {
    let t = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_else(|e| panic!("/proc/{pid}/cgroup: {e}"));
    k2_core::agents_leaf::parse_self_cgroup(&t).expect("cgroup v2 path")
}

/// The test must run as the main process of a delegated system unit. Says
/// exactly what is missing otherwise.
fn preflight() -> PathBuf {
    let me = cgroup_of(std::process::id() as i32);
    let how = "run in a delegated unit: scripts/agent-isolation-kernel-tests.sh, or sudo systemd-run --uid=$USER -p Delegate=yes -p DelegateSubgroup=daemon --wait --pipe <test binary> --ignored --test-threads=1";
    assert!(!me.contains("/user@") && !me.contains(".scope"), "this test runs in {me}, not a system service: {how}");
    let unit = k2_core::agents_leaf::unit_base(&me).unwrap_or_else(|| panic!("no unit cgroup in {me}: {how}"));
    let dir = Path::new("/sys/fs/cgroup").join(unit.trim_start_matches('/'));
    use std::os::unix::fs::MetadataExt;
    let owner = std::fs::metadata(&dir).unwrap_or_else(|e| panic!("{}: {e}: {how}", dir.display())).uid();
    // SAFETY: no preconditions.
    let uid = unsafe { libc::geteuid() };
    assert_eq!(owner, uid, "{} belongs to uid {owner}, not this test's {uid} (no Delegate=yes): {how}", dir.display());
    assert!(me.ends_with("/daemon"), "the unit must start this test in its daemon/ leaf (DelegateSubgroup=daemon): {me}: {how}");
    assert!(k2_core::agent_fence::landlock_abi().is_ok(), "this kernel has no Landlock: {:?}", k2_core::agent_fence::landlock_abi());
    dir
}

struct Daemon {
    home: PathBuf,
    k2: PathBuf,
    child: Option<Child>,
    boots: usize,
    port: u16,
    pids: Vec<i32>,
    env: Vec<(String, String)>,
    /// M1b: the box's registered daemon (the persistent test unit), not
    /// one this test started: never killed, nor its ptyd.
    external: bool,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if std::thread::panicking() {
            for b in 1..=self.boots {
                let log = std::fs::read_to_string(self.home.join(format!("daemon-{b}.log"))).unwrap_or_default();
                let lines: Vec<&str> = log.lines().filter(|l| l.contains("[agents]") || l.contains("isolation") || l.contains("panic")).collect();
                eprintln!("──── daemon-{b}.log ([agents] lines) ────\n{}", lines.join("\n"));
            }
        }
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        // KillMode=process (as on the fleet): ptyd and the sessions outlive
        // the test's own exit unless killed here.
        let ptyd = std::fs::read_to_string(self.k2.join("run/ptyd/lock")).ok().and_then(|s| s.trim().parse::<i32>().ok()).filter(|_| !self.external);
        for p in self.pids.iter().copied().chain(ptyd) {
            // SAFETY: sessions and the ptyd this test started.
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
    }
}

impl Daemon {
    fn new(tag: &str) -> Daemon {
        for k in std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("K2_") || k.starts_with("K2SO_")).collect::<Vec<_>>() {
            if !k.starts_with("K2_ISO_") && k != "K2_PTYD_BIN" && k != "K2_PTYD_TEST_AGENT" {
                std::env::remove_var(k);
            }
        }
        let home = k2_core::test_env::short_tmp(format!("k2iso{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for d in [".k2", "xdg", "tmp", "DevProjects"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        if let Some(tc) = input("K2_ISO_TOOLCHAIN_HOME") {
            for d in [".cargo", ".rustup"] {
                if tc.join(d).exists() {
                    std::os::unix::fs::symlink(tc.join(d), home.join(d)).unwrap();
                }
            }
        }
        let k2 = k2_core::paths::k2_home_in(&home);
        // The CLI where installs keep it (~/.k2/bin, which a fence reads).
        std::fs::create_dir_all(k2.join("bin")).unwrap();
        std::fs::copy(cli(), k2.join("bin/k2")).unwrap_or_else(|e| panic!("copy {} into ~/.k2/bin: {e}", cli().display()));
        Daemon { home, k2, child: None, boots: 0, port: 0, pids: Vec::new(), env: Vec::new(), external: false }
    }

    /// M1b: the persistent test unit's daemon (`K2_ISO_PERSIST_HOME` is its
    /// HOME): the root helper's registered daemon on this box.
    fn external() -> Daemon {
        let home = input("K2_ISO_PERSIST_HOME").expect("K2_ISO_PERSIST_HOME: the persistent unit's HOME (scripts/agent-isolation-m1b-kernel-tests.sh)");
        let k2 = k2_core::paths::k2_home_in(&home);
        let port = std::fs::read_to_string(k2.join("daemon.port")).expect("the persistent daemon's port").trim().parse().expect("port");
        Daemon { home, k2, child: None, boots: 0, port, pids: Vec::new(), env: Vec::new(), external: true }
    }

    fn start(&mut self) {
        self.boots += 1;
        let log = std::fs::File::create(self.home.join(format!("daemon-{}.log", self.boots))).unwrap();
        // The agents here are /bin/sh scripts, not the shim test agent.
        let _ = ptyd_bin("K2_PTYD_TEST_AGENT", "k2-ptyd-test-agent");
        let mut c = Command::new(daemon_bin());
        c.env("K2_PTYD_BIN", ptyd_bin("K2_PTYD_BIN", "k2-ptyd"))
            .env("K2_PTYD", "1")
            .env("HOME", &self.home)
            .env("XDG_RUNTIME_DIR", self.home.join("xdg"))
            .env("TMPDIR", self.home.join("tmp"))
            .env("K2_TUNNEL_LEASE", "off")
            .env("K2_CONNECT_BASE", "http://127.0.0.1:1")
            .env("K2_CERT_BROKER_URL", "http://127.0.0.1:1")
            .env("K2_HANDOFF_DROPIN", "off")
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2_LLM_ACCOUNTS_LOOP", "0")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .env_remove("NOTIFY_SOCKET")
            .env_remove("K2_AIRGAP")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        for (k, v) in &self.env {
            c.env(k, v);
        }
        self.child = Some(c.spawn().expect("spawn k2-daemon"));
        let port_file = self.k2.join("daemon.port");
        let _ = std::fs::remove_file(self.k2.join("daemon.ready"));
        self.port = wait_for("daemon.port", Duration::from_secs(60), || std::fs::read_to_string(&port_file).ok()?.trim().parse::<u16>().ok());
        let port = self.port;
        wait_for("/boot-status phase=ready", Duration::from_secs(90), || {
            if let Some(c) = self.child.as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    panic!("daemon boot {} exited early: {st}", self.boots);
                }
            }
            let (s, b) = http(port, "GET", "/boot-status", "");
            (s == 200 && b.contains("\"ready\"")).then_some(())
        });
    }

    fn kill9(&mut self) {
        let mut c = self.child.take().expect("daemon");
        let _ = c.kill();
        let _ = c.wait();
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.k2.join("daemon.token")).expect("daemon.token").trim().to_string()
    }

    fn boot_status(&self) -> Value {
        let (s, b) = http(self.port, "GET", &format!("/boot-status?token={}", self.token()), "");
        assert_eq!(s, 200, "boot-status: {b}");
        serde_json::from_str(&b).unwrap()
    }

    fn audit(&self) -> String {
        std::fs::read_to_string(self.k2.join("auth-audit.jsonl")).unwrap_or_default()
    }

    /// A registered workspace `DevProjects/<name>` with a README; its id.
    fn workspace(&self, name: &str) -> (String, PathBuf) {
        let ws = self.home.join("DevProjects").join(name);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("README.md"), format!("{name}'s private notes\n")).unwrap();
        let token = self.token();
        let (s, b) = http(self.port, "POST", &format!("/cli/projects/create?token={token}"), &json!({"name": name, "path": ws.to_string_lossy()}).to_string());
        assert_eq!(s, 200, "projects/create {name}: {b}");
        let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap_or_else(|| panic!("id in {b}")).to_string();
        (id, ws)
    }

    fn set_fence(&self, ws_id: &str, on: bool) {
        let (s, b) = http(self.port, "POST", &format!("/cli/agent-access/set?token={}", self.token()), &json!({"workspace": ws_id, "toggle": "fence", "value": on}).to_string());
        assert_eq!(s, 200, "fence {on} for {ws_id}: {b}");
    }

    /// Spawn `/bin/sh -c script` as a tab of workspace `ws`, like the
    /// renderer opens one. Returns the session id.
    fn spawn(&mut self, key: &str, ws: &Path, script: &str) -> String {
        let body = json!({
            "agent_name": key,
            "cwd": ws.to_string_lossy(),
            "command": "/bin/sh",
            "args": ["-c", script],
            "cols": 100,
            "rows": 30,
        })
        .to_string();
        let (s, resp) = http(self.port, "POST", &format!("/cli/sessions/v2/spawn?token={}", self.token()), &body);
        assert_eq!(s, 200, "v2 spawn {key}: {resp}");
        serde_json::from_str::<Value>(&resp).unwrap()["sessionId"].as_str().expect("sessionId").to_string()
    }

    /// [`Daemon::spawn`] with environment variables the owner sets.
    fn spawn_env(&mut self, key: &str, ws: &Path, script: &str, env: Value) -> String {
        let body = json!({
            "agent_name": key,
            "cwd": ws.to_string_lossy(),
            "command": "/bin/sh",
            "args": ["-c", script],
            "cols": 100,
            "rows": 30,
            "env": env,
        })
        .to_string();
        let (s, resp) = http(self.port, "POST", &format!("/cli/sessions/v2/spawn?token={}", self.token()), &body);
        assert_eq!(s, 200, "v2 spawn {key}: {resp}");
        serde_json::from_str::<Value>(&resp).unwrap()["sessionId"].as_str().expect("sessionId").to_string()
    }
}

/// What a session script wrote: `key value…` lines.
fn results(path: &Path, done_marker: &str) -> HashMap<String, String> {
    let text = wait_for(&format!("{} to hold {done_marker}", path.display()), Duration::from_secs(240), || {
        let t = std::fs::read_to_string(path).ok()?;
        t.contains(done_marker).then_some(t)
    });
    println!("──── raw: {} ────\n{text}", path.display());
    text.lines().filter_map(|l| l.split_once(' ')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Env of a session that dumped `env -0` into `file`.
fn env_dump(file: &Path) -> HashMap<String, String> {
    let raw = wait_for(&format!("{}", file.display()), Duration::from_secs(60), || {
        let b = std::fs::read(file).ok()?;
        (!b.is_empty() && b.ends_with(b"\0")).then_some(b)
    });
    raw.split(|c| *c == 0).filter_map(|kv| std::str::from_utf8(kv).ok()?.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect()
}

/// An unfenced session of `ws` that dumps its env and its pid, then waits.
fn idle_script(dir: &Path) -> String {
    format!("echo $$ > {d}/pid; env -0 > {d}/env.tmp && mv {d}/env.tmp {d}/env; exec sleep 3600", d = dir.display())
}

/// The fenced agent's demo script (plan §6.3): every attempt's raw output
/// goes into `results`, then it waits for a stolen passport (`stolen`, a
/// file of `KEY=value` lines) and tries it everywhere.
fn demo_script(ws: &Path, sibling: &Path, sibling_cgroup: &Path, k2_home: &Path, cli: &Path, sibling_pid: i32) -> String {
    format!(
        r#"set +e
cd {ws}
R={ws}/results
: > $R
echo "pid $$" >> $R
echo "cgroup $(cat /proc/self/cgroup)" >> $R
echo "tmpdir $TMPDIR" >> $R
echo "ssh_auth_sock [${{SSH_AUTH_SOCK:-}}]" >> $R
# 1. It still works: a build and a commit in its own workspace.
if command -v cargo >/dev/null 2>&1; then
  cargo new --vcs none --offline hello > build.log 2>&1 && (cd hello && cargo build --offline >> ../build.log 2>&1)
  echo "build_rc $? $(tail -n 1 build.log | tr '\n' ' ')" >> $R
else
  echo "build_rc no-cargo (set K2_ISO_TOOLCHAIN_HOME)" >> $R
fi
git init -q gitws 2>git.err && cd gitws && echo x > f && git add f && git -c user.name=a -c user.email=a@example.invalid commit -qm x 2>>../git.err; echo "git_rc $? $(cat ../git.err | tr '\n' ' ')" >> $R; cd {ws}
# 2. It can't read a sibling's folder.
cat {sibling}/README.md > out 2> err; echo "sibling_read_rc $? $(cat err)" >> $R
ls {sibling} > out 2> err; echo "sibling_ls_rc $? $(cat err)" >> $R
# 2b. Nor a sibling's passport out of its process (/proc/<pid>/environ).
: > out; {{ tr '\0' '\n' < /proc/{bpid}/environ > out; }} 2> err; echo "sibling_environ_rc $? $(grep -c K2_HOOK_TOKEN out) $(cat err)" >> $R
kill -0 {bpid} 2> err; echo "sibling_signal_rc $? $(cat err)" >> $R
# 3. It can't read the owner token.
cat {k2_home}/daemon.token > out 2> err; echo "token_read_rc $? $(cat err)" >> $R
ls {k2_home} > out 2> err; echo "k2_home_ls_rc $? $(cat err)" >> $R
# 4. It can't leave its cgroup.
{{ echo $$ > {sib_cg}/cgroup.procs; }} 2> err; echo "cgroup_move_rc $? $(cat err)" >> $R
{{ echo 0 > {sib_cg}/../cgroup.procs; }} 2> err; echo "cgroup_leaf_move_rc $? $(cat err)" >> $R
mkdir {sib_cg}/../a-escape 2> err; echo "cgroup_mkdir_rc $? $(cat err)" >> $R
echo "cgroup_after $(cat /proc/self/cgroup)" >> $R
unshare -U -m true > out 2> err; echo "unshare_rc $? $(cat err)" >> $R
sudo -n true > out 2> err; echo "sudo_rc $? $(cat err)" >> $R
# Its own k2 call goes over its session socket (the CLI's own route).
bash -x {cli} whoami --json > k2.out 2> k2.err; echo "k2_whoami_rc $? $(cat k2.out | tr '\n' ' ' | cut -c1-300) trace: $(grep -v "^+* *_" k2.err | tail -n 12 | tr '\n' ' ' | cut -c1-900)" >> $R
echo "phase1 done" >> $R
# 5. A stolen passport is worthless.
while [ ! -f {ws}/stolen ]; do sleep 0.2; done
. {ws}/stolen
code=$(curl -s -o o1 -w '%{{http_code}}' --unix-socket "$STOLEN_SOCK" -H "Authorization: Bearer $STOLEN_TOKEN" http://localhost/cli/whoami); echo "stolen_on_its_socket $code $(cat o1)" >> $R
code=$(curl -s -o o2 -w '%{{http_code}}' --unix-socket "$STOLEN_SOCK" -H "Authorization: Bearer $STOLEN_TOKEN" -H "X-K2-Via-Tcp: 1" http://localhost/cli/projects/list); echo "stolen_on_its_socket_via_tcp $code $(cat o2 | cut -c1-300)" >> $R
code=$(curl -s -o o3 -w '%{{http_code}}' -H "Authorization: Bearer $STOLEN_TOKEN" http://127.0.0.1:$PORT/cli/whoami); echo "stolen_over_tcp $code $(cat o3)" >> $R
code=$(curl -s -o o4 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $STOLEN_TOKEN" http://localhost/cli/whoami); echo "stolen_on_own_socket $code $(cat o4)" >> $R
code=$(curl -s -o o5 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/capabilities); echo "own_on_own_socket $code $(cat o5 | cut -c1-300)" >> $R
bash {cli} capabilities --json > o9 2>&1; echo "own_cli_rc $? $(cat o9 | tr '\n' ' ' | cut -c1-300)" >> $R
# A request the socket doesn't serve, with no X-K2-Via-Tcp (a client that
# used to take "not served" to TCP): the socket hands it on itself.
code=$(curl -s -o o12 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -X POST -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/capabilities); echo "unserved_handed_on $code $(cut -c1-200 o12)" >> $R
# Review F3: the smuggling request (B's passport in the query, its own in
# the form body, a bare LF hiding a session header) on its own socket.
python3 - "$K2_HOOK_SOCK" "$STOLEN_TOKEN" "$STOLEN_SID" "$K2_HOOK_TOKEN" > o10 2>&1 <<'PY'
import socket, sys
sock, stolen, stolen_sid, own = sys.argv[1:5]
def ask(head, body):
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock)
    s.sendall(head.encode() + body)
    out = b""
    while True:
        b = s.recv(65536)
        if not b:
            break
        out += b
    return out.decode(errors="replace").split("\r\n")[0]
body = ("token=" + own).encode()
print("smuggled_query " + ask("POST /cli/thread?addr=B&token=%s HTTP/1.1\r\nHost: k2\r\nX-K2-Via-Tcp: 1\r\nFoo: x\nX-K2-Cell-Session: %s\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: %d\r\n\r\n" % (stolen, stolen_sid, len(body)), body))
print("smuggled_bearer " + ask("POST /cli/thread?addr=B&token=%s HTTP/1.1\r\nHost: k2\r\nAuthorization: Bearer %s\r\nX-K2-Via-Tcp: 1\r\nFoo: x\nX-K2-Cell-Session: %s\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: %d\r\n\r\n" % (stolen, own, stolen_sid, len(body)), body))
PY
cat o10 >> $R
code=$(curl -s -o o6 -w '%{{http_code}}' -H "Authorization: Bearer $K2_HOOK_TOKEN" http://127.0.0.1:$PORT/cli/whoami); echo "own_over_tcp $code $(cat o6)" >> $R
code=$(curl -s -o o7 -w '%{{http_code}}' -H "Authorization: Bearer $OWNER" "http://127.0.0.1:$PORT/cli/agent-whois?pid=$$"); echo "owner_pasted_over_tcp $code $(cat o7 | cut -c1-200)" >> $R
code=$(curl -s -o o8 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $OWNER" http://localhost/cli/whoami); echo "owner_pasted_on_socket $code $(cat o8 | cut -c1-200)" >> $R
echo "phase2 done" >> $R
exec sleep 3600
"#,
        ws = ws.display(),
        sibling = sibling.display(),
        sib_cg = sibling_cgroup.display(),
        k2_home = k2_home.display(),
        cli = cli.display(),
        bpid = sibling_pid,
    )
}

/// Refused, and the refusal names this attempt's own path (a stale error
/// from an earlier step can't pass for it).
fn denied_at(v: &str, path: &str) -> bool {
    denied(v) && v.contains(path)
}

fn denied(v: &str) -> bool {
    let (rc, rest) = v.split_once(' ').unwrap_or((v, ""));
    rc != "0" && (rest.contains("Permission denied") || rest.contains("Operation not permitted"))
}

/// The demo, end to end: the fenced agent A and its unfenced sibling B.
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn the_five_step_demo_on_a_real_kernel() {
    let unit = preflight();
    let mut d = Daemon::new("demo");
    d.start();
    let bs = d.boot_status();
    println!("agentIsolation: {}", bs["agentIsolation"]);
    // M1b: a test daemon is not the box's registered daemon, so the root
    // helper doesn't place its sessions: the daemon does (K2-user-owned
    // cgroups), which is "traceable", never claimed as identity. The
    // checks below are the same either way (the root-placed run is
    // root_owned_agent_cgroups_refuse_a_sibling, on the persistent unit).
    assert_eq!(bs["agentIsolation"]["identity"], "unavailable", "{}", bs["agentIsolation"]);
    assert_eq!(bs["agentIsolation"]["traceable"], true, "the daemon places the sessions: {}", bs["agentIsolation"]);
    let agents_dir = unit.join("agents");
    let (a_id, a_ws) = d.workspace("A");
    let (b_id, b_ws) = d.workspace("B");
    d.set_fence(&a_id, true);

    let b_sid = d.spawn("tab-b", &b_ws, &idle_script(&b_ws));
    let b_env = env_dump(&b_ws.join("env"));
    let b_pid: i32 = wait_for("B's pid", Duration::from_secs(30), || std::fs::read_to_string(b_ws.join("pid")).ok()?.trim().parse().ok());
    d.pids.push(b_pid);
    assert!(cgroup_of(b_pid).ends_with(&format!("/agents/a-{b_id}")), "B runs in its own cgroup: {}", cgroup_of(b_pid));

    let script = demo_script(&a_ws, &b_ws, &agents_dir.join(format!("a-{b_id}")), &d.k2, &d.k2.join("bin/k2"), b_pid);
    let a_sid = d.spawn("tab-a", &a_ws, &script);
    let r = results(&a_ws.join("results"), "phase1 done");
    let a_pid: i32 = r["pid"].trim().parse().expect("A's pid");
    d.pids.push(a_pid);

    // 1. It still works.
    assert!(r["cgroup"].ends_with(&format!("/agents/a-{a_id}")), "A's terminal is in A's cgroup: {}", r["cgroup"]);
    assert!(r["tmpdir"].contains(&format!("agents/{a_id}/tmp")), "a private TMPDIR: {}", r["tmpdir"]);
    assert_eq!(r["ssh_auth_sock"], "[]", "the owner's ssh-agent is not handed in");
    assert!(r["git_rc"].starts_with("0"), "git in its own workspace: {}", r["git_rc"]);
    if input("K2_ISO_TOOLCHAIN_HOME").is_some() {
        assert!(r["build_rc"].starts_with("0 "), "cargo build in its own workspace: {}", r["build_rc"]);
    }
    // 2. A sibling's folder.
    assert!(denied_at(&r["sibling_read_rc"], "B/README.md"), "reading B's README must be denied: {}", r["sibling_read_rc"]);
    assert!(denied_at(&r["sibling_ls_rc"], "DevProjects/B"), "listing B's folder must be denied: {}", r["sibling_ls_rc"]);
    assert!(denied_at(&r["sibling_environ_rc"], "/environ"), "reading B's passport from /proc/<pid>/environ must be denied: {}", r["sibling_environ_rc"]);
    // Known (M2/M5): Landlock on 6.8 doesn't scope signals (ABI 6, 6.12+).
    println!("KNOWN GAP: a fenced agent signalling a sibling (kill -0): {}", r["sibling_signal_rc"]);
    // 3. The owner token.
    assert!(denied_at(&r["token_read_rc"], "daemon.token"), "reading the owner token must be denied: {}", r["token_read_rc"]);
    assert!(denied_at(&r["k2_home_ls_rc"], ".k2'"), "listing ~/.k2 must be denied: {}", r["k2_home_ls_rc"]);
    // 4. Its cgroup.
    assert!(denied_at(&r["cgroup_move_rc"], &format!("a-{b_id}/cgroup.procs")), "moving into B's cgroup must be denied: {}", r["cgroup_move_rc"]);
    assert!(denied_at(&r["cgroup_leaf_move_rc"], "/../cgroup.procs"), "moving into the shared leaf must be denied: {}", r["cgroup_leaf_move_rc"]);
    assert!(denied_at(&r["cgroup_mkdir_rc"], "a-escape"), "making a cgroup must be denied: {}", r["cgroup_mkdir_rc"]);
    assert!(r["cgroup_after"].ends_with(&format!("/agents/a-{a_id}")), "still in A's cgroup: {}", r["cgroup_after"]);
    assert!(!r["sudo_rc"].starts_with("0"), "no sudo in a fence: {}", r["sudo_rc"]);
    assert!(!r["unshare_rc"].starts_with("0"), "no new user+mount namespace: {}", r["unshare_rc"]);
    assert!(
        !r["k2_whoami_rc"].contains("passport_needs_socket") && !r["k2_whoami_rc"].contains("passport_wrong_cgroup"),
        "A's own k2 reaches the daemon over its socket: {}",
        r["k2_whoami_rc"]
    );

    // 5. B's passport, copied into A's shell.
    std::fs::write(
        a_ws.join("stolen"),
        format!(
            "STOLEN_TOKEN='{}'\nSTOLEN_SID='{}'\nSTOLEN_SOCK='{}'\nPORT={}\nOWNER='{}'\nOWN_ADDR='A'\n",
            b_env["K2_HOOK_TOKEN"],
            b_env["K2_HOOK_TOKEN"].split_once('.').expect("passport shape").0,
            b_env["K2_HOOK_SOCK"],
            d.port,
            d.token()
        ),
    )
    .unwrap();
    let r = results(&a_ws.join("results"), "phase2 done");
    let code = |k: &str| r[k].split_whitespace().next().unwrap_or("").to_string();
    assert!(code("stolen_on_its_socket") == "403" && r["stolen_on_its_socket"].contains("passport_wrong_cgroup"), "{}", r["stolen_on_its_socket"]);
    assert!(code("stolen_on_its_socket_via_tcp") == "403" && r["stolen_on_its_socket_via_tcp"].contains("passport_wrong_cgroup"), "{}", r["stolen_on_its_socket_via_tcp"]);
    assert!(code("stolen_over_tcp") == "403" && r["stolen_over_tcp"].contains("passport_needs_socket"), "{}", r["stolen_over_tcp"]);
    assert_eq!(code("stolen_on_own_socket"), "403", "B's passport on A's socket: {}", r["stolen_on_own_socket"]);
    // A's own passport works: a 2xx on a real agent verb, raw and through
    // the CLI (review: never a 400 counted as working).
    assert_eq!(code("own_on_own_socket"), "200", "A's own passport on its own socket, /cli/capabilities: {}", r["own_on_own_socket"]);
    assert!(r["own_cli_rc"].starts_with("0 {"), "k2 capabilities --json from the fenced terminal: {}", r["own_cli_rc"]);
    assert!(
        !r["unserved_handed_on"].starts_with("000") && !r["unserved_handed_on"].contains("not served on cell socket") && !r["unserved_handed_on"].contains("passport_needs_socket"),
        "a route the socket doesn't serve is answered by the TCP route code, not 'not served': {}",
        r["unserved_handed_on"]
    );
    // Review F3: the smuggling shapes never run as B.
    assert!(r["smuggled_query"].contains("403"), "B's passport in the query, own in the body, a bare LF: {}", r["smuggled_query"]);
    assert!(r["smuggled_bearer"].contains("400"), "own Bearer with a bare-LF smuggled session header is refused: {}", r["smuggled_bearer"]);
    assert!(code("own_over_tcp") == "403" && r["own_over_tcp"].contains("passport_needs_socket"), "{}", r["own_over_tcp"]);
    assert_eq!(code("owner_pasted_on_socket"), "403", "the owner token on a session socket: {}", r["owner_pasted_on_socket"]);
    // Known and reported (M2's nftables rule closes it): the owner token,
    // pasted by hand, still works over TCP. The fence keeps it off disk.
    println!("owner token pasted by hand over TCP (M2 closes): {}", r["owner_pasted_over_tcp"]);

    // The audit rows name both agents.
    let audit = d.audit();
    println!("──── auth-audit.jsonl (agent rows) ────");
    for l in audit.lines().filter(|l| l.contains("\"agent.")) {
        println!("{l}");
    }
    assert!(
        audit.lines().any(|l| l.contains("agent.passport_wrong_cgroup") && l.contains(&format!("agent:{b_id}")) && l.contains(&format!("agent {a_id}"))),
        "an audit row: B's passport from A's cgroup"
    );
    assert!(audit.lines().any(|l| l.contains("agent.passport_needs_socket") && l.contains(&format!("agent:{b_id}"))), "an audit row: B's passport over TCP");
    assert!(audit.lines().any(|l| l.contains("agent.passport_wrong_socket") && l.contains(&format!("agent:{b_id}"))), "an audit row: B's passport on A's socket (the F3 shape)");

    // Review F4: B's passport in the session cookie, or in a TCP form body.
    let b_tok = &b_env["K2_HOOK_TOKEN"];
    let mut s = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let (st, body) = exchange(&mut s, &format!("GET /cli/thread?addr=B&limit=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: k2_session={b_tok}\r\n"), "");
    println!("cookie passport over TCP: {st} {body}");
    assert!(st == 403 && body.contains("passport_needs_socket"), "a passport in the k2_session cookie: {st} {body}");
    let form = format!("token={b_tok}&message=x");
    let mut s = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let (st, body) = exchange(&mut s, "POST /cli/respond HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/x-www-form-urlencoded\r\n", &form);
    println!("form-body passport over TCP: {st} {body}");
    assert!((st == 401 || st == 403) && !body.contains("\"ok\":true"), "a passport in a TCP form body: {st} {body}");
    assert!(d.audit().lines().any(|l| l.contains("in a TCP form body") && l.contains(&format!("agent:{b_id}"))), "an audit row: the form-body passport");

    // whois (owner only), and an agent's passport can't ask.
    let (s, b) = http(d.port, "GET", &format!("/cli/agent-whois?pid={a_pid}&token={}", d.token()), "");
    println!("whois {a_pid}: {s} {b}");
    let w: Value = serde_json::from_str(&b).unwrap();
    assert_eq!((s, w["workspaceId"].as_str(), w["fence"].as_bool()), (200, Some(a_id.as_str()), Some(true)), "{b}");
    let (s, _) = over_tcp(d.port, &b_env["K2_HOOK_TOKEN"], &format!("/cli/agent-whois?pid={a_pid}"));
    assert_eq!(s, 403, "a passport never asks whois");

    // ps shows the agent at a glance.
    let ps = Command::new("ps").args(["-ww", "-o", "pid,cgroup:160,args", "-p", &a_pid.to_string()]).output().unwrap();
    println!("──── ps -o pid,cgroup,cmd ────\n{}", String::from_utf8_lossy(&ps.stdout));
    assert!(String::from_utf8_lossy(&ps.stdout).contains(&format!("a-{a_id}")));
    let _ = (a_sid, b_sid);
}

/// Restart survival: ptyd's sessions outlive the daemon in their cgroups;
/// the next daemon knows them from the cgroup names and the passport store.
/// M2 review (open items): a fenced agent can't bring an outside file into
/// its workspace for the daemon to read. A hard link to a credential file or
/// to a sibling's file is refused by the kernel (Landlock's refer rule); a
/// symlink can be made but not read, and the daemon refuses to deliver it
/// (403 `path_out_of_reach`) and delivers nothing.
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn a_fenced_agent_links_no_outside_file_in() {
    let _unit = preflight();
    let mut d = Daemon::new("links");
    d.start();
    let (a_id, a_ws) = d.workspace("A");
    let (_b_id, b_ws) = d.workspace("B");
    // A hard link made before the fence went on (by anyone unfenced).
    std::fs::hard_link(b_ws.join("README.md"), a_ws.join("premade")).unwrap();
    d.set_fence(&a_id, true);
    let ssh = d.home.join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("id_test"), "PRIVATE KEY\n").unwrap();
    let script = format!(
        r#"R={a}/results; : > $R; echo $$ > {a}/pid
ln {key} {a}/hl 2> {a}/e1; echo "hardlink_key_rc $? $(cat {a}/e1)" >> $R
ln {b}/README.md {a}/hl2 2> {a}/e2; echo "hardlink_sibling_rc $? $(cat {a}/e2)" >> $R
ln -s {key} {a}/sl 2> {a}/e3; cat {a}/sl > /dev/null 2> {a}/e4; echo "symlink_read_rc $? $(cat {a}/e4)" >> $R
code=$(curl -s -o {a}/o1 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -X POST -H "Authorization: Bearer $K2_HOOK_TOKEN" -H 'Content-Type: application/x-www-form-urlencoded' --data "workspace={a_id}&path={a}/sl&wake=false" http://localhost/cli/inbox/deliver); echo "deliver_symlink $code $(cut -c1-200 {a}/o1)" >> $R
code=$(curl -s -o {a}/o2 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -X POST -H "Authorization: Bearer $K2_HOOK_TOKEN" -H 'Content-Type: application/x-www-form-urlencoded' --data "workspace={a_id}&path={key}&wake=false" http://localhost/cli/inbox/deliver); echo "deliver_key $code $(cut -c1-200 {a}/o2)" >> $R
code=$(curl -s -o {a}/o3 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -X POST -H "Authorization: Bearer $K2_HOOK_TOKEN" -H 'Content-Type: application/x-www-form-urlencoded' --data "workspace={a_id}&path={a}/premade&wake=false" http://localhost/cli/inbox/deliver); echo "deliver_premade $code $(cut -c1-200 {a}/o3)" >> $R
echo "done" >> $R; exec sleep 3600
"#,
        a = a_ws.display(),
        b = b_ws.display(),
        key = ssh.join("id_test").display(),
    );
    d.spawn("tab-a", &a_ws, &script);
    let r = results(&a_ws.join("results"), "done");
    let a_pid: i32 = std::fs::read_to_string(a_ws.join("pid")).unwrap().trim().parse().unwrap();
    d.pids.push(a_pid);
    println!("links: {r:?}");
    assert!(!r["hardlink_key_rc"].starts_with("0"), "a hard link to a credential file is refused: {}", r["hardlink_key_rc"]);
    assert!(!r["hardlink_sibling_rc"].starts_with("0"), "a hard link to a sibling's file is refused: {}", r["hardlink_sibling_rc"]);
    assert!(!a_ws.join("hl").exists() && !a_ws.join("hl2").exists(), "no hard link was made");
    assert!(denied(&r["symlink_read_rc"]), "a symlink out can't be read: {}", r["symlink_read_rc"]);
    for row in ["deliver_symlink", "deliver_key", "deliver_premade"] {
        assert!(r[row].starts_with("403") && r[row].contains("path_out_of_reach"), "{row}: the daemon refuses: {}", r[row]);
    }
    let inbox = a_ws.join(".k2/inbox");
    let landed = std::fs::read_dir(&inbox).map(|d| d.count()).unwrap_or(0);
    assert_eq!(landed, 0, "nothing was delivered into {}", inbox.display());
}

#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn a_fenced_agent_survives_a_daemon_restart_and_is_still_recognised() {
    let unit = preflight();
    let mut d = Daemon::new("restart");
    d.start();
    let (a_id, a_ws) = d.workspace("A");
    d.set_fence(&a_id, true);
    d.spawn("tab-a", &a_ws, &idle_script(&a_ws));
    let env = env_dump(&a_ws.join("env"));
    let a_pid: i32 = wait_for("A's pid", Duration::from_secs(30), || std::fs::read_to_string(a_ws.join("pid")).ok()?.trim().parse().ok());
    d.pids.push(a_pid);
    let want = format!("/agents/a-{a_id}");
    assert!(cgroup_of(a_pid).ends_with(&want));
    d.kill9();
    d.start();
    assert!(cgroup_of(a_pid).ends_with(&want), "after the restart A is still in its cgroup: {}", cgroup_of(a_pid));
    let store: Value = serde_json::from_str(&std::fs::read_to_string(d.k2.join("hook-sessions.json")).unwrap()).unwrap();
    let sid = env["K2_HOOK_TOKEN"].split_once('.').unwrap().0.to_string();
    assert_eq!(store["records"][&sid]["agent_cgroup"], json!(a_id), "the binding survived in the store: {}", store["records"][&sid]);
    let (s, b) = http(d.port, "GET", &format!("/cli/agent-whois?pid={a_pid}&token={}", d.token()), "");
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["workspaceId"], json!(a_id), "whois after the restart: {s} {b}");
    // From A's cgroup (a probe the test places there, as an unfenced
    // process of the same user can): accepted. From outside: refused.
    let probe = in_cgroup(&unit.join("agents").join(format!("a-{a_id}")), &env["K2_HOOK_SOCK"], &env["K2_HOOK_TOKEN"]);
    assert!(served(probe.0, &probe.1), "A's passport from A's cgroup after the restart: {probe:?}");
    let (s, b) = over_cell(Path::new(&env["K2_HOOK_SOCK"]), &env["K2_HOOK_TOKEN"], "/cli/whoami");
    assert!(s == 403 && b.contains("passport_wrong_cgroup"), "A's passport from the test's own cgroup: {s} {b}");
}

/// `curl` on `sock` with `token`, from a process placed in `cgroup`.
fn in_cgroup(cgroup: &Path, sock: &str, token: &str) -> (u16, String) {
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "echo $$ > {}/cgroup.procs || exit 9; exec curl -s -w '\\n%{{http_code}}' --unix-socket '{sock}' -H 'Authorization: Bearer {token}' http://localhost/cli/whoami",
            cgroup.display()
        ))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "in_cgroup probe: {:?} {text} {}", out.status, String::from_utf8_lossy(&out.stderr));
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", ""));
    (code.trim().parse().unwrap_or(0), body.to_string())
}

/// The bites: each guarantee fails without its mechanism.
/// - Fence off: A reads B's folder and the owner token.
/// - Identity off (the emergency switch): B's passport works from anywhere.
/// - Known gap until M1b: an UNFENCED agent can move itself into another
///   agent's cgroup and pose as it (the cgroup files belong to the K2 user).
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn the_bites_each_guarantee_fails_without_its_mechanism() {
    let unit = preflight();
    let mut d = Daemon::new("bites");
    d.start();
    let (a_id, a_ws) = d.workspace("A");
    let (b_id, b_ws) = d.workspace("B");
    d.spawn("tab-b", &b_ws, &idle_script(&b_ws));
    let b_env = env_dump(&b_ws.join("env"));
    let b_pid: i32 = wait_for("B's pid", Duration::from_secs(30), || std::fs::read_to_string(b_ws.join("pid")).ok()?.trim().parse().ok());
    d.pids.push(b_pid);
    // Fence OFF (the default): the same demo script reads what the fence hid.
    let script = demo_script(&a_ws, &b_ws, &unit.join("agents").join(format!("a-{b_id}")), &d.k2, &d.k2.join("bin/k2"), b_pid);
    d.spawn("tab-a", &a_ws, &script);
    let r = results(&a_ws.join("results"), "phase1 done");
    d.pids.push(r["pid"].trim().parse().unwrap());
    assert!(r["sibling_read_rc"].starts_with("0"), "BITE fence off: A reads B's README: {}", r["sibling_read_rc"]);
    assert!(r["token_read_rc"].starts_with("0"), "BITE fence off: A reads the owner token: {}", r["token_read_rc"]);
    assert!(r["sibling_environ_rc"].starts_with("0 1"), "BITE fence off: A reads B's passport from /proc: {}", r["sibling_environ_rc"]);
    // Known gap until M1b: unfenced, A moved itself into B's cgroup.
    assert!(r["cgroup_move_rc"].starts_with("0"), "unfenced, the move into B's cgroup works (M1b closes it): {}", r["cgroup_move_rc"]);
    // …and then out of it into the shared leaf (the script's next step).
    assert!(r["cgroup_leaf_move_rc"].starts_with("0"), "{}", r["cgroup_leaf_move_rc"]);
    assert!(r["cgroup_after"].ends_with("/agents"), "{}", r["cgroup_after"]);
    println!("fence off, sudo -n (this box's sudoers decide): {}", r["sudo_rc"]);
    println!("KNOWN GAP (M1b): an unfenced agent moved into another agent's cgroup (rc {})", r["cgroup_move_rc"]);
    let _ = a_id;

    // Identity OFF: the emergency switch. B's passport from the test's own
    // cgroup (no agent's) is accepted, as before 0.46.17.
    d.kill9();
    d.env.push(("K2_AGENT_CGROUP_IDENTITY".into(), "0".into()));
    d.start();
    let bs = d.boot_status();
    assert_eq!(bs["agentIsolation"]["identity"], "off", "{}", bs["agentIsolation"]);
    println!("agentIsolation with the switch off: {}", bs["agentIsolation"]);
    let (s, b) = over_cell(Path::new(&b_env["K2_HOOK_SOCK"]), &b_env["K2_HOOK_TOKEN"], "/cli/whoami");
    assert!(served(s, &b), "BITE identity off: B's passport works from outside B's cgroup on its socket: {s} {b}");
    let (s, b) = over_tcp(d.port, &b_env["K2_HOOK_TOKEN"], "/cli/whoami");
    assert!(served(s, &b), "BITE identity off: B's passport works over TCP: {s} {b}");
    let log = std::fs::read_to_string(d.home.join(format!("daemon-{}.log", d.boots))).unwrap_or_default();
    assert!(log.contains("K2_AGENT_CGROUP_IDENTITY=0"), "the switch is logged loudly at boot");
}

/// Review F1: a `k2 job` run is the agent's like its terminals: in its
/// cgroup, under its fence, with a bound passport and a session socket of
/// its own. Review F2: a link planted in the agent's own login slot, or a
/// harness home outside its slot, never widens the fence.
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn job_runs_and_login_slots_stay_inside_the_fence() {
    let _unit = preflight();
    let mut d = Daemon::new("jobs");
    d.start();
    let (a_id, a_ws) = d.workspace("A");
    d.set_fence(&a_id, true);
    let token = d.token();
    // F1: the owner adds a job to A and runs it; it reports what it is.
    let script = format!(
        r#"R={ws}/job.results; : > $R
echo "cgroup $(cat /proc/self/cgroup)" >> $R
cat {k2}/daemon.token > /dev/null 2> {ws}/job.err; echo "token_read_rc $? $(cat {ws}/job.err)" >> $R
echo "sock [${{K2_HOOK_SOCK:-}}]" >> $R
code=$(curl -s -o {ws}/job.o1 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/capabilities); echo "own_on_socket $code" >> $R
code=$(curl -s -o {ws}/job.o2 -w '%{{http_code}}' -H "Authorization: Bearer $K2_HOOK_TOKEN" "http://127.0.0.1:$K2_PORT/cli/thread?addr=A&limit=1"); echo "own_over_tcp $code $(cut -c1-120 {ws}/job.o2)" >> $R
echo "pid $$" >> $R
echo "job done" >> $R
sleep 5
"#,
        ws = a_ws.display(),
        k2 = d.k2.display()
    );
    let (s, b) = http(d.port, "POST", &format!("/cli/jobs/add?token={token}"), &json!({"workspace": a_id, "name": "probe", "every": "24h", "argv": ["sh", "-c", script]}).to_string());
    assert_eq!(s, 200, "jobs/add: {b}");
    let (s, b) = http(d.port, "POST", &format!("/cli/jobs/run?token={token}"), &json!({"workspace": a_id, "name": "probe"}).to_string());
    assert_eq!(s, 200, "jobs/run: {b}");
    let r = results(&a_ws.join("job.results"), "job done");
    assert!(r["cgroup"].ends_with(&format!("/agents/a-{a_id}")), "the job runs in A's cgroup: {}", r["cgroup"]);
    assert!(denied_at(&r["token_read_rc"], "daemon.token"), "the job is fenced: {}", r["token_read_rc"]);
    assert!(r["sock"].ends_with(".sock]") && r["sock"].contains("job-"), "the job has its own session socket: {}", r["sock"]);
    assert_eq!(r["own_on_socket"], "200", "the job's passport works on its socket");
    assert!(r["own_over_tcp"].starts_with("403") && r["own_over_tcp"].contains("passport_needs_socket"), "the job's passport is bound: {}", r["own_over_tcp"]);
    let job_pid: u32 = r["pid"].trim().parse().unwrap();
    let (s, b) = http(d.port, "GET", &format!("/cli/agent-whois?pid={job_pid}&token={token}"), "");
    assert!(s == 200 && serde_json::from_str::<Value>(&b).unwrap()["workspaceId"] == json!(a_id), "whois names the job's agent: {s} {b}");

    // F2: A's pinned login slot with K2's own link and two planted ones.
    let home = d.home.clone();
    let slot = d.k2.join("llm-accounts/codex/acct-1");
    for dir in [slot.clone(), home.join(".ssh"), home.join(".codex/sessions")] {
        std::fs::create_dir_all(&dir).unwrap();
    }
    std::fs::write(home.join(".ssh/id_test"), "secret key").unwrap();
    std::os::unix::fs::symlink(home.join(".codex/sessions"), slot.join("sessions")).unwrap();
    std::os::unix::fs::symlink(home.join(".ssh"), slot.join("x")).unwrap();
    std::os::unix::fs::symlink(&home, slot.join("everything")).unwrap();
    // Review R2: same-named links to the default Codex login and config.
    std::fs::write(home.join(".codex/auth.json"), "{\"default\":\"login\"}").unwrap();
    std::fs::write(home.join(".codex/config.toml"), "notify = []\n").unwrap();
    std::os::unix::fs::symlink(home.join(".codex/auth.json"), slot.join("auth.json")).unwrap();
    std::os::unix::fs::symlink(home.join(".codex/config.toml"), slot.join("config.toml")).unwrap();
    let probe = format!(
        r#"R={ws}/slot.results; : > $R
cat {home}/.ssh/id_test > /dev/null 2> {ws}/s.err; echo "ssh_rc $? $(cat {ws}/s.err)" >> $R
cat $CODEX_HOME/x/id_test > /dev/null 2> {ws}/s.err; echo "link_rc $? $(cat {ws}/s.err)" >> $R
ls $CODEX_HOME/everything/ > /dev/null 2> {ws}/s.err; echo "home_rc $? $(cat {ws}/s.err)" >> $R
touch $CODEX_HOME/sessions/ok 2> {ws}/s.err; echo "own_link_rc $? $(cat {ws}/s.err)" >> $R
cat $CODEX_HOME/auth.json > /dev/null 2> {ws}/s.err; echo "auth_link_rc $? $(cat {ws}/s.err)" >> $R
{{ echo 'notify = ["sh"]' >> $CODEX_HOME/config.toml; }} 2> {ws}/s.err; echo "config_link_rc $? $(cat {ws}/s.err)" >> $R
echo "slot done" >> $R; exec sleep 3600
"#,
        ws = a_ws.display(),
        home = home.display()
    );
    let body = json!({"agent_name": "tab-slot", "cwd": a_ws.to_string_lossy(), "command": "/bin/sh", "args": ["-c", probe], "cols": 100, "rows": 30, "env": {"CODEX_HOME": slot.to_string_lossy()}}).to_string();
    let (s, resp) = http(d.port, "POST", &format!("/cli/sessions/v2/spawn?token={token}"), &body);
    assert_eq!(s, 200, "spawn with a pinned slot: {resp}");
    let r = results(&a_ws.join("slot.results"), "slot done");
    assert!(denied_at(&r["ssh_rc"], ".ssh/id_test"), "~/.ssh stays denied: {}", r["ssh_rc"]);
    assert!(denied(&r["link_rc"]), "a planted slot link to ~/.ssh grants nothing: {}", r["link_rc"]);
    assert!(denied(&r["home_rc"]), "a planted slot link to the home grants nothing: {}", r["home_rc"]);
    assert!(r["own_link_rc"].starts_with("0"), "K2's own slot link (Codex sessions) works: {}", r["own_link_rc"]);
    assert!(denied_at(&r["auth_link_rc"], "acct-1/auth.json"), "a same-named link to the default Codex login grants nothing: {}", r["auth_link_rc"]);
    assert!(denied_at(&r["config_link_rc"], "acct-1/config.toml"), "a same-named link to the shared Codex config grants nothing: {}", r["config_link_rc"]);
    assert_eq!(std::fs::read_to_string(home.join(".codex/config.toml")).unwrap(), "notify = []\n", "the shared config is unchanged");
    // A harness home that is neither its slot nor the tool's own home.
    let body = json!({"agent_name": "tab-bad", "cwd": a_ws.to_string_lossy(), "command": "/bin/sh", "args": ["-c", "exec sleep 60"], "cols": 100, "rows": 30, "env": {"CODEX_HOME": home.join(".ssh").to_string_lossy()}}).to_string();
    let (s, resp) = http(d.port, "POST", &format!("/cli/sessions/v2/spawn?token={token}"), &body);
    println!("spawn with CODEX_HOME=~/.ssh: {s} {resp}");
    assert!(s != 200 && resp.contains("isolation"), "the spawn refuses a harness home outside its slot: {s} {resp}");
}

/// The local ports of the live TCP connections from 127.0.0.1 to the
/// daemon's port that isn't the daemon's own end (`/proc/net/tcp`).
fn forwards_to(port: u16) -> Vec<u16> {
    let t = std::fs::read_to_string("/proc/net/tcp").expect("/proc/net/tcp");
    let to = format!("0100007F:{port:04X}");
    t.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (local, remote, state) = (f.get(1)?, f.get(2)?, f.get(3)?);
            // ESTABLISHED or CLOSE_WAIT (the daemon has sent all and closed).
            (*remote == to && local.starts_with("0100007F:") && matches!(*state, "01" | "08")).then(|| u16::from_str_radix(&local[9..], 16).ok())?
        })
        .collect()
}

/// Review R1: a held-open forward is recognised by its full source address
/// (127.0.0.1 and the port), never by the port alone. While A's own
/// forward holds 127.0.0.1:P, anyone may bind 127.0.0.2:P; a copied A
/// passport presented from there must get 403 passport_needs_socket.
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn a_held_forward_is_recognised_by_its_full_address_only() {
    let _unit = preflight();
    let mut d = Daemon::new("held");
    // LAN listening on (0.0.0.0): review T1's second row connects to the
    // box's LAN address.
    d.env.push(("K2_LISTEN".into(), "lan".into()));
    d.start();
    let (_a_id, a_ws) = d.workspace("A");
    // 30 inbox items with ~900 KB titles: A's `inbox/list` (a passport
    // route on TCP too) answers ~27 MB, far more than the socket buffers
    // hold, so A's forward stays open while A doesn't read its answer.
    let inbox = a_ws.join(".k2/inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let title = "x".repeat(900_000);
    for i in 0..30 {
        std::fs::write(inbox.join(format!("big-{i:02}.md")), format!("---\ntitle: {title}\ncreated: 2026-11-21T00:00:{i:02}Z\n---\nbody\n")).unwrap();
    }
    let script = format!(
        r#"echo $$ > {ws}/pid; env -0 > {ws}/env.tmp && mv {ws}/env.tmp {ws}/env
python3 - "$K2_HOOK_SOCK" "$K2_HOOK_TOKEN" "{ws}" <<'PY'
import socket, sys, time
sock, tok, ws = sys.argv[1:4]
s = socket.socket(socket.AF_UNIX)
s.connect(sock)
s.sendall(("GET /cli/inbox/list?project=%s HTTP/1.1\r\nHost: k2\r\nAuthorization: Bearer %s\r\nX-K2-Via-Tcp: 1\r\n\r\n" % (ws, tok)).encode())
first = s.recv(400).decode(errors="replace").replace("\r", " ").replace("\n", " ")
open(ws + "/held.results", "w").write("held " + first + "\n")
time.sleep(120)
PY
exec sleep 3600
"#,
        ws = a_ws.display()
    );
    let before = forwards_to(d.port);
    d.spawn("tab-held", &a_ws, &script);
    let a_env = env_dump(&a_ws.join("env"));
    let r = results(&a_ws.join("held.results"), "held");
    assert!(r["held"].contains(" 200 "), "A's forward answered and is being held: {}", r["held"]);
    let ports: Vec<u16> = forwards_to(d.port).into_iter().filter(|p| !before.contains(p)).collect();
    assert_eq!(ports.len(), 1, "one held forward to the daemon (before: {before:?}): {ports:?}");
    let p = ports[0];
    // From 127.0.0.2, the held forward's port, A's copied passport.
    let out = Command::new("python3")
        .args(["-c", r#"import socket, sys
s = socket.socket()
s.bind(("127.0.0.2", int(sys.argv[1])))
s.connect(("127.0.0.1", int(sys.argv[2])))
s.sendall(("GET /cli/capabilities HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer %s\r\nConnection: close\r\n\r\n" % sys.argv[3]).encode())
out = b""
while True:
    b = s.recv(65536)
    if not b:
        break
    out += b
print(out.decode(errors="replace").split("\r\n")[0])
print(out.decode(errors="replace").split("\r\n\r\n", 1)[-1][:200])"#, &p.to_string(), &d.port.to_string(), &a_env["K2_HOOK_TOKEN"]])
        .output()
        .expect("python3");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    println!("127.0.0.2:{p} with A's passport, while 127.0.0.1:{p} is A's held forward:\n{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "the 127.0.0.2 client ran: {}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains(" 403 ") && text.contains("passport_needs_socket"), "same port, another loopback address: {text}");

    // Review T1 (a): nothing else can hold 127.0.0.1:P while the forward
    // does, not even with SO_REUSEADDR + SO_REUSEPORT.
    let out = Command::new("python3")
        .args(["-c", r#"import errno, socket, sys
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
try:
    s.bind(("127.0.0.1", int(sys.argv[1])))
    print("bound", s.getsockname())
except OSError as e:
    print("refused", errno.errorcode.get(e.errno, e.errno))"#, &p.to_string()])
        .output()
        .expect("python3");
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    println!("bind 127.0.0.1:{p} with SO_REUSEADDR + SO_REUSEPORT during the hold: {text}");
    assert_eq!(text, "refused EADDRINUSE", "127.0.0.1:{p} while A's forward holds it");

    // Review T1 (b): with the daemon on 0.0.0.0, a connect from 127.0.0.1 to
    // the box's LAN address never gets port P. The ephemeral range is set to
    // P alone for the few connects below (and put back), so any port the
    // kernel would hand out could only be P.
    let lan = String::from_utf8_lossy(
        &Command::new("python3")
            .args(["-c", "import socket\ns = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)\ns.connect(('192.0.2.1', 9))\nprint(s.getsockname()[0])"])
            .output()
            .expect("python3")
            .stdout,
    )
    .trim()
    .to_string();
    assert!(!lan.is_empty() && !lan.starts_with("127."), "the box has a LAN address: {lan:?}");
    let (s, _) = http_at(&lan, d.port, "GET", "/boot-status", "");
    assert_eq!(s, 200, "the daemon listens on the LAN address {lan}:{}", d.port);
    let range = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range").unwrap().split_whitespace().collect::<Vec<_>>().join(" ");
    struct PutBack(String);
    impl Drop for PutBack {
        fn drop(&mut self) {
            let st = Command::new("sudo").args(["-n", "sysctl", "-q", "-w", &format!("net.ipv4.ip_local_port_range={}", self.0)]).status();
            if !st.is_ok_and(|s| s.success()) {
                // Never a panic in a drop: loud, and the gate's own check
                // (agent-isolation-kernel-tests.sh) reads the range after.
                eprintln!("FAILED to put the ephemeral port range back to {}", self.0);
            }
        }
    }
    let st = Command::new("sudo").args(["-n", "sysctl", "-q", "-w", &format!("net.ipv4.ip_local_port_range={p} {p}")]).status().expect("sudo sysctl");
    assert!(st.success(), "set the ephemeral port range to {p}");
    let put_back = PutBack(range);
    let out = Command::new("python3")
        .args(["-c", r#"import errno, socket, sys
lan, port, p = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
IP_BIND_ADDRESS_NO_PORT = 24
def attempt(name, prep):
    s = socket.socket()
    try:
        prep(s)
        s.connect((lan, port))
        print(name, "port", s.getsockname()[1], "P" if s.getsockname()[1] == p else "other")
    except OSError as e:
        print(name, "refused", errno.errorcode.get(e.errno, e.errno))
    finally:
        s.close()
attempt("from_127_no_port", lambda s: (s.setsockopt(socket.IPPROTO_IP, IP_BIND_ADDRESS_NO_PORT, 1), s.bind(("127.0.0.1", 0))))
attempt("from_127_bind0", lambda s: s.bind(("127.0.0.1", 0)))
attempt("from_127_reuse", lambda s: (s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1), s.setsockopt(socket.IPPROTO_IP, IP_BIND_ADDRESS_NO_PORT, 1), s.bind(("127.0.0.1", 0))))"#, &lan, &d.port.to_string(), &p.to_string()])
        .output()
        .expect("python3");
    drop(put_back);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    println!("connects from 127.0.0.1 to {lan}:{} with the ephemeral range = {p} only, during the hold:\n{text}{}", d.port, String::from_utf8_lossy(&out.stderr));
    assert_eq!(text.lines().count(), 3, "three connect attempts: {text}");
    for l in text.lines() {
        assert!(!l.ends_with(" P"), "a connect from 127.0.0.1 got the held forward's port: {l}");
    }
    assert!(forwards_to(d.port).contains(&p), "A's forward was still held during the attempts");
}

/// `http` to another address of this box.
fn http_at(host: &str, port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let Ok(mut s) = TcpStream::connect((host, port)) else { return (0, String::new()) };
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    exchange(&mut s, &format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n"), body)
}


/// M1b: with the root helper placing sessions (the persistent unit is the
/// box's registered daemon), an agent's cgroup is root's: an UNFENCED agent
/// can't write its pid into a sibling's `cgroup.procs`, so a copied
/// passport stays refused. Run twice by
/// scripts/agent-isolation-m1b-kernel-tests.sh: `K2_ISO_EXPECT=root` (the
/// helper on) and, as the bite, `K2_ISO_EXPECT=user` (the helper moved
/// away, the daemon restarted): then the cgroups are the K2 user's and the
/// same write works, and the copied passport with it.
#[test]
#[ignore = "needs the persistent test unit and the root helper: scripts/agent-isolation-m1b-kernel-tests.sh"]
fn root_owned_agent_cgroups_refuse_a_sibling() {
    let expect_root = match std::env::var("K2_ISO_EXPECT").as_deref() {
        Ok("root") => true,
        Ok("user") => false,
        other => panic!("K2_ISO_EXPECT must be root or user, got {other:?}"),
    };
    let mut d = Daemon::external();
    let bs = d.boot_status();
    let iso = &bs["agentIsolation"];
    println!("agentIsolation: {iso}");
    if expect_root {
        assert_eq!(iso["identity"], "on", "the root helper places sessions: {iso}");
        assert!(iso["placement"].as_str().unwrap_or("").contains("root helper"), "{iso}");
    } else {
        assert_eq!(iso["identity"], "unavailable", "{iso}");
        assert_eq!(iso["traceable"], true, "{iso}");
    }
    let agents = Path::new("/sys/fs/cgroup").join(iso["agentsCgroup"].as_str().expect("agentsCgroup").trim_start_matches('/'));
    let tag = std::process::id();
    let (a_id, a_ws) = d.workspace(&format!("A{tag}"));
    let (b_id, b_ws) = d.workspace(&format!("B{tag}"));
    d.spawn("tab-b", &b_ws, &idle_script(&b_ws));
    let b_env = env_dump(&b_ws.join("env"));
    let b_pid: i32 = wait_for("B's pid", Duration::from_secs(30), || std::fs::read_to_string(b_ws.join("pid")).ok()?.trim().parse().ok());
    d.pids.push(b_pid);
    let b_cg = agents.join(format!("a-{b_id}"));
    assert!(cgroup_of(b_pid).ends_with(&format!("/agents/a-{b_id}")), "B runs in its own cgroup: {}", cgroup_of(b_pid));
    use std::os::unix::fs::MetadataExt;
    let owner = std::fs::metadata(&b_cg).unwrap().uid();
    let procs_owner = std::fs::metadata(b_cg.join("cgroup.procs")).unwrap().uid();
    println!("{}: owner uid {owner}, cgroup.procs uid {procs_owner}", b_cg.display());
    if expect_root {
        assert_eq!((owner, procs_owner), (0, 0), "B's cgroup is root's");
    } else {
        assert_ne!(owner, 0, "without the helper B's cgroup is the K2 user's");
    }
    // A, unfenced: B's passport copied (B's env, readable to the same user),
    // then the forging write into B's cgroup.procs, then the copied passport
    // on B's socket, then A dropping its own label.
    std::fs::write(a_ws.join("stolen"), format!("STOLEN_TOKEN='{}'\nSTOLEN_SOCK='{}'\n", b_env["K2_HOOK_TOKEN"], b_env["K2_HOOK_SOCK"])).unwrap();
    let script = format!(
        r#"R={a}/results; : > $R; echo $$ > {a}/pid; . {a}/stolen
echo "cgroup_before $(cat /proc/self/cgroup)" >> $R
{{ echo $$ > {bcg}/cgroup.procs; }} 2> {a}/e1; echo "forge_rc $? $(cat {a}/e1)" >> $R
echo "cgroup_after_forge $(cat /proc/self/cgroup)" >> $R
code=$(curl -s -o {a}/o1 -w '%{{http_code}}' --unix-socket "$STOLEN_SOCK" -H "Authorization: Bearer $STOLEN_TOKEN" http://localhost/cli/capabilities); echo "stolen_on_b_socket $code $(cut -c1-200 {a}/o1)" >> $R
{{ echo $$ > {agents}/cgroup.procs; }} 2> {a}/e2; echo "drop_rc $? $(cat {a}/e2)" >> $R
echo "cgroup_dropped $(cat /proc/self/cgroup)" >> $R
code=$(curl -s -o {a}/o2 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/capabilities); echo "own_after_drop $code $(cut -c1-200 {a}/o2)" >> $R
echo "done" >> $R; exec sleep 3600
"#,
        a = a_ws.display(),
        bcg = b_cg.display(),
        agents = agents.display()
    );
    d.spawn("tab-a", &a_ws, &script);
    let a_pid: i32 = wait_for("A's pid", Duration::from_secs(30), || std::fs::read_to_string(a_ws.join("pid")).ok()?.trim().parse().ok());
    d.pids.push(a_pid);
    let r = results(&a_ws.join("results"), "done");
    assert!(r["cgroup_before"].ends_with(&format!("/agents/a-{a_id}")), "A starts in its own cgroup: {}", r["cgroup_before"]);
    if expect_root {
        assert!(denied_at(&r["forge_rc"], "cgroup.procs"), "an unfenced agent can't write into a sibling's root-owned cgroup: {}", r["forge_rc"]);
        assert!(r["cgroup_after_forge"].ends_with(&format!("/agents/a-{a_id}")), "A is still A: {}", r["cgroup_after_forge"]);
        assert!(r["stolen_on_b_socket"].starts_with("403") && r["stolen_on_b_socket"].contains("passport_wrong_cgroup"), "B's copied passport from A: {}", r["stolen_on_b_socket"]);
    } else {
        // The bite: the same write works on a K2-user-owned cgroup, and the
        // copied passport with it (the M1 limit the helper closes).
        assert!(r["forge_rc"].starts_with("0"), "BITE: without the helper the forging write works: {}", r["forge_rc"]);
        assert!(r["cgroup_after_forge"].ends_with(&format!("/agents/a-{b_id}")), "BITE: A now wears B's label: {}", r["cgroup_after_forge"]);
        assert!(r["stolen_on_b_socket"].starts_with("200"), "BITE: B's copied passport works from A once A is in B's cgroup: {}", r["stolen_on_b_socket"]);
    }
    // Either way an unfenced agent can drop its own label, and its own
    // passport stops working (the claim says so).
    assert!(r["drop_rc"].starts_with("0"), "an unfenced agent can move itself to the shared leaf: {}", r["drop_rc"]);
    assert!(r["own_after_drop"].starts_with("403") && r["own_after_drop"].contains("passport_wrong_cgroup"), "its passport stops working outside its label: {}", r["own_after_drop"]);
    // whois says who owns the agent's cgroup.
    let (s, b) = http(d.port, "GET", &format!("/cli/agent-whois?pid={b_pid}&token={}", d.token()), "");
    let w: Value = serde_json::from_str(&b).unwrap_or_else(|e| panic!("whois {s}: {e}: {b}"));
    println!("whois B: {w}");
    assert_eq!(w["workspaceId"], json!(b_id), "{w}");
    assert_eq!(w["rootOwned"], json!(expect_root), "{w}");
    let audit = d.audit();
    if expect_root {
        assert!(audit.lines().any(|l| l.contains("agent.passport_wrong_cgroup") && l.contains(&format!("agent:{b_id}"))), "an audit row: B's passport from A");
    }
    // A job run of B's is placed like its terminals (root's cgroup with the
    // helper, the K2 user's without). The job is removed after.
    let token = d.token();
    let job = format!("m1b{tag}");
    let script = format!(
        "R={b}/job.results; : > $R; echo \"cgroup $(cat /proc/self/cgroup)\" >> $R; echo \"job done\" >> $R",
        b = b_ws.display()
    );
    let (s, body) = http(d.port, "POST", &format!("/cli/jobs/add?token={token}"), &json!({"workspace": b_id, "name": job, "every": "24h", "argv": ["sh", "-c", script]}).to_string());
    assert_eq!(s, 200, "jobs/add: {body}");
    let (s, body) = http(d.port, "POST", &format!("/cli/jobs/run?token={token}"), &json!({"workspace": b_id, "name": job}).to_string());
    assert_eq!(s, 200, "jobs/run: {body}");
    let jr = results(&b_ws.join("job.results"), "job done");
    let (s, body) = http(d.port, "POST", &format!("/cli/jobs/remove?token={token}"), &json!({"workspace": b_id, "name": job}).to_string());
    assert_eq!(s, 200, "jobs/remove: {body}");
    println!("B's job run: {}", jr["cgroup"]);
    assert!(jr["cgroup"].ends_with(&format!("/agents/a-{b_id}")), "B's job runs in B's cgroup: {}", jr["cgroup"]);
    let owner = std::fs::metadata(&b_cg).unwrap().uid();
    assert_eq!(owner == 0, expect_root, "B's cgroup (where its job ran) is root's exactly when the helper places: uid {owner}");
}

/// B4 pre-rows 4, P2 and P7 (design rev 3.1): what a fenced session can
/// reach through same-user sockets. RECORDS what happens (`row …` lines
/// for the review); fails only when a probe didn't run. Run from
/// `scripts/agent-isolation-b4-prerows.sh` (lingering on, so the user bus
/// exists; the owner's tmux server started by the script).
#[test]
#[ignore = "needs a delegated unit, a user manager and the owner's tmux: scripts/agent-isolation-b4-prerows.sh"]
fn b4_prerows_from_a_fenced_session() {
    let _unit = preflight();
    let bus = input("K2_ISO_USER_BUS").expect("K2_ISO_USER_BUS: the K2 user's bus socket (the script turns lingering on)");
    let tmux = input("K2_ISO_TMUX_SOCK").expect("K2_ISO_TMUX_SOCK: the owner's tmux socket (the script starts it)");
    assert!(std::fs::metadata(&bus).is_ok(), "no user bus at {}", bus.display());
    let mut d = Daemon::new("prerows");
    d.start();
    let (a_id, a_ws) = d.workspace("A");
    d.set_fence(&a_id, true);
    let ssh = d.home.join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("id_test"), "PRIVATE KEY\n").unwrap();
    let rt = bus.parent().expect("runtime dir");
    let script = format!(
        r#"R={a}/results; : > $R; echo $$ > {a}/pid
export XDG_RUNTIME_DIR={rt} DBUS_SESSION_BUS_ADDRESS=unix:path={bus}
cat {key} > /dev/null 2> {a}/e0; echo "own_read_key $? $(cat {a}/e0)" >> $R
busctl --system call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetId > {a}/o1 2>&1; echo "system_bus $? $(head -c 200 {a}/o1 | tr '\n' ' ')" >> $R
busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetId > {a}/o2 2>&1; echo "user_bus $? $(head -c 200 {a}/o2 | tr '\n' ' ')" >> $R
systemd-run --user --quiet --collect --wait --pipe -- sh -c 'echo cg=$(cut -d: -f3 /proc/self/cgroup); cat {key} > /dev/null 2>&1 && echo key=read || echo key=refused' > {a}/o3 2>&1; echo "user_run $? $(head -c 300 {a}/o3 | tr '\n' ' ')" >> $R
systemd-run --user --quiet --scope -- sh -c 'echo cg=$(cut -d: -f3 /proc/self/cgroup)' > {a}/o4 2>&1; echo "user_scope $? $(head -c 300 {a}/o4 | tr '\n' ' ')" >> $R
test -S {tmux}; echo "tmux_socket_visible $?" >> $R
tmux -S {tmux} new-window -d "sh -c 'cat {key} > /dev/null 2>&1 && echo read > {a}/p7 || echo refused > {a}/p7'" > {a}/o5 2>&1; echo "tmux_new_window $? $(head -c 200 {a}/o5 | tr '\n' ' ')" >> $R
sleep 2; echo "tmux_window_ran $(cat {a}/p7 2>/dev/null || echo none)" >> $R
echo "done" >> $R; exec sleep 3600
"#,
        a = a_ws.display(),
        rt = rt.display(),
        bus = bus.display(),
        key = ssh.join("id_test").display(),
        tmux = tmux.display(),
    );
    d.spawn("tab-a", &a_ws, &script);
    let r = results(&a_ws.join("results"), "done");
    let a_pid: i32 = std::fs::read_to_string(a_ws.join("pid")).unwrap().trim().parse().unwrap();
    d.pids.push(a_pid);
    assert!(denied(&r["own_read_key"]), "the session is fenced (it can't read the key itself): {}", r["own_read_key"]);
    println!("row 4a: a fenced session on the system bus (GetId): {}", r["system_bus"]);
    println!("row 4b: a fenced session on the user bus (GetId): {}", r["user_bus"]);
    println!("row P2a: systemd-run --user from a fenced session: {}", r["user_run"]);
    println!("row P2b: systemd-run --user --scope from a fenced session: {}", r["user_scope"]);
    println!("row P7a: the owner's tmux socket visible to the fenced session (0 = yes): {}", r["tmux_socket_visible"]);
    println!("row P7b: tmux new-window on the owner's server: {} · the window read the key: {}", r["tmux_new_window"], r["tmux_window_ran"]);
}

/// Two fenced sessions, each with the secrets folder the vault made for it
/// (`$K2_SECRETS_DIR`): B can't read A's folder (a secret file and the
/// passport the test puts there), A reads its own. A caller's own
/// `K2_SECRETS_DIR` in the spawn request names nothing (review V2).
/// (Connecting to another folder's ssh-agent socket is the Unix-socket
/// block's, not this test's.)
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn a_fenced_session_reads_no_other_sessions_secrets() {
    let _unit = preflight();
    let mut d = Daemon::new("secrets");
    d.start();
    let (a_id, a_ws) = d.workspace("A");
    let (b_id, b_ws) = d.workspace("B");
    d.set_fence(&a_id, true);
    d.set_fence(&b_id, true);
    // A: names its folder, then waits for the test's files and reads its own.
    let a_script = format!(
        r#"R={a}/results; : > $R; echo $$ > {a}/pid
echo "secrets_dir $K2_SECRETS_DIR" >> $R
while [ ! -f {a}/go ]; do sleep 0.2; done
cat "$K2_SECRETS_DIR/MY_KEY" > {a}/o1 2>&1; echo "own_secret $? $(head -c 120 {a}/o1)" >> $R
echo "done" >> $R; exec sleep 3600
"#,
        a = a_ws.display(),
    );
    d.spawn("tab-a", &a_ws, &a_script);
    let ra = results(&a_ws.join("results"), "secrets_dir");
    let a_dir = PathBuf::from(ra["secrets_dir"].trim());
    assert!(a_dir.is_absolute() && a_dir.is_dir(), "the vault made A a secrets folder: {:?}", ra["secrets_dir"]);
    std::fs::write(a_dir.join("MY_KEY"), "A SECRET\n").unwrap();
    std::fs::write(a_dir.join(".k2-passport"), "A PASSPORT\n").unwrap();
    std::fs::write(a_ws.join("go"), "").unwrap();
    // B: asks for A's folder in its own request too; reads A's files.
    let b_script = format!(
        r#"R={b}/results; : > $R; echo $$ > {b}/pid
echo "secrets_dir $K2_SECRETS_DIR" >> $R
cat {o}/MY_KEY > {b}/o1 2>&1; echo "other_secret $? $(head -c 120 {b}/o1)" >> $R
cat {o}/.k2-passport > {b}/o2 2>&1; echo "other_passport $? $(head -c 120 {b}/o2)" >> $R
ls {o} > {b}/o3 2>&1; echo "other_list $? $(head -c 120 {b}/o3 | tr '\n' ' ')" >> $R
echo "done" >> $R; exec sleep 3600
"#,
        b = b_ws.display(),
        o = a_dir.display(),
    );
    d.spawn_env("tab-b", &b_ws, &b_script, json!({ "K2_SECRETS_DIR": a_dir.display().to_string() }));
    let rb = results(&b_ws.join("results"), "done");
    let ra = results(&a_ws.join("results"), "done");
    for (ws, pid) in [(&a_ws, "pid"), (&b_ws, "pid")] {
        d.pids.push(std::fs::read_to_string(ws.join(pid)).unwrap().trim().parse().unwrap());
    }
    println!("secrets: A {ra:?} B {rb:?}");
    assert_ne!(rb["secrets_dir"].trim(), a_dir.display().to_string(), "B's folder is the vault's, not the one its request named");
    for row in ["other_secret", "other_passport", "other_list"] {
        assert!(denied(&rb[row]), "{row}: another session's folder can't be read: {}", rb[row]);
    }
    assert!(ra["own_secret"].starts_with("0 A SECRET"), "A reads its own folder: {}", ra["own_secret"]);
}

/// Review V1: a link a fenced session plants in /dev/shm (writable for it)
/// is never a rule for the next fenced start, so that start can't read what
/// it points at (the owner's keys).
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn a_link_planted_in_dev_shm_opens_nothing_at_the_next_start() {
    let _unit = preflight();
    // SAFETY: no preconditions.
    let uid = unsafe { libc::geteuid() };
    // The /dev/shm secrets root exists, so /dev/shm is carved at each start.
    std::fs::create_dir_all(format!("/dev/shm/k2-{uid}")).unwrap();
    let link = PathBuf::from(format!("/dev/shm/k2isotest-link-{}", std::process::id()));
    let _ = std::fs::remove_file(&link);
    let mut d = Daemon::new("shmlink");
    d.start();
    let (a_id, a_ws) = d.workspace("A");
    d.set_fence(&a_id, true);
    let ssh = d.home.join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("id_test"), "PRIVATE KEY\n").unwrap();
    let plant = format!(
        r#"R={a}/r1; : > $R; echo $$ > {a}/pid1
ln -s {ssh} {l} 2> {a}/e1; echo "planted $? $(cat {a}/e1)" >> $R
echo "done" >> $R; exec sleep 3600
"#,
        a = a_ws.display(),
        ssh = ssh.display(),
        l = link.display(),
    );
    d.spawn("tab-a1", &a_ws, &plant);
    let r1 = results(&a_ws.join("r1"), "done");
    d.pids.push(std::fs::read_to_string(a_ws.join("pid1")).unwrap().trim().parse().unwrap());
    let read = format!(
        r#"R={a}/r2; : > $R; echo $$ > {a}/pid2
cat {l}/id_test > {a}/o1 2>&1; echo "through_link $? $(head -c 120 {a}/o1)" >> $R
echo x > {l}/planted 2> {a}/e2; echo "write_through_link $? $(cat {a}/e2)" >> $R
echo "done" >> $R; exec sleep 3600
"#,
        a = a_ws.display(),
        l = link.display(),
    );
    d.spawn("tab-a2", &a_ws, &read);
    let r2 = results(&a_ws.join("r2"), "done");
    d.pids.push(std::fs::read_to_string(a_ws.join("pid2")).unwrap().trim().parse().unwrap());
    let _ = std::fs::remove_file(&link);
    println!("shm link: {r1:?} {r2:?}");
    assert!(r1["planted"].starts_with('0'), "a fenced session can make a link in /dev/shm (the case this guards): {}", r1["planted"]);
    assert!(denied(&r2["through_link"]), "the next fenced start can't read through it: {}", r2["through_link"]);
    assert!(!r2["write_through_link"].starts_with('0') && !ssh.join("planted").exists(), "nor write: {}", r2["write_through_link"]);
}

// ── M2: the Unix-socket block (review-agent-isolation-m2-unix-socket-block) ──

/// What a fenced session's probes run (python3 on the box): one `key value`
/// line each. Args: temp folder, user bus, owner's tmux socket, the
/// supervisor's pid, the daemon's port.
const SOCKBLOCK_PROBE: &str = r#"import ctypes, os, platform, socket, sys
libc = ctypes.CDLL(None, use_errno=True)
NR = {"x86_64": {"seccomp": 317, "uring": 425}, "aarch64": {"seccomp": 277, "uring": 425}}[platform.machine()]
def row(k, v): print(f"{k} {v}", flush=True)
def err(): return ctypes.get_errno()
def conn(path):
    s = socket.socket(socket.AF_UNIX)
    try:
        s.connect(path); return "ok"
    except OSError as e:
        return f"err {e.errno}"
    finally:
        s.close()
tmp, bus, tmux, sup, port = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
p = os.path.join(tmp, "own.sock")
if os.path.lexists(p): os.unlink(p)
srv = socket.socket(socket.AF_UNIX); srv.bind(p); srv.listen(8)
row("tmp_socket", conn(p))
l = os.path.join(tmp, "buslink")
if os.path.lexists(l): os.unlink(l)
os.symlink(bus, l)
row("tmp_link", conn(l))
up = tmp + "/.." * (tmp.strip("/").count("/") + 1) + bus
row("tmp_dotdot", conn(up))
h = os.path.join(tmp, "hard")
try:
    os.link(tmux, h); row("tmp_hardlink_made", "yes"); row("tmp_hardlink", conn(h))
except OSError as e:
    row("tmp_hardlink_made", f"err {e.errno}")
try:
    socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM).close(); row("dgram", "ok")
except OSError as e:
    row("dgram", f"err {e.errno}")
params = ctypes.create_string_buffer(120)
r = libc.syscall(NR["uring"], 1, params)
row("uring", "ok" if r >= 0 else f"err {err()}")
pid = os.fork()
if pid == 0:
    class F(ctypes.Structure): _fields_ = [("code", ctypes.c_ushort), ("jt", ctypes.c_ubyte), ("jf", ctypes.c_ubyte), ("k", ctypes.c_uint)]
    class P(ctypes.Structure): _fields_ = [("len", ctypes.c_ushort), ("filter", ctypes.POINTER(F))]
    f = F(6, 0, 0, 0x7fff0000); prog = P(1, ctypes.pointer(f))
    libc.prctl(38, 1, 0, 0, 0)
    r = libc.syscall(NR["seccomp"], 1, 8, ctypes.byref(prog))
    os._exit(0 if r >= 0 else err())
_, st = os.waitpid(pid, 0)
rc = os.waitstatus_to_exitcode(st)
row("s2_listener", "ok" if rc == 0 else f"err {rc}")
libc.ptrace.restype = ctypes.c_long
r = libc.ptrace(16, sup, None, None)
row("ptrace_supervisor", "ok" if r == 0 else f"err {err()}")
if r == 0: libc.ptrace(17, sup, None, None)
class IOV(ctypes.Structure): _fields_ = [("base", ctypes.c_void_p), ("len", ctypes.c_size_t)]
buf = ctypes.create_string_buffer(8)
loc = IOV(ctypes.cast(buf, ctypes.c_void_p), 8); rem = IOV(0x400000, 8)
libc.process_vm_readv.restype = ctypes.c_ssize_t
libc.process_vm_readv.argtypes = [ctypes.c_int, ctypes.POINTER(IOV), ctypes.c_ulong, ctypes.POINTER(IOV), ctypes.c_ulong, ctypes.c_ulong]
r = libc.process_vm_readv(sup, ctypes.byref(loc), 1, ctypes.byref(rem), 1, 0)
row("vm_read_supervisor", "ok" if r >= 0 else f"err {err()}")
for name in ["environ", "mem"]:
    try:
        open(f"/proc/{sup}/{name}", "rb").read(1); row(f"{name}_supervisor", "ok")
    except OSError as e:
        row(f"{name}_supervisor", f"err {e.errno}")
try:
    os.listdir(f"/proc/{sup}/fd"); row("fds_supervisor", "ok")
except OSError as e:
    row("fds_supervisor", f"err {e.errno}")
s = socket.socket()
try:
    s.connect(("127.0.0.1", port)); row("tcp", "ok")
except OSError as e:
    row("tcp", f"err {e.errno}")
"#;

/// The shell side of the block's rows in a fenced session (rows 4, P2, P7
/// and the review's section 7), then `probe.py`, then (when `prlimit`) the
/// supervisor's open-file limit set to zero and one more connect.
fn sockblock_script(a: &Path, cli: &Path, bus: &Path, tmux: &Path, port: u16, prlimit: bool) -> String {
    let rt = bus.parent().expect("runtime dir");
    format!(
        r#"R={a}/results; : > $R; echo $$ > {a}/pid; echo $PPID > {a}/ppid
export XDG_RUNTIME_DIR={rt} DBUS_SESSION_BUS_ADDRESS=unix:path={bus}
echo "tmpdir $TMPDIR" >> $R; echo "hook_sock $K2_HOOK_SOCK" >> $R
echo "supervisor_comm $(cat /proc/$PPID/comm)" >> $R
busctl --system call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetId > {a}/o1 2>&1; echo "system_bus $? $(head -c 200 {a}/o1 | tr '\n' ' ')" >> $R
busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetId > {a}/o2 2>&1; echo "user_bus $? $(head -c 200 {a}/o2 | tr '\n' ' ')" >> $R
systemd-run --user --quiet --collect --wait --pipe -- true > {a}/o3 2>&1; echo "user_run $? $(head -c 300 {a}/o3 | tr '\n' ' ')" >> $R
systemd-run --user --quiet --scope -- true > {a}/o4 2>&1; echo "user_scope $? $(head -c 300 {a}/o4 | tr '\n' ' ')" >> $R
tmux -S {tmux} new-window -d "touch {a}/p7" > {a}/o5 2>&1; echo "tmux_new_window $? $(head -c 200 {a}/o5 | tr '\n' ' ')" >> $R
sleep 2; test -e {a}/p7; echo "tmux_window_ran $?" >> $R
code=$(curl -s -o {a}/o6 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/capabilities); echo "hook $code" >> $R
bash {cli} whoami --json > {a}/o7 2>&1; echo "whoami $? $(head -c 200 {a}/o7 | tr '\n' ' ')" >> $R
( ( sleep 1; curl -s -o /dev/null -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/capabilities > {a}/df 2>&1 ) & )
sleep 3; echo "doublefork $(cat {a}/df 2>/dev/null || echo none)" >> $R
if command -v node > /dev/null; then
  node -e "const n=require('net');const c=n.connect({{path:process.env.K2_HOOK_SOCK}});c.on('connect',()=>{{console.log('ok');process.exit(0)}});c.on('error',e=>{{console.log('err '+e.code);process.exit(1)}})" > {a}/o8 2>&1; echo "node_hook $(head -c 100 {a}/o8)" >> $R
  node -e "const n=require('net');const c=n.connect({{port:{port},host:'127.0.0.1'}});c.on('connect',()=>{{console.log('ok');process.exit(0)}});c.on('error',e=>{{console.log('err '+e.code);process.exit(1)}})" > {a}/o9 2>&1; echo "node_tcp $(head -c 100 {a}/o9)" >> $R
else
  echo "node_hook no-node" >> $R; echo "node_tcp no-node" >> $R
fi
python3 {a}/probe.py "$TMPDIR" {bus} {tmux} $PPID {port} >> $R 2> {a}/probe.err; echo "probe_rc $? $(head -c 300 {a}/probe.err | tr '\n' ' ')" >> $R
if [ {prlimit} = 1 ]; then
  prlimit --pid $PPID --nofile=0:0 > {a}/o10 2>&1; echo "prlimit $? $(head -c 200 {a}/o10)" >> $R
  code=$(curl -s -o /dev/null -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/capabilities); echo "after_prlimit_hook $code" >> $R
  code=$(curl -s -o /dev/null -w '%{{http_code}}' http://127.0.0.1:{port}/boot-status); echo "after_prlimit_tcp $code" >> $R
fi
echo "done" >> $R; exec sleep 3600
"#,
        a = a.display(),
        rt = rt.display(),
        bus = bus.display(),
        tmux = tmux.display(),
        cli = cli.display(),
        port = port,
        prlimit = u8::from(prlimit),
    )
}

/// A fenced session of a new workspace `name` running [`sockblock_script`];
/// its results, its pid and its supervisor's.
fn sockblock_session(d: &mut Daemon, name: &str, bus: &Path, tmux: &Path, prlimit: bool) -> (HashMap<String, String>, PathBuf, i32, i32) {
    let (id, ws) = d.workspace(name);
    d.set_fence(&id, true);
    std::fs::write(ws.join("probe.py"), SOCKBLOCK_PROBE).unwrap();
    let script = sockblock_script(&ws, &d.k2.join("bin/k2"), bus, tmux, d.port, prlimit);
    d.spawn(&format!("tab-{name}"), &ws, &script);
    let r = results(&ws.join("results"), "done");
    let pid: i32 = std::fs::read_to_string(ws.join("pid")).unwrap().trim().parse().unwrap();
    let sup: i32 = std::fs::read_to_string(ws.join("ppid")).unwrap().trim().parse().unwrap();
    d.pids.extend([pid, sup]);
    (r, ws, pid, sup)
}

fn rc_of(v: &str) -> &str {
    v.split_whitespace().next().unwrap_or("")
}

/// The block in ENFORCE mode (`K2_AGENT_SOCKBLOCK=enforce`), written
/// fail-first from rows 4, P2 and P7: a fenced session can't reach the
/// system or the user bus, can't `systemd-run --user`, can't drive the
/// owner's tmux; it still reaches its own hook socket (curl, `k2 whoami`,
/// node) and a socket it made in its own temp folder. Plus the review's
/// section 7 rows a session can check: its own listener refused (S2),
/// io_uring `ENOSYS`, the link / `..` / hard-link forms refused, datagram
/// sockets refused, a double-forked child connects, no descriptor above 2,
/// the supervisor non-dumpable (ptrace, memory, environ, fds refused) and,
/// last, its open-file limit set to zero: then even the hook socket is
/// refused. Then the supervisor killed: the session ends.
#[test]
#[ignore = "needs a delegated unit, a user manager and the owner's tmux: scripts/agent-isolation-b4-prerows.sh (its K2_ISO_USER_BUS and K2_ISO_TMUX_SOCK)"]
fn sockblock_enforced_rows_4_p2_p7_and_section_7() {
    let _unit = preflight();
    let bus = input("K2_ISO_USER_BUS").expect("K2_ISO_USER_BUS: the K2 user's bus socket (lingering on)");
    let tmux = input("K2_ISO_TMUX_SOCK").expect("K2_ISO_TMUX_SOCK: the owner's tmux socket (the script starts it)");
    assert!(std::fs::metadata(&bus).is_ok(), "no user bus at {}", bus.display());
    let mut d = Daemon::new("sbenf");
    d.env.push(("K2_AGENT_SOCKBLOCK".into(), "enforce".into()));
    d.start();
    let st = d.boot_status();
    assert_eq!(st["agentIsolation"]["sockblock"]["mode"], "enforce", "/boot-status says the block's mode: {}", st["agentIsolation"]["sockblock"]);
    assert_eq!(st["agentIsolation"]["sockblock"]["kernel"], "ok", "{}", st["agentIsolation"]["sockblock"]);
    let (r, _ws, pid, sup) = sockblock_session(&mut d, "A", &bus, &tmux, true);
    println!("sockblock enforce: {r:?}");
    // The supervisor is the program's parent, in the same cgroup.
    assert_eq!(r["supervisor_comm"], "k2-daemon", "the exec helper stays as the supervisor");
    assert_eq!(cgroup_of(sup), cgroup_of(pid), "the supervisor is in the session's cgroup");
    // Rows 4, P2, P7: refused now.
    for row in ["system_bus", "user_bus", "user_run", "user_scope", "tmux_new_window"] {
        assert_ne!(rc_of(&r[row]), "0", "{row} is refused under the block: {}", r[row]);
    }
    assert_ne!(r["tmux_window_ran"], "0", "the owner's tmux ran nothing");
    // What it still reaches.
    assert_eq!(r["hook"], "200", "its own hook socket");
    assert!(r["whoami"].starts_with("0 ") || r["whoami"].contains("cannot determine identity"), "k2 whoami over its socket: {}", r["whoami"]);
    assert_eq!(r["doublefork"], "200", "a double-forked child still connects (row 7)");
    assert_eq!(r["tmp_socket"], "ok", "a socket it made in its own temp folder (row 3; through /proc/self/fd, row 4)");
    assert_eq!(r["tcp"], "ok", "TCP");
    if r["node_hook"] != "no-node" {
        assert_eq!((r["node_hook"].as_str(), r["node_tcp"].as_str()), ("ok", "ok"), "an event-loop client (row 10)");
    }
    // Section 7.
    assert_eq!(rc_of(&r["probe_rc"]), "0", "the probes ran: {}", r["probe_rc"]);
    assert_eq!(r["tmp_link"], "err 111", "a link in its temp folder to the bus (row 3)");
    assert_eq!(r["tmp_dotdot"], "err 111", ".. out of its temp folder (row 3)");
    if r["tmp_hardlink_made"] == "yes" {
        assert_eq!(r["tmp_hardlink"], "err 111", "a hard link of the owner's tmux socket");
    }
    println!("row 3 (hard link of the owner's tmux socket into the temp folder): {}", r["tmp_hardlink_made"]);
    assert_eq!(r["dgram"], "err 1", "Unix datagram sockets can't be made (row 5)");
    assert_eq!(r["uring"], "err 38", "io_uring_setup: ENOSYS (row 2)");
    assert_eq!(r["s2_listener"], "err 1", "its own notification listener: refused (row 1)");
    for row in ["ptrace_supervisor", "vm_read_supervisor", "environ_supervisor", "mem_supervisor", "fds_supervisor"] {
        assert!(r[row].starts_with("err "), "{row}: the supervisor is non-dumpable (row 8): {}", r[row]);
    }
    // Row 9: nothing above 2 in the program (read from outside).
    let fds: Vec<String> = std::fs::read_dir(format!("/proc/{pid}/fd")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(fds.iter().all(|f| matches!(f.as_str(), "0" | "1" | "2")), "the program holds only 0-2: {fds:?}");
    // Row 8: prlimit to zero, then nothing opens.
    assert_eq!(rc_of(&r["prlimit"]), "0", "prlimit on the supervisor is allowed for the same user: {}", r["prlimit"]);
    assert_eq!(r["after_prlimit_hook"], "000", "then even the hook socket is refused");
    assert_eq!(r["after_prlimit_tcp"], "000", "and TCP");
    let logs = sockblock_logs(&d.k2);
    println!("──── sockblock.log ────\n{logs}");
    assert!(logs.contains("\"applied\":\"refused\"") && logs.contains("/bus"), "the refusals are in the owner's log");
    // The supervisor killed: the session ends (the program goes with it).
    let (_, _, pid_b, sup_b) = sockblock_session(&mut d, "B", &bus, &tmux, false);
    // SAFETY: the test's own session's supervisor.
    unsafe { libc::kill(sup_b, libc::SIGKILL) };
    wait_for("the program to end with its supervisor", Duration::from_secs(10), || (!Path::new(&format!("/proc/{pid_b}")).exists()).then_some(()));
}

/// Every `sockblock.log` under `~/.k2/agents`.
fn sockblock_logs(k2: &Path) -> String {
    let mut out = String::new();
    for e in std::fs::read_dir(k2.join("agents")).into_iter().flatten().flatten() {
        if let Ok(t) = std::fs::read_to_string(e.path().join("sockblock.log")) {
            out.push_str(&t);
        }
    }
    out
}

/// Record mode (the default) lets the same session through and logs what
/// enforce would refuse; with the block off (the bite) the session has no
/// supervisor and its own listener filter goes in.
#[test]
#[ignore = "needs a delegated unit, a user manager and the owner's tmux: scripts/agent-isolation-b4-prerows.sh (its K2_ISO_USER_BUS and K2_ISO_TMUX_SOCK)"]
fn sockblock_record_logs_and_off_is_the_bite() {
    let _unit = preflight();
    let bus = input("K2_ISO_USER_BUS").expect("K2_ISO_USER_BUS");
    let tmux = input("K2_ISO_TMUX_SOCK").expect("K2_ISO_TMUX_SOCK");
    let mut d = Daemon::new("sbrec");
    d.start();
    assert_eq!(d.boot_status()["agentIsolation"]["sockblock"]["mode"], "record", "record is the default");
    let (r, _, _, _) = sockblock_session(&mut d, "A", &bus, &tmux, false);
    println!("sockblock record: {r:?}");
    assert_eq!(r["supervisor_comm"], "k2-daemon");
    assert_eq!(rc_of(&r["user_bus"]), "0", "record lets the user bus through: {}", r["user_bus"]);
    assert_eq!(r["hook"], "200");
    assert_eq!(r["tmp_link"], "ok", "record lets the link through");
    let logs = sockblock_logs(&d.k2);
    println!("──── sockblock.log ────\n{logs}");
    let lines: Vec<Value> = logs.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}"))).collect();
    assert!(lines.iter().any(|l| l["event"] == "start" && l["mode"] == "record"), "a start line");
    let bus_s = bus.display().to_string();
    let bus_line = lines.iter().find(|l| l["path"].as_str() == Some(bus_s.as_str())).expect("a line for the user bus");
    assert_eq!((bus_line["verdict"].as_str(), bus_line["applied"].as_str()), (Some("refuse"), Some("allowed")), "{bus_line}");
    assert!(lines.iter().any(|l| l["path"].as_str().is_some_and(|p| p.ends_with(".sock")) && l["verdict"] == "allow"), "its hook socket: allow");
    drop(d);
    let mut off = Daemon::new("sboff");
    off.env.push(("K2_AGENT_SOCKBLOCK".into(), "off".into()));
    off.start();
    assert_eq!(off.boot_status()["agentIsolation"]["sockblock"]["mode"], "off");
    let (r, _, _, _) = sockblock_session(&mut off, "A", &bus, &tmux, false);
    println!("sockblock off: {r:?}");
    assert_ne!(r["supervisor_comm"], "k2-daemon", "no supervisor with the block off");
    assert_eq!(rc_of(&r["user_bus"]), "0", "the bite: the user bus is reachable without the block");
    assert_eq!(r["s2_listener"], "ok", "the bite: its own listener goes in without the block");
    assert_eq!(r["dgram"], "ok");
}

/// Inventory, record only (review section 5): what common tools try to
/// reach from a fenced session. Asserts only that the probes ran; prints
/// each probe's result and every decision the log saw.
#[test]
#[ignore = "needs a delegated cgroup v2 system unit on Linux: scripts/agent-isolation-kernel-tests.sh"]
fn sockblock_inventory_record_only() {
    let _unit = preflight();
    let mut d = Daemon::new("sbinv");
    d.start();
    let (id, ws) = d.workspace("A");
    d.set_fence(&id, true);
    let script = format!(
        r#"R={a}/results; : > $R; echo $$ > {a}/pid; cd {a}
p() {{ k=$1; shift; timeout 60 "$@" > {a}/out.$k 2>&1; echo "$k $? $(head -c 200 {a}/out.$k | tr '\n' ' ')" >> $R; }}
p id id
p getent_passwd getent passwd "$(id -un)"
p getent_hosts getent hosts example.com
p git_ls_remote git ls-remote https://github.com/git/git HEAD
if command -v gpg-connect-agent > /dev/null; then p gpg_agent gpg-connect-agent /bye; else echo "gpg_agent absent" >> $R; fi
p ssh_control_home ssh -o BatchMode=yes -S "$HOME/.ssh/cm-k2iso" -O check localhost
p ssh_control_tmp ssh -o BatchMode=yes -S "$TMPDIR/cm-k2iso" -O check localhost
git init -q fsm && git -C fsm config core.fsmonitor true && p git_fsmonitor git -C fsm status
git -C fsm fsmonitor--daemon stop > /dev/null 2>&1
B=""; for b in chromium chromium-browser google-chrome; do command -v $b > /dev/null && B=$b && break; done
if [ -n "$B" ]; then p chromium $B --headless=new --no-sandbox --user-data-dir="$TMPDIR/chr" --dump-dom about:blank; else echo "chromium absent" >> $R; fi
B32=""
for f in /usr/bin/* /usr/lib32/*; do [ -f "$f" ] && [ -x "$f" ] && [ "$(head -c 5 "$f" | od -An -tx1 | tr -d ' \n')" = "7f454c4601" ] && B32=$f && break; done
if [ -n "$B32" ]; then p bin32 "$B32" --version; echo "bin32_path $B32" >> $R; else echo "bin32 absent" >> $R; fi
echo "done" >> $R; exec sleep 3600
"#,
        a = ws.display(),
    );
    d.spawn("tab-a", &ws, &script);
    let r = results(&ws.join("results"), "done");
    d.pids.push(std::fs::read_to_string(ws.join("pid")).unwrap().trim().parse().unwrap());
    let mut keys: Vec<_> = r.keys().collect();
    keys.sort();
    for k in keys {
        println!("inventory {k}: {}", r[k]);
    }
    let logs = sockblock_logs(&d.k2);
    println!("──── sockblock.log (record) ────\n{logs}");
    assert!(logs.contains("\"event\":\"start\""), "the session ran under the block in record mode");
}
