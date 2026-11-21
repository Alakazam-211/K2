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
            ["K2_ISO_DAEMON_BIN", "K2_PTYD_BIN", "K2_PTYD_TEST_AGENT", "K2_ISO_CLI", "K2_ISO_TOOLCHAIN_HOME"]
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
        let ptyd = std::fs::read_to_string(self.k2.join("run/ptyd/lock")).ok().and_then(|s| s.trim().parse::<i32>().ok());
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
        Daemon { home, k2, child: None, boots: 0, port: 0, pids: Vec::new(), env: Vec::new() }
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
code=$(curl -s -o o5 -w '%{{http_code}}' --unix-socket "$K2_HOOK_SOCK" -H "Authorization: Bearer $K2_HOOK_TOKEN" http://localhost/cli/whoami); echo "own_on_own_socket $code $(cat o5 | cut -c1-300)" >> $R
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
    assert_eq!(bs["agentIsolation"]["identity"], "on", "identity on in a delegated, guarded unit: {}", bs["agentIsolation"]);
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
        format!("STOLEN_TOKEN='{}'\nSTOLEN_SOCK='{}'\nPORT={}\nOWNER='{}'\n", b_env["K2_HOOK_TOKEN"], b_env["K2_HOOK_SOCK"], d.port, d.token()),
    )
    .unwrap();
    let r = results(&a_ws.join("results"), "phase2 done");
    let code = |k: &str| r[k].split_whitespace().next().unwrap_or("").to_string();
    assert!(code("stolen_on_its_socket") == "403" && r["stolen_on_its_socket"].contains("passport_wrong_cgroup"), "{}", r["stolen_on_its_socket"]);
    assert!(code("stolen_on_its_socket_via_tcp") == "403" && r["stolen_on_its_socket_via_tcp"].contains("passport_wrong_cgroup"), "{}", r["stolen_on_its_socket_via_tcp"]);
    assert!(code("stolen_over_tcp") == "403" && r["stolen_over_tcp"].contains("passport_needs_socket"), "{}", r["stolen_over_tcp"]);
    assert_eq!(code("stolen_on_own_socket"), "403", "B's passport on A's socket: {}", r["stolen_on_own_socket"]);
    let own = r["own_on_own_socket"].split_once(' ').map(|(c, b)| (c.parse::<u16>().unwrap_or(0), b.to_string())).unwrap();
    assert!(served(own.0, &own.1), "A's own passport on its own socket works: {}", r["own_on_own_socket"]);
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
