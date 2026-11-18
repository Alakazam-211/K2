//! The rig a native family's parity test runs on: `cli/k2 <verb>` (as the
//! front hands it a verb) and `k5-cli <verb>`, each run as people run it
//! against one logging stub daemon ([`super::stub`]). For every case both
//! get the same environment, folder and answers, and must send the same
//! requests and print the same stdout and stderr with the same exit code.
//! `tests/whoami_parity.rs` is the first of these, written before the rig.
//!
//! The shell runs with `K5_CLI_VIA=rust`, and `K5_CLI_WOULD_REFUSE=1`
//! when the passport rule says so, so its requests carry the hand-over
//! headers. The normaliser treats only these as expected differences,
//! each counted (exactly one per request):
//!
//! - `User-Agent`: `curl/<version>` from bash, `k5/<version>` here;
//! - `X-K5-Cli-Version: <version>`: sent by the binary only, dropped;
//! - `X-K5-Cli-Via`: `fallthrough` from bash, `native` from the binary;
//! - `X-K5-Cli-Noun: <verb>`: sent by bash only, dropped;
//! - the case of each `%XX` escape in the query (curl 8.5 and 8.7 write
//!   `-G`'s in lower case; the binary writes upper case; K2's ruling).
//!
//! A case that the binary hands to the shell ([`Compare::Shell`]) must
//! reach the daemon through the shell on both sides: both are read by
//! bash's rules. One case kind differs on purpose, by PRD 13.9 #4
//! ([`Compare::OwnerTokenWithPassport`]).
//!
//! Hermetic: HOME, TMPDIR and the K2 home are scratch folders, the tokens
//! are made up, and each side gets nothing from the environment but PATH
//! and what the case sets.

#![cfg(unix)]

use super::stub::{Logged, Reply, Stub};
use std::ffi::OsString;
use std::net::TcpListener;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
pub const OWNER: &str = "owner-tok-7171";
pub const PASSPORT: &str = "sessA.secret7272";
pub const HEALED: &str = "sessA.secret7373";
pub const REFUSED: &str = r#"{"error":"Invalid or missing auth token"}"#;
/// A refusal with the permission cards' `permission` object.
pub const PERMISSION_ASK: &str = r#"{"error":{"code":"owner_only","hint":"Only the owner can do that."},"permission":{"command":"k2 permission request --scope x.write --why \"...\"","requestable":true}}"#;
/// The Org Chart's mark of an unread bulletin.
pub const BULLETIN: &str =
    r#"{"ok":true,"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#;
/// Not HTTP: curl reads it as HTTP/0.9 and refuses it (status 000, exit 1).
pub const GARBAGE: &[u8] = b"garbage\r\n\r\n";

pub fn cli_k2() -> PathBuf {
    let p = super::cli_src().join("../k2");
    assert!(p.is_file(), "no shell CLI at {}", p.display());
    p
}

static N: AtomicUsize = AtomicUsize::new(0);

