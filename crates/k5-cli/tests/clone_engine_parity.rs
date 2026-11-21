//! `k5-cli clone` against `cli/k2 clone` for the engine's verbs
//! (`preview`, `to`, `from`, `here`, `approve`, a bare `<ws> --dest`):
//! the same requests to this computer's daemon, its vault socket and a
//! remote server, the same stdout, stderr and exit, and the same files
//! left in the K2 home's clone-tmp. Each side runs again with no
//! `python3` on `PATH` and must answer as it does with one.
//!
//! Its own rig, as [`common::parity::Rig`] has no room for a second
//! server, the vault socket's http.client requests (no `User-Agent`) or
//! the engine's random clone id: each side has its own HOME (so its own
//! vault socket, the daemon stub's socket) and a remote stub reached
//! over plain http to a loopback address (the rule lets a token go
//! there). Masked, and only these: the `User-Agent` and `X-K5-Cli-Via`
//! values, `X-K5-Cli-Version` (the binary's) and `X-K5-Cli-Noun` (the
//! shell's), each side's own folder, the clone id and the upload id.
//! No side has a terminal (each runs in a new session); the terminal's
//! rows are tests/cli/clone_security.sh's pty driver, which cli-parity
//! runs on both.
//!
//! For `https://`: each stub has a TLS front ([`TlsFront`], the
//! certificate a throwaway CA's, trusted by `SSL_CERT_FILE` on both
//! sides), the remote's name [`tunnel::HOST`] (never resolves) and this
//! server's `localhost`; the person's proxy is a recording CONNECT proxy
//! that tunnels to the front (or refuses). Both sides read the proxy as
//! the shell's Python does and write `http.client`'s CONNECT head (K2's
//! ruling: one proxy rule for every urllib-style family), so the heads
//! are compared byte for byte (bash's as Python 3.12 writes them: the
//! named difference python-connect-host), besides counted and read for
//! secrets. A bite runs
//! the rows against a stand-in binary (`CLONE_RIG_BIN`).

#![cfg(unix)]

mod common;

use common::parity::{cli_k2, fold_query_escapes, run_within, BIN, OWNER};
use common::stub::{Logged, Reply, Stub};
use common::tunnel::{self, Pki, ProxyMode, ProxyStub, TlsFront};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static N: AtomicUsize = AtomicUsize::new(0);

const SIDES: [&str; 3] = ["bash", "rust", "nopy"];

struct Side {
    dir: PathBuf,
    home: PathBuf,
    k2home: PathBuf,
    local: Stub,
    remote: Stub,
    /// `https://` in front of each stub (the remote's name is
    /// [`tunnel::HOST`], this server's `localhost`).
    remote_tls: TlsFront,
    local_tls: TlsFront,
    /// A relay on `[::1]` to this server's stub; none where this machine
    /// has no IPv6 loopback.
    v6: Option<u16>,
}

struct CRig {
    sides: Vec<Side>,
    no_python3: String,
    /// The CA every side trusts (`SSL_CERT_FILE`), and one no side does.
    pki: Pki,
    other: Pki,
    /// The person's proxy: one that opens the tunnel, one that refuses
    /// it (407). Shared by the sides, its log taken after each run.
    proxy: ProxyStub,
    refuse: ProxyStub,
}

/// Where `heartbeat.port` points this server's side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PortTo {
    Stub,
    Tls,
    V6,
}

/// Which stub a case waits on before its Ctrl-C.
#[derive(Clone, Copy, PartialEq, Eq)]
enum At {
    Local,
    Remote,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Exact,
    /// The shell's Python raises: stderr up to the traceback and its last
    /// line's exception name.
    Traceback,
    /// A failed TLS handshake: the reason's words are Python's `ssl`
    /// module's or the binary's own (the named difference
    /// python-tls-error-text), so each side's `[SSL…` reason is masked.
    TlsError,
}

struct CCase {
    name: &'static str,
    /// `{remote}` is the side's remote stub (`http://127.0.0.1:<port>`);
    /// `{remote_big}` the same with the port 65536 past it, which glibc
    /// would cut back to the stub's.
    args: Vec<&'static str>,
    env: Vec<(&'static str, &'static str)>,
    stdin: Option<&'static [u8]>,
    /// `{dir}` in a body is the side's folder.
    local: Vec<Reply>,
    remote: Vec<Reply>,
    /// No vault socket on this computer.
    no_vault: bool,
    /// Files made in the side's folder first: (name, bytes).
    files: Vec<(&'static str, &'static [u8])>,
    /// `connect-tokens.json` in the K2 home.
    tokens: Option<&'static str>,
    /// A file of this many bytes made in the side's folder first.
    big: Option<(&'static str, usize)>,
    port_to: PortTo,
    /// CONNECTs the person's proxy gets from each side; none of them may
    /// hold any of `secrets`.
    proxy_heads: usize,
    secrets: Vec<&'static str>,
    /// Ctrl-C (SIGINT to the side's process group) once this stub holds
    /// this many requests.
    interrupt: Option<(At, usize)>,
    code: i32,
    local_requests: usize,
    remote_requests: usize,
    cmp: Cmp,
}

fn case(name: &'static str, args: &[&'static str]) -> CCase {
    CCase {
        name,
        args: args.to_vec(),
        env: Vec::new(),
        stdin: None,
        local: Vec::new(),
        remote: Vec::new(),
        no_vault: false,
        files: Vec::new(),
        tokens: None,
        big: None,
        port_to: PortTo::Stub,
        proxy_heads: 0,
        secrets: Vec::new(),
        interrupt: None,
        code: 0,
        local_requests: 0,
        remote_requests: 0,
        cmp: Cmp::Exact,
    }
}

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

#[derive(Debug, PartialEq, Eq)]
struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    local: Vec<String>,
    remote: Vec<String>,
    tmp: Vec<String>,
    /// The CONNECT heads the person's proxy got (either proxy).
    proxy: Vec<String>,
}

