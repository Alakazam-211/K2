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

#![cfg(unix)]

mod common;

use common::parity::{cli_k2, fold_query_escapes, run_within, BIN, OWNER};
use common::stub::{Logged, Reply, Stub};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static N: AtomicUsize = AtomicUsize::new(0);

const SIDES: [&str; 3] = ["bash", "rust", "nopy"];

struct Side {
    dir: PathBuf,
    home: PathBuf,
    k2home: PathBuf,
    local: Stub,
    remote: Stub,
}

struct CRig {
    sides: Vec<Side>,
    no_python3: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Exact,
    /// The shell's Python raises: stderr up to the traceback and its last
    /// line's exception name.
    Traceback,
}

struct CCase {
    name: &'static str,
    /// `{remote}` is the side's remote stub (`http://127.0.0.1:<port>`).
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
}

impl CRig {
    fn new() -> CRig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = PathBuf::from(format!("/tmp/k5ce-{}-{n}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
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
                    local,
                    remote,
                }
            })
            .collect();
        CRig {
            sides,
            no_python3: common::path_without_python3(&root),
        }
    }

    fn mask(&self, side: &Side, text: &str) -> String {
        let t = text.replace(&side.dir.display().to_string(), "<dir>");
        mask_ids(&t)
    }

    fn normalise(&self, side: &Side, r: &Logged, case: &str) -> String {
        let text = String::from_utf8_lossy(&r.bytes).into_owned();
        let (head, body) = text
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("{case}: a request with no head"));
        let mut lines = head.split("\r\n");
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
        self.mask(side, &format!("[{}]\n{}\n\n{body}", r.channel, out.join("\n")))
    }

    fn run(&self, i: usize, c: &CCase) -> Ran {
        let side = &self.sides[i];
        let fill = |v: &str| {
            v.replace("{remote}", &format!("http://127.0.0.1:{}", side.remote.port))
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
        std::fs::write(
            side.k2home.join("heartbeat.port"),
            format!("{}\n", side.local.port),
        )
        .unwrap();
        if let Some(t) = c.tokens {
            std::fs::write(side.k2home.join("connect-tokens.json"), fill(t)).unwrap();
        }
        for (name, bytes) in &c.files {
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
            b.arg(cli_k2()).env("K5_CLI_VIA", "rust");
            b
        } else {
            Command::new(BIN)
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
        let o = run_within(
            &mut cmd,
            c.stdin,
            Duration::from_secs(120),
            &format!("{}: {}", c.name, SIDES[i]),
        );
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
        }
    }

    fn check(&self, c: &CCase) {
        let bash = self.run(0, c);
        assert_eq!(
            (bash.code, bash.local.len(), bash.remote.len()),
            (Some(c.code), c.local_requests, c.remote_requests),
            "{}: bash's exit and requests (the case must prove what it says)\n{bash:#?}",
            c.name
        );
        for i in [1, 2] {
            let mut got = self.run(i, c);
            let mut want = clone_ran(&bash);
            if c.cmp == Cmp::Traceback {
                want.stderr = traceback_shape(&want.stderr);
                got.stderr = traceback_shape(&got.stderr);
            }
            assert_eq!(got, want, "{}: the {} side", c.name, SIDES[i]);
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

// covers: clone preview
// covers: clone preview --json
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
        // covers: clone <ws>
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

// covers: clone to
// covers: clone from
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
            ..case("to: one chunk, the token from the env", &["to", "{remote}", "ws1"])
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
            ..case("to --token-stdin --json --dest", &["to", "{remote}", "ws1", "--token-stdin", "--json", "--dest", "/there"])
        },
        CCase {
            local: to_local.clone(),
            remote: vec![ok(RECIPIENT), ok(r#"{"separator":"/"}"#)],
            files: vec![("bundle-1.tar", DATA)],
            local_requests: 2,
            remote_requests: 2,
            code: 3,
            ..case("to: the remote names no home", &["to", "{remote}", "ws1", "--token", "t2"])
        },
        CCase {
            local: vec![ok(RECIPIENT)],
            remote: vec![
                ok(PREVIEW_PLAIN),
                ok(r#"{"job_id":"job-1"}"#),
                ok(r#"{"phase":"packing"}"#),
                ok(&format!(r#"{{"phase":"done","bundle_path":"/rb/bundle-job-1.tar","size_bytes":{}}}"#, DATA.len())),
                ok(&format!(r#"{{"base64":"{b64}","eof":true}}"#)),
                ok(r#"{"ok":true}"#),
            ],
            local_requests: 1,
            remote_requests: 6,
            ..case("from: pack, wait, download, unpack here", &["from", "{remote}:ws1", "--token", "t3"])
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
            ..case("from: a short download leaves no .part", &["from", "{remote}:ws1", "--token", "t3"])
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
            ..case("from: a refused range, the cleanup still asked", &["from", "{remote}:ws1", "--token", "t3"])
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
            ..case("from: the pack failed", &["from", "{remote}:ws1", "--token", "t3"])
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

// covers: clone approve
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
            ..case("approve here by a prefix: one confirm approves it", &["approve", "cp-7"])
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
            ..case("approve a host: no saved token, nothing sent", &["approve", "cp-78"])
        }
        .env(vec![("K2_REMOTE_TOKEN", "never-sent")]),
        CCase {
            local: vec![ok(PROPOSALS_HOST), ok(PROPOSALS_HOST)],
            tokens: Some(r#"{"{remote}": "saved-tok"}"#),
            local_requests: 2,
            code: 3,
            ..case("approve a host: no terminal, nothing sent there", &["approve", "cp-78", "--yes"])
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
            ..case("approve: a failed run is finished as not ok", &["approve", "cp-77", "--json"])
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
            ..case("to: plain http to a name is refused", &["to", "http://h.example:9", "ws1", "--token", "t"])
        },
        CCase {
            code: 3,
            ..case("from: a name before an @ is refused", &["from", "https://ann:pw@h.example:ws1"])
        },
        CCase {
            code: 3,
            ..case("to https: no token", &["to", "https://h.example", "ws1"])
        },
        CCase {
            code: 2,
            stdin: Some(b""),
            ..case("--token-stdin with nothing on stdin", &["here", "ws1", "--token-stdin"])
        },
        CCase {
            code: 2,
            ..case("--token-stdin and --token", &["to", "h", "w", "--token-stdin", "--token", "x"])
        },
        CCase {
            code: 2,
            ..case("approve takes no --token", &["approve", "cp-1", "--token", "x"])
        },
        CCase {
            code: 3,
            env: vec![("K2_HOOK_TOKEN", "hk")],
            ..case("an agent can't clone", &["here", "ws1"])
        },
        CCase {
            code: 2,
            env: vec![("K2_HOST", "h.example")],
            ..case("this server over plain http: the warning once, then the usage", &["preview"])
        },
        CCase {
            code: 2,
            ..case("a bare word with no --dest", &["ws1"])
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

#[test]
fn mask_ids_masks_only_ids() {
    assert_eq!(
        mask_ids("a 0f3c2b1a-1111-4abc-8def-0123456789ab b clone-cli-0123456789ab c"),
        "a <uuid> b clone-cli-<id> c"
    );
    assert_eq!(mask_ids("not-a-uuid"), "not-a-uuid");
}
