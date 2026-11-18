//! `k5-cli whoami` against the shell CLI's `cli/k2 whoami`, each run as
//! people run it, against one logging stub daemon ([`common::stub`]): for
//! every case both get the same environment, folder and answers, and must
//! send the same requests and print the same stdout and stderr with the
//! same exit code.
//!
//! The shell runs as the front hands it a verb (`K5_CLI_VIA=rust`, and
//! `K5_CLI_WOULD_REFUSE=1` when the passport rule says so), so its
//! requests carry the hand-over headers. The normaliser treats only these
//! as expected differences, each counted (exactly one per request):
//!
//! - `User-Agent`: `curl/<version>` from bash, `k5/<version>` here;
//! - `X-K5-Cli-Version: <version>`: sent by the binary only, dropped;
//! - `X-K5-Cli-Via`: `fallthrough` from bash, `native` from the binary;
//! - `X-K5-Cli-Noun: whoami`: sent by bash only (the daemon counts nouns
//!   of fallthroughs), dropped;
//! - the case of each `%XX` escape in the query (curl 8.5 and 8.7 write
//!   `-G`'s in lower case; the binary writes upper case; K2's ruling).
//!
//! One case differs on purpose, by PRD 13.9 #4, and is checked as such:
//! in a session with a cell socket and the owner token on disk, bash's
//! open tools send the owner token over TCP (COMPAT-58); the binary sends
//! the passport. The requests are otherwise the same.
//!
//! Where the shell's Python fails with a traceback (an answer that is JSON
//! but no object, a `siblings.live` that is no number), its text depends
//! on the Python version, so those cases compare the exit code, stdout and
//! the traceback's last line's exception name.
//!
//! Each case then runs the binary once more with no `python3` on `PATH`
//! (K2's ruling: a native command runs where python3 is missing): the
//! same requests and the same output as with one.
//!
//! Hermetic: HOME, TMPDIR and the K2 home are scratch folders, the tokens
//! are made up, and each side gets nothing from the environment but PATH
//! and what the case sets.

#![cfg(unix)]

mod common;

use common::stub::{Logged, Reply, Stub};
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
const OWNER: &str = "owner-tok-7171";
const PASSPORT: &str = "sessA.secret7272";
const HEALED: &str = "sessA.secret7373";
const REFUSED: &str = r#"{"error":"Invalid or missing auth token"}"#;
const FULL: &str = r#"{"workspace":"sales","role":"sidecar","address":"sales/reviewer","primary":"sales","session":"s1","path":"/w/sales","siblings":{"live":2,"others":[]}}"#;

fn cli_k2() -> PathBuf {
    let p = common::cli_src().join("../k2");
    assert!(p.is_file(), "no shell CLI at {}", p.display());
    p
}

static N: AtomicUsize = AtomicUsize::new(0);

struct Rig {
    dir: PathBuf,
    home: PathBuf,
    k2home: PathBuf,
    proj: PathBuf,
    secrets: PathBuf,
    stub: Stub,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k5-whoami-{tag}-{}-{n}", std::process::id()));
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
            &dir.join("tmp-nopy"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let stub = Stub::start(&PathBuf::from(format!(
            "/tmp/k5w-{}-{n}.sock",
            std::process::id()
        )));
        Rig {
            dir,
            home,
            k2home,
            proj,
            secrets,
            stub,
        }
    }

    fn owner_on_disk(&self) {
        std::fs::write(self.k2home.join("heartbeat.token"), format!("{OWNER}\n")).unwrap();
    }

    fn port_on_disk(&self) {
        std::fs::write(
            self.k2home.join("heartbeat.port"),
            format!("{}\n", self.stub.port),
        )
        .unwrap();
    }

    /// One side's run. `bash`: `cli/k2` as the front hands it a verb;
    /// else the binary.
    fn run(&self, bash: bool, case: &Case) -> Out {
        self.run_on(bash, case, None)
    }