impl CRig {
    fn new() -> CRig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = PathBuf::from(format!("/tmp/k5ce-{}-{n}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        let pki = Pki::new(&root.join("pki"));
        let other = Pki::new(&root.join("other-pki"));
        let sides = SIDES
            .iter()
            .map(|s| {
                let dir = root.join(&s[..1]);
                let home = dir.join("h");
                let k2home = k2_client::paths::k2_home_in(&home);
                std::fs::create_dir_all(&k2home).unwrap();
                std::fs::create_dir_all(dir.join("tmp")).unwrap();
                std::fs::create_dir_all(dir.join("proj")).unwrap();
                let local = Stub::start(&k2home.join("vault.sock"));
                let remote = Stub::start(&dir.join("r.sock"));
                Side {
                    dir,
                    home,
                    k2home,
                    remote_tls: TlsFront::start(&pki, remote.port),
                    local_tls: TlsFront::start(&pki, local.port),
                    v6: v6_relay(local.port),
                    local,
                    remote,
                }
            })
            .collect::<Vec<_>>();
        for side in &sides {
            // For the bites: a stand-in that trusts the server's own CA
            // finds it here (`$HOME/../server-ca.pem`).
            std::fs::copy(&pki.ca_file, side.dir.join("server-ca.pem")).unwrap();
        }
        CRig {
            sides,
            no_python3: common::path_without_python3(&root),
            pki,
            other,
            proxy: ProxyStub::start(ProxyMode::Tunnel),
            refuse: ProxyStub::start(ProxyMode::Refuse),
        }
    }

    /// Does every side have an IPv6 loopback relay?
    fn v6(&self) -> bool {
        self.sides.iter().all(|s| s.v6.is_some())
    }

    fn mask(&self, side: &Side, text: &str) -> String {
        let mut t = text
            .replace(&side.dir.display().to_string(), "<dir>")
            .replace(&format!("127.0.0.1:{}", big(side.remote.port)), "<remote+65536>")
            .replace(&format!("127.0.0.1:{}", side.remote.port), "<remote>")
            .replace(&format!("127.0.0.1:{}", side.local.port), "<local>");
        t = t.replace(&format!(":+{}", side.remote_tls.port), ":+<rtls>");
        t = mask_port(&t, side.remote_tls.port, "rtls");
        t = mask_port(&t, side.local_tls.port, "ltls");
        if let Some(p) = side.v6 {
            t = mask_port(&t, p, "v6");
        }
        t = mask_port(&t, side.local.port, "lport");
        mask_ids(&t)
    }

    fn normalise(&self, side: &Side, r: &Logged, case: &str) -> String {
        let text = String::from_utf8_lossy(&r.bytes).into_owned();
        let (head, body) = text
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("{case}: a request with no head"));
        let mut lines = head.split("\r\n");
        // A long body (an upload's chunk) by its length and a hash, taken
        // after the masks (the upload id is random).
        let body = self.mask(side, body);
        let body = if body.len() > 4096 {
            format!("<{} bytes, fnv {:016x}>", body.len(), fnv(body.as_bytes()))
        } else {
            body
        };
        let mut out = vec![fold_query_escapes(lines.next().unwrap_or(""))];
        for l in lines {
            if l.starts_with("User-Agent: ") {
                out.push("User-Agent: <ua>".into());
            } else if l == "X-K5-Cli-Via: fallthrough" || l == "X-K5-Cli-Via: native" {
                out.push("X-K5-Cli-Via: <via>".into());
            } else if l.starts_with("X-K5-Cli-Version: ") || l.starts_with("X-K5-Cli-Noun: ") {
                continue;
            } else {
                out.push(l.to_string());
            }
        }
        self.mask(
            side,
            &format!("[{}]\n{}\n\n{body}", r.channel, out.join("\n")),
        )
    }

    fn run(&self, i: usize, c: &CCase) -> Ran {
        let side = &self.sides[i];
        let fill = |v: &str| {
            v.replace(
                "{remote_big}",
                &format!("http://127.0.0.1:{}", big(side.remote.port)),
            )
            .replace(
                "{remote}",
                &format!("http://127.0.0.1:{}", side.remote.port),
            )
            .replace(
                "{tls_remote}",
                &format!("https://{}:{}", tunnel::HOST, side.remote_tls.port),
            )
            .replace("{tls_port}", &side.remote_tls.port.to_string())
            .replace("{proxy}", &self.proxy.url(None))
            .replace("{proxy_refuse}", &self.refuse.url(None))
            .replace("{ca}", &self.pki.ca_file.display().to_string())
            .replace("{other_ca}", &self.other.ca_file.display().to_string())
            .replace("{dir}", &side.dir.display().to_string())
        };
        let replies = |rs: &[Reply]| -> Vec<Reply> {
            rs.iter()
                .map(|r| match r {
                    Reply::Answer(s, b) => Reply::Answer(*s, fill(b)),
                    Reply::Headed(s, h, b) => Reply::Headed(*s, h.clone(), fill(b)),
                    other => other.clone(),
                })
                .collect()
        };
        side.local.reset(&replies(&c.local));
        side.remote.reset(&replies(&c.remote));
        let _ = std::fs::remove_dir_all(side.k2home.join("clone-tmp"));
        let _ = std::fs::remove_file(side.k2home.join("connect-tokens.json"));
        std::fs::write(side.k2home.join("heartbeat.token"), format!("{OWNER}\n")).unwrap();
        let port = match c.port_to {
            PortTo::Stub => side.local.port,
            PortTo::Tls => side.local_tls.port,
            PortTo::V6 => side.v6.unwrap_or_else(common::stub::dead_port),
        };
        std::fs::write(side.k2home.join("heartbeat.port"), format!("{port}\n")).unwrap();
        if let Some(t) = c.tokens {
            std::fs::write(side.k2home.join("connect-tokens.json"), fill(t)).unwrap();
        }
        for (name, bytes) in &c.files {
            std::fs::write(side.dir.join(name), bytes).unwrap();
        }
        if let Some((name, n)) = c.big {
            let bytes: Vec<u8> = (0..n).map(|i| b"big-bundle-"[i % 11]).collect();
            std::fs::write(side.dir.join(name), bytes).unwrap();
        }
        // The vault socket is the daemon stub's: moved away for a case
        // that has none, put back after.
        let vs = side.k2home.join("vault.sock");
        let away = side.dir.join("vault.sock.away");
        if c.no_vault {
            std::fs::rename(&vs, &away).unwrap();
        }
        let mut cmd = if i == 0 {
            let mut b = Command::new("bash");
            b.arg(cli_k2());
            b
        } else {
            Command::new(bin())
        };
        cmd.env_clear()
            .env("HOME", &side.home)
            .env("TMPDIR", side.dir.join("tmp"))
            .env(
                "PATH",
                if i == 2 {
                    self.no_python3.clone()
                } else {
                    std::env::var("PATH").unwrap()
                },
            )
            .current_dir(side.dir.join("proj"));
        if i == 0 {
            cmd.env("K5_CLI_VIA", "rust");
            // macOS's python3 ignores SSL_CERT_FILE: the shell's python3
            // trusts it through the CA hook ([`tunnel::Pki::py_trust`]).
            if c.env.iter().any(|(k, _)| *k == "SSL_CERT_FILE") {
                cmd.env("PYTHONPATH", &self.pki.py_trust);
            }
        }
        for (k, v) in &c.env {
            cmd.env(k, fill(v));
        }
        cmd.arg("clone").args(c.args.iter().map(|a| fill(a)));
        // A new session: no controlling terminal, as on a server.
        // SAFETY: setsid in the child before exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let who = format!("{}: {}", c.name, SIDES[i]);
        tunnel::take(&self.proxy.heads);
        tunnel::take(&self.refuse.heads);
        let o = match c.interrupt {
            None => run_within(&mut cmd, c.stdin, Duration::from_secs(120), &who),
            Some((at, n)) => {
                let stub = match at {
                    At::Local => &side.local,
                    At::Remote => &side.remote,
                };
                interrupted(&mut cmd, stub, n, &who)
            }
        };
        // The proxy logs on its own threads: let a CONNECT that ended
        // with the process land.
        std::thread::sleep(Duration::from_millis(200));
        let mut proxy = tunnel::take(&self.proxy.heads);
        proxy.extend(tunnel::take(&self.refuse.heads));
        if c.no_vault {
            std::fs::rename(&away, &vs).unwrap();
        }
        let tmp_dir = side.k2home.join("clone-tmp");
        let mut tmp: Vec<String> = std::fs::read_dir(&tmp_dir)
            .map(|rd| {
                rd.map(|e| {
                    use std::os::unix::fs::PermissionsExt;
                    let e = e.unwrap();
                    let mode = e.metadata().unwrap().permissions().mode() & 0o777;
                    format!("{} {mode:o}", e.file_name().to_string_lossy())
                })
                .collect()
            })
            .unwrap_or_default();
        tmp.sort();
        if let Ok(m) = std::fs::metadata(&tmp_dir) {
            use std::os::unix::fs::PermissionsExt;
            tmp.insert(0, format!("clone-tmp {:o}", m.permissions().mode() & 0o777));
        }
        Ran {
            code: o.status.code(),
            stdout: self.mask(side, &String::from_utf8_lossy(&o.stdout)),
            stderr: self.mask(side, &String::from_utf8_lossy(&o.stderr)),
            local: side
                .local
                .take_log()
                .iter()
                .map(|r| self.normalise(side, r, c.name))
                .collect(),
            remote: side
                .remote
                .take_log()
                .iter()
                .map(|r| self.normalise(side, r, c.name))
                .collect(),
            tmp,
            // Each side's own TLS front's port, masked as in its output.
            proxy: proxy.iter().map(|h| self.mask(side, h)).collect(),
        }
    }