pub struct Rig {
    pub verb: &'static str,
    pub dir: PathBuf,
    pub home: PathBuf,
    pub k2home: PathBuf,
    pub proj: PathBuf,
    pub secrets: PathBuf,
    /// A socket file nothing listens on.
    pub dead_sock: PathBuf,
    pub stub: Stub,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Out {
    /// The exit code, or 128 plus the signal that ended the process (as a
    /// shell reports it: 141 for SIGPIPE).
    pub code: Option<i32>,
    /// The signal that ended the process, if one did.
    pub signal: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// How a case compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    /// Everything, byte for byte.
    Exact,
    /// A no-answer line's seconds may be one apart: bash counts whole
    /// seconds of `$SECONDS`, which ticks on its own.
    Waited,
    /// The binary hands the line to the shell: both sides' requests are
    /// the shell's.
    Shell,
    /// PRD 13.9 #4: bash sends the owner token, the binary the passport.
    OwnerTokenWithPassport,
    /// No answer to a request on the left of a pipe into a reader: the
    /// line, then death by SIGPIPE (141, `_k2_end_command`), compared as
    /// [`Compare::Waited`]. macOS's `/bin/bash` 3.2 doesn't stop the
    /// reader there: it reads an empty input, prints `stdout` and
    /// `stderr`, and the command exits 1. Only there, and only when bash
    /// ended with no signal, that is what bash must print (after the
    /// binary's line) while the binary still dies of SIGPIPE with nothing
    /// on stdout.
    InPipe {
        stdout: &'static str,
        stderr: &'static str,
    },
    /// The shell's Python raises: everything as [`Compare::Exact`] but
    /// the traceback on stderr, where only what comes before it and its
    /// last line's exception name must match (its frames and wording
    /// depend on the Python version; the binary prints 3.12's).
    Traceback,
}

pub struct Case {
    pub name: &'static str,
    pub args: &'static [&'static str],
    /// `{sock}` is the stub's socket, `{deadsock}` a socket file nothing
    /// listens on, `{nosock}` a path with nothing there, `{secrets}` the
    /// secrets folder.
    pub env: Vec<(&'static str, String)>,
    pub replies: Vec<Reply>,
    /// Run before each side (files the case needs).
    pub setup: fn(&Rig),
    pub cwd: Option<PathBuf>,
    /// TMPDIR names a folder that isn't there.
    pub no_tmpdir: bool,
    /// Bash's exit code (141: killed by SIGPIPE) and request count: the
    /// case proves what it says.
    pub code: i32,
    pub requests: usize,
    pub compare: Compare,
}

impl Rig {
    pub fn new(verb: &'static str, tag: &str) -> Rig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k5-{verb}-{tag}-{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let home = dir.join("home");
        let k2home = k2_client::paths::k2_home_in(&home);
        let proj = dir.join("proj");
        let secrets = dir.join("secrets");
        for d in [
            &k2home,
            &proj.join(".k2"),
            &secrets,
            &dir.join("tmp-bash"),
            &dir.join("tmp-rust"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let short = verb.chars().next().expect("a verb");
        let stub = Stub::start(&PathBuf::from(format!(
            "/tmp/k5{short}-{}-{n}.sock",
            std::process::id()
        )));
        let dead_sock = PathBuf::from(format!("/tmp/k5{short}d-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&dead_sock);
        drop(
            std::os::unix::net::UnixListener::bind(&dead_sock)
                .unwrap_or_else(|e| panic!("bind {}: {e}", dead_sock.display())),
        );
        Rig {
            verb,
            dir,
            home,
            k2home,
            proj,
            secrets,
            dead_sock,
            stub,
        }
    }

    pub fn owner_on_disk(&self) {
        std::fs::write(self.k2home.join("heartbeat.token"), format!("{OWNER}\n")).unwrap();
    }

    pub fn port_on_disk(&self) {
        std::fs::write(
            self.k2home.join("heartbeat.port"),
            format!("{}\n", self.stub.port),
        )
        .unwrap();
    }

    fn fill(&self, v: &str) -> String {
        v.replace("{sock}", &self.stub.sock.to_string_lossy())
            .replace("{deadsock}", &self.dead_sock.to_string_lossy())
            .replace("{nosock}", &self.dir.join("no-socket").to_string_lossy())
            .replace("{secrets}", &self.secrets.to_string_lossy())
    }

    /// One side's run. `bash`: `cli/k2` as the front hands it a verb;
    /// else the binary.
    pub fn run(&self, bash: bool, case: &Case) -> Out {
        let mut cmd = if bash {
            let mut c = Command::new("bash");
            c.arg(cli_k2());
            c
        } else {
            Command::new(BIN)
        };
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        let tmp = if case.no_tmpdir {
            self.dir.join("no-tmp")
        } else {
            self.dir.join(if bash { "tmp-bash" } else { "tmp-rust" })
        };
        cmd.env("HOME", &self.home)
            .env("TMPDIR", &tmp)
            .current_dir(case.cwd.as_deref().unwrap_or(&self.proj));
        let env: Vec<(String, String)> = case
            .env
            .iter()
            .map(|(k, v)| (k.to_string(), self.fill(v)))
            .collect();
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        if bash {
            cmd.env("K5_CLI_VIA", "rust");
            let lookup = |name: &str| {
                env.iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| OsString::from(v))
            };
            let args: Vec<OsString> = std::iter::once(self.verb)
                .chain(case.args.iter().copied())
                .map(OsString::from)
                .collect();
            let verdict = k5_cli::passport::decide(
                k5_cli::passport::facts(&args, lookup),
                k5_cli::passport::live_session_ancestor,
            );
            if verdict.would_refuse() {
                cmd.env("K5_CLI_WOULD_REFUSE", "1");
            }
        }
        cmd.arg(self.verb).args(case.args);
        let o = cmd.output().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
        let signal = o.status.signal();
        Out {
            code: o.status.code().or_else(|| signal.map(|s| 128 + s)),
            signal,
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// Runs both sides of `c` and compares them.
    pub fn check(&self, c: &Case) {
        let name = c.name;
        let mut outs = Vec::new();
        let mut logs = Vec::new();
        for bash in [true, false] {
            // Each case starts from an empty K2 home and no re-minted passport.
            for f in [
                self.secrets.join(".k2-passport"),
                self.k2home.join("heartbeat.port"),
                self.k2home.join("heartbeat.token"),
                self.k2home.join("daemon.token"),
            ] {
                let _ = std::fs::remove_file(f);
            }
            (c.setup)(self);
            self.stub.reset(&c.replies);
            let out = self.run(bash, c);
            let side = if bash { "bash" } else { "rust" };
            assert_eq!(
                self.stub.left(),
                0,
                "{name}: {side} left answers unused: {out:?}"
            );
            let native = !bash && c.compare != Compare::Shell;
            logs.push(
                self.stub
                    .log()
                    .iter()
                    .map(|r| normalise(r, native, self.verb, name))
                    .collect::<Vec<_>>(),
            );
            outs.push(out);
        }
        let (b, r) = (&outs[0], &outs[1]);
        let (mut bl, rl) = (logs[0].clone(), logs[1].clone());
        println!(
            "== {name}\n{}\n-- bash: {b:?}\n-- rust: {r:?}",
            bl.join("\n----\n").replace('\r', "")
        );
        let bash_3_2_pipe = match c.compare {
            Compare::InPipe { stdout, stderr }
                if cfg!(target_os = "macos") && b.signal.is_none() =>
            {
                Some((stdout, stderr))
            }
            _ => None,
        };
        let want = if bash_3_2_pipe.is_some() { 1 } else { c.code };
        assert_eq!(b.code, Some(want), "{name}: bash's exit: {b:?}");
        assert_eq!(
            bl.len(),
            c.requests,
            "{name}: bash sent {} requests:\n{}",
            bl.len(),
            bl.join("\n----\n")
        );
        if c.compare == Compare::OwnerTokenWithPassport {
            assert!(
                bl.iter().all(|l| l.contains(&format!("token={OWNER}&"))),
                "{name}: bash no longer sends the owner token: {bl:?}"
            );
            assert!(
                rl.iter()
                    .all(|l| l.contains(&format!("token={PASSPORT}&")) && !l.contains(OWNER)),
                "{name}: the binary: {rl:?}"
            );
            bl = bl
                .iter()
                .map(|l| l.replace(&format!("token={OWNER}&"), &format!("token={PASSPORT}&")))
                .collect();
        }
        assert_eq!(
            rl,
            bl,
            "{name}: the requests differ\n== rust ==\n{}\n== bash ==\n{}",
            rl.join("\n----\n"),
            bl.join("\n----\n")
        );
        if let Some((stdout, stderr)) = bash_3_2_pipe {
            assert_eq!(
                (r.code, r.signal, r.stdout.as_str()),
                (Some(c.code), Some(13), ""),
                "{name}: the binary: {r:?}"
            );
            assert_eq!(
                b.stdout, stdout,
                "{name}: bash 3.2's reader on an empty input"
            );
            let (bs, bn) = mask_waited(&b.stderr);
            let (rs, rn) = mask_waited(&format!("{}{stderr}", r.stderr));
            assert_eq!(
                bs, rs,
                "{name}: stderr (seconds masked): bash 3.2 adds the reader's"
            );
            assert!(
                bn.len() == rn.len() && bn.iter().zip(&rn).all(|(x, y)| x.abs_diff(*y) <= 1),
                "{name}: the waited lines"
            );
            return;
        }
        assert_eq!(
            r.code, b.code,
            "{name}: the exit code\nrust: {r:?}\nbash: {b:?}"
        );
        assert_eq!(
            r.signal, b.signal,
            "{name}: the signal that ended it\nrust: {r:?}\nbash: {b:?}"
        );
        assert_eq!(r.stdout, b.stdout, "{name}: stdout");
        if c.compare == Compare::Traceback {
            const HEAD: &str = "Traceback (most recent call last):\n";
            let split = |text: &str, side: &str| -> (String, String) {
                let (before, tb) = text
                    .split_once(HEAD)
                    .unwrap_or_else(|| panic!("{name}: no traceback from {side}: {text:?}"));
                let last = tb.lines().last().unwrap_or_default();
                let class = last.split(':').next().unwrap_or_default().to_string();
                (before.to_string(), class)
            };
            assert_eq!(
                split(&r.stderr, "the binary"),
                split(&b.stderr, "bash"),
                "{name}: stderr before the traceback, and its exception\nrust: {}\nbash: {}",
                r.stderr,
                b.stderr
            );
        } else if matches!(c.compare, Compare::Waited | Compare::InPipe { .. }) {
            let (bs, bn) = mask_waited(&b.stderr);
            let (rs, rn) = mask_waited(&r.stderr);
            assert_eq!(rs, bs, "{name}: stderr (seconds masked)");
            assert_eq!(rn.len(), bn.len(), "{name}: the waited lines");
            for (x, y) in rn.iter().zip(&bn) {
                assert!(
                    x.abs_diff(*y) <= 1,
                    "{name}: waited {x} s here, {y} s in bash"
                );
            }
        } else {
            assert_eq!(r.stderr, b.stderr, "{name}: stderr");
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_file(&self.dead_sock);
    }
}

/// `text` with the number in each `within <n> s` and `after <n> s`
/// masked, and the numbers.
pub fn mask_waited(text: &str) -> (String, Vec<u64>) {
    let mut out = String::new();
    let mut nums = Vec::new();
    let mut rest = text;
    loop {
        let next = ["within ", "after "]
            .iter()
            .filter_map(|w| rest.find(w).map(|at| (at, w.len())))
            .min();
        let Some((at, len)) = next else {
            out.push_str(rest);
            return (out, nums);
        };
        let start = at + len;
        out.push_str(&rest[..start]);
        let digits: String = rest[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !digits.is_empty() && rest[start + digits.len()..].starts_with(" s") {
            nums.push(digits.parse().expect("a whole number of seconds"));
            out.push_str("<n>");
            rest = &rest[start + digits.len()..];
        } else {
            rest = &rest[start..];
        }
    }
}

/// A logged request with the expected differences masked, each counted.
/// `native`: the binary sent it itself.
pub fn normalise(r: &Logged, native: bool, verb: &str, case: &str) -> String {
    let version = k5_cli::embedded::version();
    let text = String::from_utf8(r.bytes.clone())
        .unwrap_or_else(|_| panic!("{case}: a request that is not UTF-8"));
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("{case}: a request with no head: {text:?}"));
    let (mut ua, mut via, mut ver, mut noun) = (0, 0, 0, 0);
    let mut lines: Vec<String> = Vec::new();
    let mut head_lines = head.split("\r\n");
    lines.push(fold_query_escapes(
        head_lines
            .next()
            .unwrap_or_else(|| panic!("{case}: no request line")),
    ));
    let noun_line = format!("X-K5-Cli-Noun: {verb}");
    for line in head_lines {
        let masked = if native {
            if line == format!("User-Agent: k5/{version}") {
                ua += 1;
                Some("User-Agent: <ua>")
            } else if line == format!("X-K5-Cli-Version: {version}") {
                ver += 1;
                continue;
            } else if line == "X-K5-Cli-Via: native" {
                via += 1;
                Some("X-K5-Cli-Via: <via>")
            } else {
                None
            }
        } else if line.starts_with("User-Agent: curl/") {
            ua += 1;
            Some("User-Agent: <ua>")
        } else if line == "X-K5-Cli-Via: fallthrough" {
            via += 1;
            Some("X-K5-Cli-Via: <via>")
        } else if line == noun_line {
            noun += 1;
            continue;
        } else {
            None
        };
        lines.push(
            masked
                .map(str::to_string)
                .unwrap_or_else(|| line.to_string()),
        );
    }
    let side = if native { "native" } else { "the shell" };
    assert_eq!(
        (ua, via, ver, noun),
        (1, 1, usize::from(native), usize::from(!native)),
        "{case} ({side}): the masked lines in {text:?}"
    );
    format!("[{}]\n{}\n\n{body}", r.channel, lines.join("\n"))
}

/// The request line with the hex of each `%XX` escape in its query in
/// upper case (as in `tests/transport_parity.rs`).
pub fn fold_query_escapes(request_line: &str) -> String {
    let Some((before, query)) = request_line.split_once('?') else {
        return request_line.to_string();
    };
    let (query, after) = query
        .rsplit_once(' ')
        .unwrap_or_else(|| panic!("no HTTP version after the query: {request_line:?}"));
    let b = query.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            out.extend_from_slice(&[
                b'%',
                b[i + 1].to_ascii_uppercase(),
                b[i + 2].to_ascii_uppercase(),
            ]);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    format!(
        "{before}?{} {after}",
        String::from_utf8(out).expect("upper-casing ASCII hex keeps UTF-8")
    )
}

pub fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

pub fn owner_and_port(r: &Rig) {
    r.owner_on_disk();
    r.port_on_disk();
}

pub fn port_only(r: &Rig) {
    r.port_on_disk();
}

pub fn owner_only(r: &Rig) {
    r.owner_on_disk();
}

pub fn nothing(_: &Rig) {}

pub fn healed_on_disk(r: &Rig) {
    r.port_on_disk();
    std::fs::write(r.secrets.join(".k2-passport"), format!("{HEALED}\n")).unwrap();
}

/// A refusal that writes the re-minted passport first.
pub fn heal(rig: &Rig) -> Reply {
    Reply::WriteThen {
        file: rig.secrets.join(".k2-passport"),
        content: format!("{HEALED}\n"),
        status: 403,
        body: REFUSED.to_string(),
    }
}

/// The passport's settings, and `extra`.
pub fn with_passport(port: u16, extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    [
        ("K2_PORT", port.to_string()),
        ("K2_HOOK_TOKEN", PASSPORT.to_string()),
        ("K2_SECRETS_DIR", "{secrets}".to_string()),
    ]
    .into_iter()
    .chain(extra.iter().map(|(k, v)| (*k, v.to_string())))
    .collect()
}

pub fn case(
    name: &'static str,
    args: &'static [&'static str],
    env: &[(&'static str, &str)],
    replies: Vec<Reply>,
    setup: fn(&Rig),
    code: i32,
    requests: usize,
) -> Case {
    Case {
        name,
        args,
        env: env.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        replies,
        setup,
        cwd: None,
        no_tmpdir: false,
        code,
        requests,
        compare: Compare::Exact,
    }
}

/// A port nothing listens on.
pub fn dead_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port().to_string()
}

/// A symlink to the workspace folder, for a PWD through a link.
pub fn link_to_proj(rig: &Rig) -> PathBuf {
    let link = rig.dir.join("link");
    if !link.exists() {
        std::os::unix::fs::symlink(&rig.proj, &link).unwrap();
    }
    link
}