    /// [`Rig::run`], with `path` as `PATH` when given.
    fn run_on(&self, bash: bool, case: &Case, path: Option<&str>) -> Out {
        let mut cmd = if bash {
            let mut c = Command::new("bash");
            c.arg(cli_k2());
            c
        } else {
            Command::new(BIN)
        };
        cmd.env_clear();
        match path {
            Some(path) => {
                cmd.env("PATH", path);
            }
            None => {
                if let Some(path) = std::env::var_os("PATH") {
                    cmd.env("PATH", path);
                }
            }
        }
        let tmp = if case.no_tmpdir {
            self.dir.join("no-tmp")
        } else {
            self.dir.join(match (bash, path) {
                (true, _) => "tmp-bash",
                (false, None) => "tmp-rust",
                (false, Some(_)) => "tmp-nopy",
            })
        };
        cmd.env("HOME", &self.home)
            .env("TMPDIR", &tmp)
            .current_dir(case.cwd.as_deref().unwrap_or(&self.proj));
        let env: Vec<(String, String)> = case
            .env
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.replace("{sock}", &self.stub.sock.to_string_lossy())
                        .replace("{secrets}", &self.secrets.to_string_lossy()),
                )
            })
            .collect();
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        if bash {
            cmd.env("K5_CLI_VIA", "rust");
            let lookup = |name: &str| {
                env.iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| OsString::from(v))
            };
            let args: Vec<OsString> = std::iter::once("whoami")
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
        cmd.arg("whoami").args(case.args);
        let o = cmd.output().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A logged request with the expected differences masked, each counted.