    /// The proxy's CONNECTs from side `i`: as many as the case says, each
    /// a CONNECT, none holding a secret (the token and any password ride
    /// inside the tunnel only).
    fn check_proxy(&self, i: usize, c: &CCase, r: &Ran) {
        for h in &r.proxy {
            for secret in &c.secrets {
                assert!(
                    !h.contains(secret),
                    "{}: the {} side's proxy saw {secret:?} in clear: {h:?}",
                    c.name,
                    SIDES[i]
                );
            }
            assert!(
                h.starts_with(&format!("CONNECT {}:", tunnel::HOST)),
                "{}: the {} side's proxy got no CONNECT for {}: {h:?}",
                c.name,
                SIDES[i],
                tunnel::HOST
            );
        }
        assert_eq!(
            r.proxy.len(),
            c.proxy_heads,
            "{}: the {} side's CONNECTs\n{:#?}",
            c.name,
            SIDES[i],
            r.proxy
        );
    }

    fn check(&self, c: &CCase) {
        let bash = self.run(0, c);
        assert_eq!(
            (bash.code, bash.local.len(), bash.remote.len()),
            (Some(c.code), c.local_requests, c.remote_requests),
            "{}: bash's exit and requests (the case must prove what it says)\n{bash:#?}",
            c.name
        );
        self.check_proxy(0, c, &bash);
        for i in [1, 2] {
            let mut got = self.run(i, c);
            self.check_proxy(i, c, &got);
            let mut want = clone_ran(&bash);
            if c.cmp == Cmp::Traceback {
                want.stderr = traceback_shape(&want.stderr);
                got.stderr = traceback_shape(&got.stderr);
            }
            if c.cmp == Cmp::TlsError {
                want.stderr = tls_masked(&want.stderr);
                got.stderr = tls_masked(&got.stderr);
            }
            let fields = [
                (
                    "exit",
                    format!("{:?}", got.code),
                    format!("{:?}", want.code),
                ),
                ("stdout", got.stdout.clone(), want.stdout.clone()),
                ("stderr", got.stderr.clone(), want.stderr.clone()),
                (
                    "local",
                    got.local.join("\n----\n"),
                    want.local.join("\n----\n"),
                ),
                (
                    "remote",
                    got.remote.join("\n----\n"),
                    want.remote.join("\n----\n"),
                ),
                ("clone-tmp", got.tmp.join("\n"), want.tmp.join("\n")),
                // http.client's CONNECT on both sides (K2's ruling: one
                // proxy rule), bash's as Python 3.12 writes it.
                (
                    "CONNECT heads",
                    got.proxy.join("\n----\n"),
                    tunnel::bash_heads_as_312(c.name, &want.proxy).join("\n----\n"),
                ),
            ];
            for (what, g, w) in fields {
                assert!(
                    g == w,
                    "{}: the {} side's {what} differs from bash's\n--- {}:\n{g}\n--- bash:\n{w}",
                    c.name,
                    SIDES[i],
                    SIDES[i]
                );
            }
        }
    }
}

fn clone_ran(r: &Ran) -> Ran {
    Ran {
        code: r.code,
        stdout: r.stdout.clone(),
        stderr: r.stderr.clone(),
        local: r.local.clone(),
        remote: r.remote.clone(),
        tmp: r.tmp.clone(),
        proxy: r.proxy.clone(),
    }
}

/// The binary the rust and nopy sides run: the build's own, or (for a
/// bite) `CLONE_RIG_BIN`, a stand-in that must make a row fail.
fn bin() -> PathBuf {
    match std::env::var_os("CLONE_RIG_BIN") {
        Some(p) => {
            let p = PathBuf::from(p);
            assert!(p.is_file(), "CLONE_RIG_BIN {} is no file", p.display());
            p
        }
        None => PathBuf::from(BIN),
    }
}

/// `port` + 65536: where a port cut to 16 bits lands back on `port`.
fn big(port: u16) -> u32 {
    u32::from(port) + 65536
}

/// `:<port>` (not followed by another digit) as `:<name>`.
fn mask_port(t: &str, port: u16, name: &str) -> String {
    let needle = format!(":{port}");
    let mut out = String::new();
    let mut rest = t;
    while let Some(at) = rest.find(&needle) {
        let after = &rest[at + needle.len()..];
        out.push_str(&rest[..at]);
        if after.bytes().next().is_some_and(|b| b.is_ascii_digit()) {
            out.push_str(&needle);
        } else {
            out.push_str(&format!(":<{name}>"));
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// FNV-1a, for a body too long to compare in a message.
fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325, |h, x| {
        (h ^ u64::from(*x)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Each `[SSL…` reason up to the end of its JSON string as `<tls error>`.
fn tls_masked(stderr: &str) -> String {
    let mut out = String::new();
    let mut rest = stderr;
    while let Some(at) = rest.find("[SSL") {
        out.push_str(&rest[..at]);
        out.push_str("<tls error>");
        let after = &rest[at..];
        // The hint's string ends at the first quote no backslash escapes.
        let b = after.as_bytes();
        let mut j = 0;
        while j < b.len() && !(b[j] == b'"' && (j == 0 || b[j - 1] != b'\\')) {
            j += 1;
        }
        rest = &after[j..];
    }
    out.push_str(rest);
    out
}

/// A plain relay on `[::1]` to `upstream` on IPv4 loopback, for
/// `K2_HOST=::1`. `None`: no IPv6 loopback on this machine.
fn v6_relay(upstream: u16) -> Option<u16> {
    use std::net::{Shutdown, TcpListener, TcpStream};
    let l = TcpListener::bind("[::1]:0").ok()?;
    let port = l.local_addr().ok()?.port();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(c) = c else { continue };
            std::thread::spawn(move || {
                let Ok(up) = TcpStream::connect(("127.0.0.1", upstream)) else {
                    return;
                };
                let (mut c2, mut up2) = (c.try_clone().unwrap(), up.try_clone().unwrap());
                let t = std::thread::spawn(move || {
                    let _ = std::io::copy(&mut c2, &mut up2);
                    let _ = up2.shutdown(Shutdown::Write);
                });
                let (mut up, mut c) = (up, c);
                let _ = std::io::copy(&mut up, &mut c);
                let _ = c.shutdown(Shutdown::Write);
                let _ = t.join();
            });
        }
    });
    Some(port)
}

/// Runs `cmd` until `stub` holds `after` requests, then Ctrl-C: SIGINT
/// to its process group, as a terminal sends it.
fn interrupted(cmd: &mut Command, stub: &Stub, after: usize, who: &str) -> std::process::Output {
    use std::io::Read;
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("{who}: run: {e}"));
    let drain = |mut p: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            p.read_to_end(&mut buf).expect("read a side's output");
            buf
        })
    };
    let out = drain(Box::new(child.stdout.take().expect("its stdout")) as Box<dyn Read + Send>);
    let err = drain(Box::new(child.stderr.take().expect("its stderr")) as Box<dyn Read + Send>);
    let group = i32::try_from(child.id()).expect("a pid");
    let kill = |sig: i32| {
        // SAFETY: a plain kill of the group the child leads.
        unsafe {
            libc::killpg(group, sig);
        }
    };
    let started = Instant::now();
    while stub.log().len() < after {
        let ended = child.try_wait().expect("wait").is_some();
        if ended || started.elapsed() > Duration::from_secs(60) {
            kill(libc::SIGKILL);
            let _ = child.wait();
            panic!(
                "{who}: the stub never held {after} requests ({} came)\n{}",
                stub.log().len(),
                String::from_utf8_lossy(&err.join().expect("its stderr"))
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Let the side settle into its read of the answer.
    std::thread::sleep(Duration::from_millis(300));
    kill(libc::SIGINT);
    let status = loop {
        if let Some(s) = child.try_wait().expect("wait") {
            break s;
        }
        if started.elapsed() > Duration::from_secs(120) {
            kill(libc::SIGKILL);
            let _ = child.wait();
            panic!("{who}: still running after Ctrl-C");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    std::process::Output {
        status,
        stdout: out.join().expect("its stdout"),
        stderr: err.join().expect("its stderr"),
    }
}

/// stderr up to a traceback, then its last line's exception name.
fn traceback_shape(stderr: &str) -> String {
    match stderr.find("Traceback (most recent call last):") {
        None => stderr.to_string(),
        Some(at) => {
            let last = stderr.trim_end().lines().last().unwrap_or("");
            let name = last.split(':').next().unwrap_or("");
            format!("{}<traceback> {name}", &stderr[..at])
        }
    }
}

/// Each uuid (8-4-4-4-12 hex) as `<uuid>`, each `clone-cli-<12 hex>` as
/// `clone-cli-<id>`.
fn mask_ids(t: &str) -> String {
    let b = t.as_bytes();
    let hex = |c: u8| c.is_ascii_hexdigit() && !c.is_ascii_uppercase();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let shape = [8, 4, 4, 4, 12];
        let mut j = i;
        let mut whole = true;
        for (k, n) in shape.iter().enumerate() {
            if j + n > b.len() || !b[j..j + n].iter().all(|c| hex(*c)) {
                whole = false;
                break;
            }
            j += n;
            if k < 4 {
                if b.get(j) != Some(&b'-') {
                    whole = false;
                    break;
                }
                j += 1;
            }
        }
        if whole {
            out.push_str("<uuid>");
            i = j;
            continue;
        }
        if t[i..].starts_with("clone-cli-")
            && b.len() >= i + 22
            && b[i + 10..i + 22].iter().all(|c| hex(*c))
        {
            out.push_str("clone-cli-<id>");
            i += 22;
            continue;
        }
        let ch = t[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

const RECIPIENT: &str = r#"{"recipient":"age1stub","vaultId":"vault-1"}"#;
const PREVIEW: &str = r#"{"workspace":{"path":"/w/ws1","handle":"ws1"},"history":{"files":3,"sessions":2},"threads":{"available":true,"conversations":2,"docs":5,"participants":["ann","bo é"]},"envFiles":[".env"],"callerCanCopySecrets":true,"secrets":{"available":true,"rows":[{"name":"API_KEY","kind":"api_key","group":"keys","copyable":true,"default":"skip","note":"a note"}],"notCarried":["X"],"confirm":{"methods":["connect-password"]}}}"#;
const PREVIEW_PLAIN: &str = r#"{"workspace":{"path":"/w/ws1/","handle":"ws1"},"history":{},"threads":{},"envFiles":[],"callerCanCopySecrets":false,"secrets":{"available":false}}"#;
const UNPACKED: &str = r#"{"dest_path":"/dest/ws1","project":{"id":"p1","name":"ws1","path":"/dest/ws1"},"threads_imported":4,"vault_imported":[]}"#;
const DATA: &[u8] = b"bundle-bytes-bundle-bytes";

// native: clone preview
// native: clone preview --json
#[test]
fn clone_preview_and_here() {
    let rig = CRig::new();
    let cases = vec![
        CCase {
            local: vec![ok(PREVIEW)],
            local_requests: 1,
            ..case("preview: the plan on stderr", &["preview", "ws1"])
        },
        CCase {
            local: vec![ok(PREVIEW)],
            local_requests: 1,
            ..case("preview --json, --threads-since 007", &["preview", "/w/ws1", "--json", "--threads-since", "007", "--own"])
        },
        CCase {
            remote: vec![ok(PREVIEW_PLAIN)],
            remote_requests: 1,
            ..case("preview --host: the remote's plan, its token from the env", &["preview", "ws1", "--host", "{remote}/"])
        }
        .env(vec![("K2_REMOTE_TOKEN", "rtok-1")]),
        CCase {
            local: vec![Reply::Answer(403, r#"{"error":{"code":"clone_not_allowed","hint":"ask an owner"}}"#.into())],
            local_requests: 1,
            code: 3,
            ..case("preview: refused", &["preview", "ws1"])
        },
        CCase {
            local: vec![Reply::Answer(400, "not json at all".into())],
            local_requests: 1,
            code: 2,
            ..case("preview: a 400 that isn't JSON", &["preview", "ws1"])
        },
        CCase {
            local: vec![ok("{}")],
            local_requests: 1,
            code: 1,
            cmp: Cmp::Traceback,
            ..case("preview: no workspace in the answer (KeyError)", &["preview", "ws1"])
        },
        CCase {
            remote: vec![Reply::Headed(
                302,
                vec![("Location".into(), "http://elsewhere.example:9/x".into())],
                "{}".into(),
            )],
            remote_requests: 1,
            code: 3,
            ..case("preview --host: a redirect is not followed", &["preview", "ws1", "--host", "{remote}", "--token", "t1"])
        },
        CCase {
            local: vec![ok(RECIPIENT), ok(PREVIEW_PLAIN), ok(r#"{"bundle_path":"/b/x.tar"}"#), ok(UNPACKED)],
            local_requests: 4,
            ..case("here: a second copy", &["here", "ws1"])
        },
        CCase {
            local: vec![ok(RECIPIENT), ok(PREVIEW_PLAIN), ok(r#"{"bundle_path":"/b/x.tar"}"#), ok(UNPACKED)],
            local_requests: 4,
            ..case("here --json --no-threads --live-only", &["here", "ws1", "--json", "--no-threads", "--live-only"])
        },
        // native: clone
        CCase {
            local: vec![ok(RECIPIENT), ok(PREVIEW_PLAIN), ok(r#"{"bundle_path":"/b/x.tar"}"#), ok(UNPACKED)],
            local_requests: 4,
            ..case("a bare workspace with --dest", &["ws1", "--dest", "/d"])
        },
        CCase {
            local: vec![
                Reply::Answer(503, r#"{"error":"vault closed"}"#.into()),
                ok(PREVIEW),
                ok(r#"{"methods":["local-auth"]}"#),
                ok(r#"{"token":"VT-1"}"#),
                ok(r#"{"bundle_path":"/b/x.tar"}"#),
                ok(UNPACKED),
            ],
            local_requests: 6,
            ..case("here --copy --env-files: the vault closed, one local-auth confirm for the .env files", &["here", "ws1", "--copy", "API_KEY", "--env-files"])
        },
        CCase {
            local: vec![
                ok(RECIPIENT),
                ok(PREVIEW),
                ok(r#"{"bundle_path":"/b/x.tar"}"#),
                ok(UNPACKED),
                ok(r#"{"methods":["local-auth"]}"#),
                ok(r#"{"token":"VT-2"}"#),
                ok(r#"{"copied":1}"#),
            ],
            local_requests: 7,
            ..case("here --copy: the secret copied in place after a confirm", &["here", "ws1", "--copy", "API_KEY"])
        },
        CCase {
            local: vec![
                ok(RECIPIENT),
                ok(PREVIEW),
                ok(r#"{"bundle_path":"/b/x.tar"}"#),
                ok(UNPACKED),
            ],
            no_vault: true,
            local_requests: 4,
            code: 3,
            ..case("here --copy: no vault socket, nothing typed, nothing over TCP", &["here", "ws1", "--copy", "API_KEY"])
        },
        CCase {
            local: vec![
                ok(RECIPIENT),
                ok(PREVIEW),
                ok(r#"{"bundle_path":"/b/x.tar"}"#),
                ok(UNPACKED),
                ok(r#"{"methods":["vault-password"]}"#),
            ],
            local_requests: 5,
            code: 3,
            ..case("here --copy: a password with no terminal is no person here", &["here", "ws1", "--copy", "API_KEY"])
        },
        CCase {
            local: vec![ok(RECIPIENT), ok(PREVIEW)],
            local_requests: 2,
            code: 2,
            ..case("here: a name given two choices", &["here", "ws1", "--copy", "API_KEY", "--skip", "API_KEY"])
        },
        CCase {
            local: vec![ok(RECIPIENT), ok(PREVIEW)],
            local_requests: 2,
            code: 2,
            ..case("here: no such secret", &["here", "ws1", "--blank", "NOPE"])
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

impl CCase {
    fn env(mut self, env: Vec<(&'static str, &'static str)>) -> CCase {
        self.env = env;
        self
    }
}

// native: clone to
// native: clone from
#[test]
fn clone_to_and_from() {
    let rig = CRig::new();
    let b64 = "YnVuZGxlLWJ5dGVzLWJ1bmRsZS1ieXRlcw==";
    let to_local = vec![
        ok(PREVIEW_PLAIN),
        ok(r#"{"bundle_path":"{dir}/bundle-1.tar"}"#),
    ];
    let cases = vec![
        CCase {
            local: to_local.clone(),
            remote: vec![
                ok(RECIPIENT),
                ok(r#"{"home":"/rhome/","separator":"/"}"#),
                ok(r#"{"path":"/rhome/.k2/clone-tmp/bundle-1.tar"}"#),
                ok(UNPACKED),
            ],
            files: vec![("bundle-1.tar", DATA)],
            local_requests: 2,
            remote_requests: 4,
            ..case(
                "to: one chunk, the token from the env",
                &["to", "{remote}", "ws1"],
            )
        }
        .env(vec![("K2_REMOTE_TOKEN", "rtok-2")]),
        CCase {
            local: to_local.clone(),
            remote: vec![
                ok(RECIPIENT),
                ok(r#"{"home":"/rhome","separator":"/"}"#),
                ok(r#"{"path":"/rhome/.k2/clone-tmp/bundle-1.tar"}"#),
                ok(UNPACKED),
            ],
            files: vec![("bundle-1.tar", DATA)],
            stdin: Some(b"stdin-tok\r\n"),
            local_requests: 2,
            remote_requests: 4,
            ..case(
                "to --token-stdin --json --dest",
                &[
                    "to",
                    "{remote}",
                    "ws1",
                    "--token-stdin",
                    "--json",
                    "--dest",
                    "/there",
                ],
            )
        },
        CCase {
            local: to_local.clone(),
            remote: vec![ok(RECIPIENT), ok(r#"{"separator":"/"}"#)],
            files: vec![("bundle-1.tar", DATA)],
            local_requests: 2,
            remote_requests: 2,
            code: 3,
            ..case(
                "to: the remote names no home",
                &["to", "{remote}", "ws1", "--token", "t2"],
            )
        },
        CCase {
            local: to_local.clone(),
            remote: vec![
                ok(RECIPIENT),
                ok(r#"{"home":"/rhome","separator":"/"}"#),
                ok(r#"{"ok":true}"#),
                ok(r#"{"path":"/rhome/.k2/clone-tmp/bundle-1.tar"}"#),
                ok(UNPACKED),
            ],
            big: Some(("bundle-1.tar", 8 * 1024 * 1024 + 5)),
            local_requests: 2,
            remote_requests: 5,
            ..case(
                "to: an 8 MiB + 5 byte bundle goes up in two chunks",
                &["to", "{remote}", "ws1", "--token", "t7", "--json"],
            )
        },
        CCase {
            local: to_local.clone(),
            remote: vec![
                ok(RECIPIENT),
                ok(r#"{"home":"/rhome","separator":"/"}"#),
                ok(r#"{"path":"/rhome/.k2/clone-tmp/bundle-1.tar"}"#),
                ok(UNPACKED),
            ],
            files: vec![("bundle-1.tar", &b""[..])],
            local_requests: 2,
            remote_requests: 4,
            ..case(
                "to: a zero-byte bundle is one last chunk",
                &["to", "{remote}", "ws1", "--token", "t8"],
            )
        },
        CCase {
            local: vec![ok(RECIPIENT)],
            remote: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"job_id":"job-1"}"#),
                ok(r#"{"phase":"packing"}"#),
                ok(&format!(
                    r#"{{"phase":"done","bundle_path":"/rb/bundle-job-1.tar","size_bytes":{}}}"#,
                    DATA.len()
                )),
                ok(&format!(r#"{{"base64":"{b64}","eof":true}}"#)),
                ok(r#"{"ok":true}"#),
            ],
            local_requests: 1,
            remote_requests: 6,
            ..case(
                "from: pack, wait, download, unpack here",
                &["from", "{remote}:ws1", "--token", "t3"],
            )
        }
        .with_local_tail(ok(UNPACKED)),
        CCase {
            local: vec![ok(RECIPIENT)],
            remote: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"job_id":7}"#),
                ok(r#"{"phase":"done","bundle_path":"/rb/b.tar","size_bytes":99}"#),
                ok(&format!(r#"{{"base64":"{b64}","eof":true}}"#)),
                ok(r#"{"ok":true}"#),
            ],
            local_requests: 1,
            remote_requests: 5,
            code: 3,
            ..case(
                "from: a short download leaves no .part",
                &["from", "{remote}:ws1", "--token", "t3"],
            )
        },
        CCase {
            local: vec![ok(RECIPIENT)],
            remote: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"job_id":"j"}"#),
                ok(r#"{"phase":"done","bundle_path":"/rb/b.tar","size_bytes":0}"#),
                Reply::Answer(403, r#"{"error":"range refused"}"#.into()),
                Reply::Answer(500, r#"{"error":"cleanup failed"}"#.into()),
            ],
            local_requests: 1,
            remote_requests: 5,
            code: 3,
            ..case(
                "from: a refused range, the cleanup still asked",
                &["from", "{remote}:ws1", "--token", "t3"],
            )
        },
        CCase {
            local: vec![ok(RECIPIENT)],
            remote: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"job_id":"j"}"#),
                ok(r#"{"phase":"failed","error":"disk full"}"#),
            ],
            local_requests: 1,
            remote_requests: 3,
            code: 3,
            ..case(
                "from: the pack failed",
                &["from", "{remote}:ws1", "--token", "t3"],
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

impl CCase {
    fn with_local_tail(mut self, r: Reply) -> CCase {
        self.local.push(r);
        self.local_requests += 1;
        self
    }
}

const PROPOSALS_HERE: &str = r#"{"proposals":[{"id":"cp-77","status":"open","summary":"clone ws1 to here","proposedBy":"agent:kessel","why":"a backup","target":"here","workspacePath":"/w/ws1","options":{"threads":false,"secrets":{}}}]}"#;
const PROPOSALS_HOST: &str = r#"{"proposals":[{"id":"cp-78","status":"open","summary":"clone ws1 to a host","proposedBy":"agent:kessel","target":"{remote}","workspacePath":"/w/ws1","options":{}}]}"#;

// native: clone approve
#[test]
fn clone_approve() {
    let rig = CRig::new();
    let cases = vec![
        CCase {
            local: vec![
                ok(PROPOSALS_HERE),
                ok(PROPOSALS_HERE),
                ok(RECIPIENT),
                ok(PREVIEW_PLAIN),
                ok(r#"{"methods":["local-auth"]}"#),
                ok(r#"{"token":"CA-1"}"#),
                ok(r#"{"ok":true}"#),
                ok(r#"{"bundle_path":"/b/x.tar"}"#),
                ok(UNPACKED),
                ok(r#"{"ok":true}"#),
            ],
            local_requests: 10,
            ..case(
                "approve here by a prefix: one confirm approves it",
                &["approve", "cp-7"],
            )
        },
        CCase {
            local: vec![ok(r#"{"proposals":[]}"#)],
            local_requests: 1,
            code: 3,
            ..case("approve: no such proposal", &["approve", "cp-1"])
        },
        CCase {
            local: vec![ok(PROPOSALS_HOST)],
            local_requests: 1,
            code: 3,
            ..case(
                "approve a host: no saved token, nothing sent",
                &["approve", "cp-78"],
            )
        }
        .env(vec![("K2_REMOTE_TOKEN", "never-sent")]),
        CCase {
            local: vec![ok(PROPOSALS_HOST), ok(PROPOSALS_HOST)],
            tokens: Some(r#"{"{remote}": "saved-tok"}"#),
            local_requests: 2,
            code: 3,
            ..case(
                "approve a host: no terminal, nothing sent there",
                &["approve", "cp-78", "--yes"],
            )
        },
        CCase {
            local: vec![
                ok(PROPOSALS_HERE),
                ok(PROPOSALS_HERE),
                ok(RECIPIENT),
                ok(PREVIEW_PLAIN),
                ok(r#"{"methods":["local-auth"]}"#),
                ok(r#"{"token":"CA-1"}"#),
                ok(r#"{"ok":true}"#),
                Reply::Answer(500, r#"{"error":"bundle failed"}"#.into()),
                ok(r#"{"ok":true}"#),
            ],
            local_requests: 9,
            code: 3,
            ..case(
                "approve: a failed run is finished as not ok",
                &["approve", "cp-77", "--json"],
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

#[test]
fn the_stops_before_anything_is_sent() {
    let rig = CRig::new();
    let cases = vec![
        CCase {
            code: 3,
            ..case(
                "to: plain http to a name is refused",
                &["to", "http://h.example:9", "ws1", "--token", "t"],
            )
        },
        CCase {
            code: 3,
            ..case(
                "from: a name before an @ is refused",
                &["from", "https://ann:pw@h.example:ws1"],
            )
        },
        CCase {
            code: 3,
            ..case("to https: no token", &["to", "https://h.example", "ws1"])
        },
        CCase {
            code: 2,
            stdin: Some(b""),
            ..case(
                "--token-stdin with nothing on stdin",
                &["here", "ws1", "--token-stdin"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "--token-stdin and --token",
                &["to", "h", "w", "--token-stdin", "--token", "x"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "approve takes no --token",
                &["approve", "cp-1", "--token", "x"],
            )
        },
        CCase {
            code: 3,
            env: vec![("K2_HOOK_TOKEN", "hk")],
            ..case("an agent can't clone", &["here", "ws1"])
        },
        CCase {
            code: 2,
            env: vec![("K2_HOST", "h.example")],
            ..case(
                "this server over plain http: the warning once, then the usage",
                &["preview"],
            )
        },
        CCase {
            code: 2,
            ..case("a bare word with no --dest", &["ws1"])
        },
        // A server port outside 1 to 65535 (`_clone_port_gate`): a usage
        // line, exit 2, nothing sent. Port + 65536 would reach the remote
        // stub on Linux if it went out.
        CCase {
            code: 2,
            ..case(
                "preview --host: a port 65536 past the remote's",
                &["preview", "ws1", "--host", "{remote_big}", "--token", "t"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "to: a port 65536 past the remote's",
                &["to", "{remote_big}", "ws1", "--token", "t", "--json"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "from: a port 65536 past the remote's",
                &["from", "{remote_big}:ws1", "--token", "t"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "preview --host: a bracketed IPv6 address's port",
                &["preview", "ws1", "--host", "http://[::1]:70000", "--token", "t"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "preview --host: a port in a % escape",
                &["preview", "ws1", "--host", "https://cp.example.invalid%3A70000", "--token", "t"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "preview --host: a leading zero",
                &["preview", "ws1", "--host", "https://cp.example.invalid:0443", "--token", "t"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "preview --host: a sign",
                &["preview", "ws1", "--host", "https://cp.example.invalid:+443", "--token", "t"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "preview --host: port 0",
                &["preview", "ws1", "--host", "https://cp.example.invalid:0", "--token", "t"],
            )
        },
        CCase {
            code: 2,
            ..case("--json alone", &["--json"])
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

const PROPOSALS_TLS: &str = r#"{"proposals":[{"id":"cp-79","status":"open","summary":"clone ws1 to a host","proposedBy":"agent:kessel","target":"{tls_remote}","workspacePath":"/w/ws1","options":{}}]}"#;

/// The person's proxy carries a remote `https://` server, read as the
/// shell's Python reads it, on the shared CONNECT path: each side's heads
/// are compared byte for byte, counted, and read for secrets.
#[test]
fn clone_through_the_persons_proxy() {
    let rig = CRig::new();
    let tls = |mut c: CCase| {
        c.env.push(("SSL_CERT_FILE", "{ca}"));
        c
    };
    let cases = vec![
        tls(CCase {
            remote: vec![ok(PREVIEW_PLAIN)],
            remote_requests: 1,
            proxy_heads: 1,
            secrets: vec!["rt-proxy-1"],
            env: vec![("HTTPS_PROXY", "{proxy}")],
            ..case(
                "preview --host https: through the person's proxy, the token only inside the tunnel",
                &["preview", "ws1", "--host", "{tls_remote}", "--token", "rt-proxy-1", "--json"],
            )
        }),
        tls(CCase {
            local: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"bundle_path":"{dir}/bundle-1.tar"}"#),
            ],
            remote: vec![
                ok(RECIPIENT),
                ok(r#"{"home":"/rhome","separator":"/"}"#),
                ok(r#"{"path":"/rhome/.k2/clone-tmp/bundle-1.tar"}"#),
                ok(UNPACKED),
            ],
            files: vec![("bundle-1.tar", DATA)],
            local_requests: 2,
            remote_requests: 4,
            proxy_heads: 4,
            secrets: vec!["rt-proxy-2", OWNER],
            env: vec![("https_proxy", "{proxy}")],
            ..case(
                "to https: each remote request in its own tunnel, this server's never proxied",
                &["to", "{tls_remote}", "ws1", "--token", "rt-proxy-2", "--json"],
            )
        }),
        tls(CCase {
            proxy_heads: 1,
            secrets: vec!["rt-proxy-3"],
            code: 3,
            env: vec![("HTTPS_PROXY", "{proxy_refuse}")],
            ..case(
                "preview --host https: the proxy refuses the tunnel (407), nothing sent",
                &["preview", "ws1", "--host", "{tls_remote}", "--token", "rt-proxy-3"],
            )
        }),
        CCase {
            proxy_heads: 1,
            secrets: vec!["rt-proxy-6"],
            code: 3,
            cmp: Cmp::TlsError,
            env: vec![
                ("HTTPS_PROXY", "{proxy}"),
                ("SSL_CERT_FILE", "{other_ca}"),
            ],
            ..case(
                "preview --host https: through the proxy, a certificate no CA here signed: the tunnel opens, nothing sent in it",
                &["preview", "ws1", "--host", "{tls_remote}", "--token", "rt-proxy-6"],
            )
        },
        tls(CCase {
            code: 3,
            env: vec![
                ("HTTPS_PROXY", "{proxy}"),
                ("NO_PROXY", "example.invalid"),
            ],
            ..case(
                "preview --host https: NO_PROXY names it, so straight there (the name doesn't resolve)",
                &["preview", "ws1", "--host", "{tls_remote}", "--token", "rt-proxy-4"],
            )
        }),
        tls(CCase {
            remote: vec![ok(PREVIEW_PLAIN)],
            remote_requests: 1,
            proxy_heads: 1,
            secrets: vec!["rt-proxy-5"],
            env: vec![("HTTPS_PROXY", "{proxy}")],
            ..case(
                "preview --host: a % escape in the host and the path, a port with a sign",
                &[
                    "preview",
                    "ws1",
                    "--host",
                    "https://cp.example.%69nvalid:+{tls_port}/p%41",
                    "--token",
                    "rt-proxy-5",
                    "--json",
                ],
            )
        }),
        tls(CCase {
            local: vec![ok(PROPOSALS_TLS), ok(PROPOSALS_TLS)],
            tokens: Some(r#"{"{tls_remote}": "saved-tok"}"#),
            local_requests: 2,
            code: 3,
            env: vec![("HTTPS_PROXY", "{proxy}")],
            ..case(
                "approve a host with the person's proxy set: no terminal, nothing sent there",
                &["approve", "cp-79", "--yes"],
            )
        }),
    ];
    for c in &cases {
        rig.check(c);
    }
}

/// `K2_HOST=https://<name>`: this server behind TLS, the certificate
/// checked against what the person trusts (`SSL_CERT_FILE`), straight to
/// a loopback name and through the person's proxy to any other.
#[test]
fn clone_this_server_over_https() {
    let rig = CRig::new();
    let cases = vec![
        CCase {
            port_to: PortTo::Tls,
            code: 3,
            cmp: Cmp::TlsError,
            env: vec![
                ("K2_HOST", "https://localhost"),
                ("SSL_CERT_FILE", "{other_ca}"),
            ],
            ..case(
                "K2_HOST=https://localhost, a certificate no CA here signed: nothing sent",
                &["preview", "ws1"],
            )
        },
        CCase {
            local: vec![ok(PREVIEW)],
            local_requests: 1,
            port_to: PortTo::Tls,
            env: vec![
                ("K2_HOST", "https://localhost"),
                ("HTTPS_PROXY", "{proxy}"),
                ("SSL_CERT_FILE", "{ca}"),
            ],
            ..case(
                "K2_HOST=https://localhost: over TLS, straight there with a proxy set",
                &["preview", "ws1"],
            )
        },
        CCase {
            local: vec![ok(PREVIEW_PLAIN)],
            local_requests: 1,
            port_to: PortTo::Tls,
            proxy_heads: 1,
            secrets: vec![OWNER],
            env: vec![
                ("K2_HOST", "https://cp.example.invalid/"),
                ("HTTPS_PROXY", "{proxy}"),
                ("SSL_CERT_FILE", "{ca}"),
            ],
            ..case(
                "K2_HOST=https://<name>/: through the person's proxy, the owner token inside the tunnel",
                &["preview", "ws1", "--json"],
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

/// `K2_HOST` as an IPv6 address: `http://::1:PORT` as the shell writes
/// it, or bracketed. Where this machine has no IPv6 loopback, both sides
/// meet the same failed connect.
#[test]
fn clone_this_server_at_an_ipv6_address() {
    let rig = CRig::new();
    let v6 = rig.v6();
    println!("IPv6 loopback here: {v6}");
    let row = |name: &'static str, host: &'static str| CCase {
        local: if v6 { vec![ok(PREVIEW)] } else { Vec::new() },
        local_requests: usize::from(v6),
        code: if v6 { 0 } else { 3 },
        port_to: PortTo::V6,
        env: vec![("K2_HOST", host)],
        ..case(name, &["preview", "ws1"])
    };
    for c in &[row("K2_HOST=::1", "::1"), row("K2_HOST=[::1]", "[::1]")] {
        rig.check(c);
    }
}

/// A remote URL read as Python's urllib reads it: a query carries every
/// route, a fragment takes the routes with it, a port that is not a
/// number is `InvalidURL` at the first request, an empty port is the
/// scheme's.
#[test]
fn clone_remote_urls_as_python_reads_them() {
    let rig = CRig::new();
    let b64 = "YnVuZGxlLWJ5dGVzLWJ1bmRsZS1ieXRlcw==";
    let cases = vec![
        CCase {
            local: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"bundle_path":"{dir}/bundle-1.tar"}"#),
            ],
            remote: vec![
                ok(RECIPIENT),
                ok(r#"{"home":"/rhome","separator":"/"}"#),
                ok(r#"{"path":"/rhome/.k2/clone-tmp/bundle-1.tar"}"#),
                ok(UNPACKED),
            ],
            files: vec![("bundle-1.tar", DATA)],
            local_requests: 2,
            remote_requests: 4,
            ..case(
                "to: a query in the remote URL",
                &["to", "{remote}/?x=1", "ws1", "--token", "t4", "--json"],
            )
        },
        CCase {
            local: vec![ok(RECIPIENT), ok(UNPACKED)],
            remote: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"job_id":"job-1"}"#),
                ok(&format!(
                    r#"{{"phase":"done","bundle_path":"/rb/bundle-job-1.tar","size_bytes":{}}}"#,
                    DATA.len()
                )),
                ok(&format!(r#"{{"base64":"{b64}","eof":true}}"#)),
                ok(r#"{"ok":true}"#),
            ],
            local_requests: 2,
            remote_requests: 5,
            ..case(
                "from: a fragment in the remote URL",
                &["from", "{remote}/#frag:ws1", "--token", "t5", "--json"],
            )
        },
        CCase {
            code: 2,
            ..case(
                "to: a port that isn't a number is refused as typed",
                &[
                    "to",
                    "https://cp.example.invalid:abc",
                    "ws1",
                    "--token",
                    "t6",
                    "--json",
                ],
            )
        },
        CCase {
            code: 3,
            ..case(
                "preview --host: an empty port is 443 (the name doesn't resolve)",
                &[
                    "preview",
                    "ws1",
                    "--host",
                    "https://cp.example.invalid:",
                    "--token",
                    "t9",
                ],
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

/// Ctrl-C (SIGINT to the process group, as a terminal sends it) while an
/// answer is awaited: the shared 130 and the cancelled line, nothing more
/// sent.
#[test]
fn clone_ctrl_c() {
    let rig = CRig::new();
    let cases = vec![
        CCase {
            local: vec![Reply::Hang(Duration::from_secs(30))],
            interrupt: Some((At::Local, 1)),
            local_requests: 1,
            code: 130,
            ..case(
                "preview: Ctrl-C while this server's answer is awaited",
                &["preview", "ws1"],
            )
        },
        CCase {
            local: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"bundle_path":"{dir}/bundle-1.tar"}"#),
            ],
            remote: vec![
                ok(RECIPIENT),
                ok(r#"{"home":"/rhome","separator":"/"}"#),
                Reply::Hang(Duration::from_secs(30)),
            ],
            files: vec![("bundle-1.tar", DATA)],
            interrupt: Some((At::Remote, 3)),
            local_requests: 2,
            remote_requests: 3,
            code: 130,
            ..case(
                "to: Ctrl-C during the upload",
                &["to", "{remote}", "ws1", "--token", "t10", "--json"],
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

#[test]
fn mask_ids_masks_only_ids() {
    assert_eq!(
        mask_ids("a 0f3c2b1a-1111-4abc-8def-0123456789ab b clone-cli-0123456789ab c"),
        "a <uuid> b clone-cli-<id> c"
    );
    assert_eq!(mask_ids("not-a-uuid"), "not-a-uuid");
}