fn normalise(r: &Logged, rust: bool, case: &str) -> String {
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
    for line in head_lines {
        let masked = if rust {
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
        } else if line == "X-K5-Cli-Noun: whoami" {
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
    let side = if rust { "rust" } else { "bash" };
    assert_eq!(
        (ua, via, ver, noun),
        (1, 1, usize::from(rust), usize::from(!rust)),
        "{case} ({side}): the masked lines in {text:?}"
    );
    format!("[{}]\n{}\n\n{body}", r.channel, lines.join("\n"))
}

/// The request line with the hex of each `%XX` escape in its query in
/// upper case (as in `tests/transport_parity.rs`).
fn fold_query_escapes(request_line: &str) -> String {
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

/// How a case compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compare {
    /// Everything, byte for byte.
    Exact,
    /// The shell's Python fails with a traceback: stderr's last line's
    /// exception name only.
    Traceback,
    /// PRD 13.9 #4: bash sends the owner token, the binary the passport.
    OwnerTokenWithPassport,
    /// A no-answer line's seconds may be one apart: bash counts whole
    /// seconds of `$SECONDS`, which ticks on its own
    /// ([`common::parity::mask_waited`]).
    Waited,
}

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    /// `{sock}` is the stub's socket, `{secrets}` the secrets folder.
    env: Vec<(&'static str, String)>,
    replies: Vec<Reply>,
    /// Run before each side (files the case needs).
    setup: fn(&Rig),
    cwd: Option<PathBuf>,
    /// TMPDIR names a folder that isn't there.
    no_tmpdir: bool,
    /// Bash's exit code and request count: the case proves what it says.
    code: i32,
    requests: usize,
    compare: Compare,
}

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

fn owner_and_port(r: &Rig) {
    r.owner_on_disk();
    r.port_on_disk();
}

fn port_only(r: &Rig) {
    r.port_on_disk();
}

fn case(
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

/// Each case starts from an empty K2 home, no re-minted passport and its
/// own answers.
fn prepare(rig: &Rig, c: &Case) {
    for f in [
        rig.secrets.join(".k2-passport"),
        rig.k2home.join("heartbeat.port"),
        rig.k2home.join("heartbeat.token"),
        rig.k2home.join("daemon.token"),
    ] {
        let _ = std::fs::remove_file(f);
    }
    (c.setup)(rig);
    rig.stub.reset(&c.replies);
}

fn check(rig: &Rig, c: &Case) {
    // The run with no python3 gets a TMPDIR as the binary's is now (the
    // bulletin line's stamp lives there).
    let nopy = rig.dir.join("tmp-nopy");
    let _ = std::fs::remove_dir_all(&nopy);
    common::parity::copy_tree(&rig.dir.join("tmp-rust"), &nopy);
    let (with, with_log) = check_parity(rig, c);
    let name = c.name;
    prepare(rig, c);
    let path = common::path_without_python3(&rig.dir);
    let out = rig.run_on(false, c, Some(&path));
    println!("-- rust, no python3: {out:?}");
    assert_eq!(
        rig.stub.left(),
        0,
        "{name}: answers left unused with no python3"
    );
    let log: Vec<String> = rig
        .stub
        .log()
        .iter()
        .map(|r| normalise(r, true, name))
        .collect();
    assert_eq!(log, with_log, "{name}: the requests with no python3");
    assert_eq!(out.code, with.code, "{name}: the exit with no python3");
    assert_eq!(out.stdout, with.stdout, "{name}: stdout with no python3");
    if c.compare == Compare::Waited {
        let (ws, wn) = common::parity::mask_waited(&with.stderr);
        let (os, on) = common::parity::mask_waited(&out.stderr);
        assert_eq!(os, ws, "{name}: stderr with no python3 (seconds masked)");
        assert!(
            on.len() == wn.len() && on.iter().zip(&wn).all(|(x, y)| x.abs_diff(*y) <= 1),
            "{name}: the waited lines with no python3"
        );
    } else {
        assert_eq!(out.stderr, with.stderr, "{name}: stderr with no python3");
    }
}

/// Runs both sides of `c` and compares them. The binary's run and its
/// requests.
fn check_parity(rig: &Rig, c: &Case) -> (Out, Vec<String>) {
    let name = c.name;
    let mut outs = Vec::new();
    let mut logs = Vec::new();
    for bash in [true, false] {
        prepare(rig, c);
        let out = rig.run(bash, c);
        let side = if bash { "bash" } else { "rust" };
        assert_eq!(
            rig.stub.left(),
            0,
            "{name}: {side} left answers unused: {out:?}"
        );
        logs.push(
            rig.stub
                .log()
                .iter()
                .map(|r| normalise(r, !bash, name))
                .collect::<Vec<_>>(),
        );
        outs.push(out);
    }
    let (b, r) = (&outs[0], &outs[1]);
    let (mut bl, rl) = (logs[0].clone(), logs[1].clone());
    println!(
        "== {name}\n{}\n-- bash: {b:?}",
        bl.join("\n----\n").replace('\r', "")
    );
    assert_eq!(b.code, Some(c.code), "{name}: bash's exit: {b:?}");
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
    assert_eq!(
        r.code, b.code,
        "{name}: the exit code\nrust: {r:?}\nbash: {b:?}"
    );
    assert_eq!(r.stdout, b.stdout, "{name}: stdout");
    if c.compare == Compare::Waited {
        let (bs, bn) = common::parity::mask_waited(&b.stderr);
        let (rs, rn) = common::parity::mask_waited(&r.stderr);
        assert_eq!(rs, bs, "{name}: stderr (seconds masked)");
        assert_eq!(rn.len(), bn.len(), "{name}: the waited lines");
        for (x, y) in rn.iter().zip(&bn) {
            assert!(
                x.abs_diff(*y) <= 1,
                "{name}: waited {x} s here, {y} s in bash"
            );
        }
    } else if c.compare == Compare::Traceback {
        let last = |s: &str| {
            s.lines()
                .last()
                .and_then(|l| l.split(':').next())
                .map(str::to_string)
        };
        assert!(
            b.stderr.starts_with("Traceback (most recent call last):\n"),
            "{name}: bash: {b:?}"
        );
        assert_eq!(
            last(&r.stderr),
            last(&b.stderr),
            "{name}: the exception\nrust: {}\nbash: {}",
            r.stderr,
            b.stderr
        );
    } else {
        assert_eq!(r.stderr, b.stderr, "{name}: stderr");
    }
    (r.clone(), rl)
}

/// A port nothing listens on.
fn dead_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port().to_string()
}

fn heal(rig: &Rig) -> Reply {
    Reply::WriteThen {
        file: rig.secrets.join(".k2-passport"),
        content: format!("{HEALED}\n"),
        status: 403,
        body: REFUSED.to_string(),
    }
}

fn healed_on_disk(r: &Rig) {
    r.port_on_disk();
    std::fs::write(r.secrets.join(".k2-passport"), format!("{HEALED}\n")).unwrap();
}

fn cases(rig: &Rig) -> Vec<Case> {
    let port = rig.stub.port.to_string();
    let passport: [(&'static str, String); 3] = [
        ("K2_PORT", port.clone()),
        ("K2_HOOK_TOKEN", PASSPORT.to_string()),
        ("K2_SECRETS_DIR", "{secrets}".to_string()),
    ];
    let with = |extra: &[(&'static str, &'static str)]| -> Vec<(&'static str, String)> {
        passport
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .chain(extra.iter().map(|(k, v)| (*k, v.to_string())))
            .collect()
    };
    let permission_ask = r#"{"error":{"code":"owner_only","hint":"Only the owner can do that."},"permission":{"command":"k2 permission request --scope whoami.read --why \"...\"","requestable":true}}"#;
    let permission_none = r#"{"error":"nope","hint":"ask a person","permission":{"requestable":false,"hint":"Only a person can change this."}}"#;
    let mut v = vec![
        case(
            "owner from disk, the plain form",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "owner from disk, --json",
            &["--json"],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "owner from disk, in a session by its markers",
            &[],
            &[("K2_CELL", "canonical")],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the session's names, under each prefix",
            &["--json"],
            &[
                ("K5_SESSION_ID", "s 5/é"),
                ("K2_SESSION_ID", "s2"),
                ("K5_CELL", ""),
                ("K2_CELL", "sidecar"),
                ("K2SO_SIDECAR_NAME", "rev&1"),
                ("K2_PRIMARY", "acme"),
            ],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "words that are not flags are ignored",
            &["extra", "--json", "help"],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an unknown flag",
            &["--bogus"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "a lone dash is a flag",
            &["--json", "-"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "no port: the connection gate",
            &[],
            &[],
            vec![],
            |r| r.owner_on_disk(),
            1,
            0,
        ),
        case(
            "no token: the connection gate",
            &[],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
        case(
            "an empty port setting is no port",
            &[],
            &[("K5_PORT", "")],
            vec![],
            |r| r.owner_on_disk(),
            1,
            0,
        ),
        case(
            "a port that is no port",
            &[],
            &[("K2_PORT", "41o0")],
            vec![],
            |r| r.owner_on_disk(),
            3,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "the guard, under the K5 name, comes before the flags",
            &["--bogus"],
            &[("K5_NO_OWNER_FALLBACK", "1")],
            vec![],
            |r| r.owner_on_disk(),
            3,
            0,
        ),
        Case {
            env: with(&[("K2_NO_OWNER_FALLBACK", "1")]),
            ..case(
                "the guard lets a passport through",
                &[],
                &[],
                vec![ok(FULL)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a passport over TCP",
                &[],
                &[],
                vec![ok(FULL)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "a passport and a live cell socket: still TCP",
                &["--json"],
                &[],
                vec![ok(FULL)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with(&[("K2_HOOK_SOCK", "{sock}")]),
            compare: Compare::OwnerTokenWithPassport,
            ..case(
                "a passport, a cell socket and the owner token on disk (PRD 13.9 #4)",
                &[],
                &[],
                vec![ok(FULL)],
                owner_and_port,
                0,
                1,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a re-minted passport is used from the start",
                &["--json"],
                &[],
                vec![ok(FULL)],
                healed_on_disk,
                0,
                1,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a refused passport heals once",
                &["--json"],
                &[],
                vec![heal(rig), ok(FULL)],
                port_only,
                0,
                2,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a dead passport: the probe, then the restart line",
                &["--json"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                1,
                2,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a refused passport the probe finds good: no line",
                &[],
                &[],
                vec![Reply::Answer(403, REFUSED.into()), ok(FULL)],
                port_only,
                1,
                2,
            )
        },
        case(
            "a refusal's closing line, the plain form",
            &[],
            &[],
            vec![Reply::Answer(403, permission_ask.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal's closing line, --json",
            &["--json"],
            &[],
            vec![Reply::Answer(403, permission_ask.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal that can't be requested",
            &[],
            &[],
            vec![Reply::Answer(403, permission_none.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no identity here",
            &[],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"cannot determine identity","hint":"run inside a workspace session"}"#
                    .into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object with no hint or code",
            &[],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"msg":"é \"q\" 'x'","n":[1.5,null,true]}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json without a role",
            &["--json"],
            &[],
            vec![ok(r#"{"workspace":"w"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json drops the answer's trailing newlines",
            &["--json"],
            &[],
            vec![ok("{\"role\":\"r\"}\n\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an answer that is not JSON",
            &[],
            &[],
            vec![ok("<html>oops</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer with 200",
            &[],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Waited,
            ..case(
                "a 5xx with an empty body: no answer",
                &[],
                &[],
                vec![Reply::Answer(502, String::new())],
                owner_and_port,
                1,
                1,
            )
        },
        case(
            "migrating: asked again",
            &[],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(FULL),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "Python's str() of each value",
            &[],
            &[],
            vec![ok(
                r#"{"workspace":null,"role":1.0,"address":["a",{"b":false}],"primary":1e16,"session":-0,"siblings":{"live":2.9}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &[],
            &[],
            vec![ok(
                r#"{"role":"r","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no bulletin line when it failed",
            &["--json"],
            &[],
            vec![ok(
                r#"{"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "an answer that is no object",
                &[],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "a siblings count that is no number",
                &[],
                &[],
                vec![ok(r#"{"role":"r","siblings":{"live":"2"}}"#)],
                owner_and_port,
                1,
                1,
            )
        },
    ];
    // PWD names the workspace through a link: both keep the link's path.
    let link = rig.dir.join("link");
    if !link.exists() {
        std::os::unix::fs::symlink(&rig.proj, &link).unwrap();
    }
    v.push(Case {
        env: vec![("PWD", link.to_string_lossy().into_owned())],
        cwd: Some(link.clone()),
        ..case(
            "PWD through a link",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        )
    });
    // PROJECT_PATH wins over the folder.
    v.push(case(
        "PROJECT_PATH",
        &[],
        &[("K2_PROJECT_PATH", "/w/proj é&")],
        vec![ok(FULL)],
        owner_and_port,
        0,
        1,
    ));
    v
}

#[test]
fn whoami_answers_as_the_shell_does() {
    let rig = Rig::new("all");
    for c in cases(&rig) {
        check(&rig, &c);
    }
}

/// Nothing accepted the connection: exit 7, nothing printed, after the
/// tries a second apart, TMPDIR or not (a refused connect is never the
/// machine's).
#[test]
fn whoami_with_no_daemon() {
    let rig = Rig::new("dead");
    for no_tmpdir in [false, true] {
        let c = Case {
            env: vec![("K2_PORT", dead_port())],
            no_tmpdir,
            ..case(
                "nothing listens",
                &[],
                &[],
                vec![],
                |r| r.owner_on_disk(),
                7,
                0,
            )
        };
        check(&rig, &c);
    }
}

/// The other transport failures end with curl's code, as `set -e` passes
/// it on, and print nothing: a reset is 56; with TMPDIR unusable a failure
/// is this machine's, `_cli_curl`'s 75, a port that is no port too.
#[test]
fn whoami_with_a_broken_exchange() {
    let rig = Rig::new("broken");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        case(
            "a reset (56)",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "a reset, --json (56)",
            &["--json"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable: a reset is 75",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            75,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable: a port that is no port is 75",
            &[],
            &[("K2_PORT", "41o0")],
            vec![],
            |r| r.owner_on_disk(),
            75,
            0,
        )),
        no_tmp(case(
            "TMPDIR unusable: an answer is an answer",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        )),
    ];
    for c in v {
        check(&rig, &c);
    }
}

/// git: the 0.40.4 rename line while `.k2so/` files are still in git's
/// index, and a worktree's main folder when no folder up from here is a
/// workspace.
#[test]
fn whoami_and_git() {
    let rig = Rig::new("git");
    let git = |dir: &Path, args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("HOME", &rig.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    // The rename line: a .k2so file in the index, the folder gone, .k2 there.
    git(&rig.proj, &["init", "-q"]);
    std::fs::create_dir_all(rig.proj.join(".k2so")).unwrap();
    std::fs::write(rig.proj.join(".k2so/note.md"), "x").unwrap();
    git(&rig.proj, &["add", ".k2so/note.md"]);
    std::fs::remove_dir_all(rig.proj.join(".k2so")).unwrap();
    let mut c = case(
        "the rename line",
        &[],
        &[],
        vec![ok(FULL)],
        owner_and_port,
        0,
        1,
    );
    check(&rig, &c);
    let probe = rig.run(false, &c);
    assert!(
        probe.stderr.starts_with("i  "),
        "the rename line is not there: {probe:?}"
    );

    // A worktree outside the workspace: the main folder is the project.
    let main = rig.dir.join("main");
    std::fs::create_dir_all(main.join(".k2")).unwrap();
    git(&main, &["init", "-q"]);
    git(
        &main,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ],
    );
    let wt = rig.dir.join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    c = Case {
        cwd: Some(wt.clone()),
        ..case(
            "a worktree finds its main folder",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        )
    };
    check(&rig, &c);
    let log = rig.stub.log();
    let main_q =
        k5_cli::transport::encode::quote(&std::fs::canonicalize(&main).unwrap().to_string_lossy());
    let line = String::from_utf8_lossy(&log[0].bytes)
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert!(
        fold_query_escapes(&line).contains(&format!("project={main_q}&")),
        "not the main folder: {line}"
    );
}
