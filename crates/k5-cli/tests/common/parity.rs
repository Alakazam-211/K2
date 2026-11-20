//! The rig a native family's parity test runs on: `cli/k2 <verb>` (as the
//! front hands it a verb) and `k5-cli <verb>`, each run as people run it
//! against a logging stub daemon ([`super::stub`]). For every case both
//! get the same environment, folder and answers, and must send the same
//! requests and print the same stdout and stderr with the same exit code.
//! `tests/whoami_parity.rs` was the first of these, written before the rig.
//!
//! Each side ([`Side`]) has its own stub: its own port (the stub binds it
//! and reads it back), its own socket and its own request log. A case's
//! settings name the port as `{port}` and the socket as `{sock}`, filled
//! in for the side that runs. Each request a side's stub logged must be
//! that side's (its `User-Agent` and `X-K5-Cli-Via`), and every request
//! any stub got must have been read by its side's run: one that wasn't
//! came from a CLI pointed at a stub not its own, a crossed request, and
//! fails the case.
//!
//! The shell runs with `K5_CLI_VIA=rust`, and `K5_CLI_WOULD_REFUSE=1`
//! when the passport rule says so, so its requests carry the hand-over
//! headers. The normaliser treats only these as expected differences,
//! each counted (exactly one per request):
//!
//! - `User-Agent`: `curl/<version>` from bash (`Python-urllib/<version>`
//!   from a family whose shell side is a `python3` block), `k5/<version>`
//!   here;
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
//! Each case then runs a third time: the binary alone, with no `python3`
//! on `PATH` ([`super::path_without_python3`]). It must send the same
//! requests and print the same as with one (K2's ruling: a native command
//! runs where python3 is missing, [`k5_cli::python::NATIVE_WITHOUT_PYTHON3`]),
//! and a case the binary hands to the shell (it wrote the shell CLI out
//! to run it) must print the missing-Python line instead, exit 69 and
//! send nothing. Its TMPDIR starts as the binary's did (the bulletin
//! line's stamp lives there). A case that differs there on
//! purpose says so ([`NoPython3::Own`]) and is checked by its own test.
//!
//! A rig whose command runs other programs ([`Rig::path_first`]) puts a
//! folder of stub programs first on `PATH`, on every side; they write
//! what they were run with to [`Rig::tool_log`], and each side's log must
//! be the other's (a hand-off with no `python3` runs none of them).
//!
//! Hermetic: HOME, TMPDIR and the K2 home are scratch folders, the tokens
//! are made up, and each side gets nothing from the environment but PATH
//! and what the case sets.

#![cfg(unix)]

use super::stub::{Logged, Reply, Stub};
use std::cell::{Cell, OnceCell};
use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
/// A case's setting for the stub's port, filled in for the side that runs.
pub const PORT: &str = "{port}";

pub fn cli_k2() -> PathBuf {
    let p = super::cli_src().join("../k2");
    assert!(p.is_file(), "no shell CLI at {}", p.display());
    p
}

static N: AtomicUsize = AtomicUsize::new(0);

/// A run of a case, each against its own stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// `cli/k2`.
    Bash,
    /// The binary.
    Rust,
    /// The binary with no `python3` on `PATH`.
    NoPython3,
}

impl Side {
    pub const ALL: [Side; 3] = [Side::Bash, Side::Rust, Side::NoPython3];

    fn index(self) -> usize {
        match self {
            Side::Bash => 0,
            Side::Rust => 1,
            Side::NoPython3 => 2,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Side::Bash => "bash",
            Side::Rust => "rust",
            Side::NoPython3 => "rust with no python3",
        }
    }

    /// The side of a run: `bash`, else the binary, with `PATH` given
    /// only when it has no `python3`.
    fn of(bash: bool, path: Option<&str>) -> Side {
        match (bash, path) {
            (true, _) => Side::Bash,
            (false, None) => Side::Rust,
            (false, Some(_)) => Side::NoPython3,
        }
    }
}

pub struct Rig {
    /// The first word, before each case's arguments; empty for none (the
    /// program's name alone, which the shell reads as `help`).
    pub verb: &'static str,
    pub dir: PathBuf,
    pub home: PathBuf,
    pub k2home: PathBuf,
    pub proj: PathBuf,
    pub secrets: PathBuf,
    /// A socket file nothing listens on.
    pub dead_sock: PathBuf,
    /// The binary the Rust sides run: [`BIN`], or a test binary
    /// (`k5-spec-fixture`).
    pub bin: &'static str,
    /// One stub per side, in [`Side::ALL`]'s order.
    stubs: Vec<Stub>,
    /// The side being prepared or run.
    side: Cell<Side>,
    /// A test's hook ([`Rig::cross`]): this side's CLI talks to that
    /// side's stub.
    cross: Cell<Option<(Side, Side)>>,
    /// `PATH` with no `python3` on it, made when first asked for.
    no_python3: OnceCell<String>,
    /// A folder put first on `PATH`, on every side.
    pub path_first: Option<PathBuf>,
    /// The file the stub programs in [`Rig::path_first`] log to.
    pub tool_log: Option<PathBuf>,
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
    /// on stdout. That is `scripts/cli-parity.sh`'s named difference
    /// `bash32-pipe-reader`, which the gate checks too.
    Bash32PipeReader {
        stdout: &'static str,
        stderr: &'static str,
    },
    /// A NUL byte in the answer, which bash's `resp=$(cli_request …)`
    /// drops and, from bash 4.4 on, warns about on stderr in one line
    /// naming `cli/k2`'s path and line, while the binary drops it
    /// silently (`scripts/cli-parity.sh`'s named difference
    /// `nul-in-answer`): everything as [`Compare::Exact`] but bash's
    /// stderr, which must be the binary's plus that one line (none on a
    /// bash before 4.4).
    NulInAnswer,
    /// The shell's Python raises: everything as [`Compare::Exact`] but
    /// the traceback on stderr, where only what comes before it and its
    /// last line's exception name must match (its frames and wording
    /// depend on the Python version; the binary prints 3.12's).
    Traceback,
    /// Both sides run the system's `mktemp`, which picks a random name:
    /// everything as [`Compare::Exact`] but stderr, compared with the six
    /// random characters after `k2-update.` masked ([`mask_temp_names`]).
    /// macOS's mktemp names the file in its error; GNU's doesn't.
    MktempName,
}

/// `scripts/cli-parity.sh`'s named difference `bash32-empty-array`, for
/// K2 to rule on: bash before 4.4 (macOS's `/bin/bash` 3.2) takes
/// `"${arr[@]}"` of an empty array for an unbound variable under `set -u`.
/// There bash writes one line, `<cli/k2>: line <n>: <array>[@]: unbound
/// variable`, naming one of `arrays`. With `stops`, bash ends there with
/// that exit after that many requests (the binary's first ones) and
/// nothing on stdout, and the binary is held to the row's own exit and
/// requests; without, that line is bash's only difference. With bash 4.4
/// or newer the row compares as it says. The shell's fix (the file's own
/// `${arr[@]+"${arr[@]}"}`) makes bash 3.2 answer as the binary does: the
/// row then fails, saying the mark goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyArray {
    pub arrays: &'static [&'static str],
    pub stops: Option<(i32, usize)>,
}

/// `c` marked [`EmptyArray`].
pub fn bash32_empty_array(
    c: Case,
    arrays: &'static [&'static str],
    stops: Option<(i32, usize)>,
) -> Case {
    Case {
        bash32_empty_array: Some(EmptyArray { arrays, stops }),
        ..c
    }
}

/// Each `k2-update.` followed by six letters or digits (the random part
/// `mktemp k2-update.XXXXXX` fills in), with those six as `XXXXXX`.
pub fn mask_temp_names(text: &str) -> String {
    const STEM: &str = "k2-update.";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(STEM) {
        let (head, tail) = rest.split_at(at + STEM.len());
        out.push_str(head);
        let six: String = tail.chars().take(6).collect();
        if six.len() == 6 && six.chars().all(|c| c.is_ascii_alphanumeric()) {
            out.push_str("XXXXXX");
            rest = &tail[6..];
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// What a case's run with no `python3` on `PATH` must do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoPython3 {
    /// What the binary does with one, or, for a case it hands to the
    /// shell, the missing-Python line.
    AsWith,
    /// Something else, on purpose; the case's own test checks it.
    Own,
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
    pub no_python3: NoPython3,
    /// What each side reads on stdin; `None`: nothing (stdin is closed).
    pub stdin: Option<&'static [u8]>,
    /// How long each side may run before the row fails as hung
    /// ([`DEADLINE`] when `None`).
    pub deadline: Option<Duration>,
    /// bash 3.2 fails on an empty array here ([`EmptyArray`]).
    pub bash32_empty_array: Option<EmptyArray>,
}

/// How long one side of a row may run: a side still running then is
/// killed (its whole process group) and the row fails, naming it. Above
/// the longest wait a side makes on purpose: a daemon that never answers
/// is curl's `--max-time` (60 s for a GET, 120 s for a compute POST).
pub const DEADLINE: Duration = Duration::from_secs(180);

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
            &dir.join("tmp-nopy"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let short = verb.chars().next().unwrap_or('h');
        let stubs = ['b', 'r', 'n']
            .iter()
            .map(|s| {
                Stub::start(&PathBuf::from(format!(
                    "/tmp/k5{short}{s}-{}-{n}.sock",
                    std::process::id()
                )))
            })
            .collect();
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
            bin: BIN,
            stubs,
            side: Cell::new(Side::Bash),
            cross: Cell::new(None),
            no_python3: OnceCell::new(),
            path_first: None,
            tool_log: None,
        }
    }

    /// `side`'s own stub: its log is that side's requests.
    pub fn stub_of(&self, side: Side) -> &Stub {
        &self.stubs[side.index()]
    }

    /// The stub `side`'s CLI talks to: its own, unless [`Rig::cross`]
    /// says otherwise.
    fn target(&self, side: Side) -> &Stub {
        match self.cross.get() {
            Some((from, to)) if from == side => self.stub_of(to),
            _ => self.stub_of(side),
        }
    }

    /// The stub the side being prepared or run talks to.
    pub fn stub(&self) -> &Stub {
        self.target(self.side.get())
    }

    /// From now on `from`'s CLI talks to `to`'s stub: a crossed request,
    /// for the test that proves the rig fails on one.
    pub fn cross(&self, from: Side, to: Side) {
        self.cross.set(Some((from, to)));
    }

    /// The words after the program's name: the verb, if there is one,
    /// then the case's arguments.
    pub fn argv(&self, case: &Case) -> Vec<&'static str> {
        let verb = (!self.verb.is_empty()).then_some(self.verb);
        verb.into_iter().chain(case.args.iter().copied()).collect()
    }

    /// The word the shell's requests name the command by in
    /// `X-K5-Cli-Noun` (`_k5_headers`): `${1:-help}`, or `other` when it
    /// is not a lower-case word (`_K5_NOUN_RE`).
    pub fn noun(&self) -> &'static str {
        let noun = if self.verb.is_empty() {
            "help"
        } else {
            self.verb
        };
        let b = noun.as_bytes();
        let word = b.len() <= 32
            && b[0].is_ascii_lowercase()
            && b[1..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
        if word {
            noun
        } else {
            "other"
        }
    }

    /// `PATH` with no `python3` on it.
    pub fn path_without_python3(&self) -> &str {
        self.no_python3
            .get_or_init(|| super::path_without_python3(&self.dir))
    }

    pub fn owner_on_disk(&self) {
        std::fs::write(self.k2home.join("heartbeat.token"), format!("{OWNER}\n")).unwrap();
    }

    pub fn port_on_disk(&self) {
        std::fs::write(
            self.k2home.join("heartbeat.port"),
            format!("{}\n", self.stub().port),
        )
        .unwrap();
    }

    fn fill(&self, side: Side, v: &str) -> String {
        let stub = self.target(side);
        v.replace(PORT, &stub.port.to_string())
            .replace("{sock}", &stub.sock.to_string_lossy())
            .replace("{deadsock}", &self.dead_sock.to_string_lossy())
            .replace("{nosock}", &self.dir.join("no-socket").to_string_lossy())
            .replace("{secrets}", &self.secrets.to_string_lossy())
    }

    /// One side's run. `bash`: `cli/k2` as the front hands it a verb;
    /// else the binary.
    pub fn run(&self, bash: bool, case: &Case) -> Out {
        self.run_on(bash, case, None)
    }

    /// [`Rig::run`], with `path` as `PATH` when given.
    fn run_on(&self, bash: bool, case: &Case, path: Option<&str>) -> Out {
        let side = Side::of(bash, path);
        self.side.set(side);
        let mut cmd = if bash {
            let mut c = Command::new("bash");
            c.arg(cli_k2());
            c
        } else {
            Command::new(self.bin)
        };
        cmd.env_clear();
        let search = path
            .map(OsString::from)
            .or_else(|| std::env::var_os("PATH"));
        let search = match (&self.path_first, search) {
            (Some(first), Some(rest)) => {
                let mut joined = first.clone().into_os_string();
                joined.push(":");
                joined.push(rest);
                Some(joined)
            }
            (Some(first), None) => Some(first.clone().into_os_string()),
            (None, search) => search,
        };
        if let Some(search) = search {
            cmd.env("PATH", search);
        }
        let tmp = if case.no_tmpdir {
            self.dir.join("no-tmp")
        } else {
            self.dir.join(match side {
                Side::Bash => "tmp-bash",
                Side::Rust => "tmp-rust",
                Side::NoPython3 => "tmp-nopy",
            })
        };
        cmd.env("HOME", &self.home)
            .env("TMPDIR", &tmp)
            .current_dir(case.cwd.as_deref().unwrap_or(&self.proj));
        let env: Vec<(String, String)> = case
            .env
            .iter()
            .map(|(k, v)| (k.to_string(), self.fill(side, v)))
            .collect();
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        if bash {
            cmd.env("K5_CLI_VIA", "rust");
            let lookup = |name: &str| {
                env.iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| OsString::from(v))
            };
            let args: Vec<OsString> = self.argv(case).into_iter().map(OsString::from).collect();
            let verdict = k5_cli::passport::decide(
                k5_cli::passport::facts(&args, lookup),
                k5_cli::passport::live_session_ancestor,
            );
            if verdict.would_refuse() {
                cmd.env("K5_CLI_WOULD_REFUSE", "1");
            }
        }
        cmd.args(self.argv(case));
        let o = run_within(
            &mut cmd,
            case.stdin,
            case.deadline.unwrap_or(DEADLINE),
            &format!("{}: the {side:?} side", case.name),
        );
        let signal = o.status.signal();
        Out {
            code: o.status.code().or_else(|| signal.map(|s| 128 + s)),
            signal,
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// Each side of a case starts from an empty K2 home, no re-minted
    /// passport, an empty log and the case's answers, once every request
    /// so far was read by its own side ([`Rig::settle`]).
    pub fn prepare(&self, side: Side, c: &Case) {
        self.settle(c.name);
        self.side.set(side);
        for f in [
            self.secrets.join(".k2-passport"),
            self.k2home.join("heartbeat.port"),
            self.k2home.join("heartbeat.token"),
            self.k2home.join("daemon.token"),
        ] {
            let _ = std::fs::remove_file(f);
        }
        // The shell CLI the binary writes out to hand a command over.
        let _ = std::fs::remove_dir_all(self.k2home.join("run"));
        self.take_tool_log();
        (c.setup)(self);
        self.stub_of(side).reset(&[]);
        self.stub().reset(&c.replies);
    }

    /// Every request a stub got was read by its own side's run. One
    /// nobody read came from a CLI talking to a stub not its own side's.
    fn settle(&self, name: &str) {
        for side in Side::ALL {
            let stray = self.stub_of(side).unread();
            assert!(
                stray.is_empty(),
                "{name}: crossed: {} request(s) in the {} side's stub that its own run did not send:\n{}",
                stray.len(),
                side.name(),
                heads(&stray)
            );
        }
    }

    /// `side`'s requests, from its own stub, each checked to be one that
    /// side sent: the binary's own (`native`) or the shell's. Then no
    /// stub may hold a request nobody read ([`Rig::settle`]).
    pub fn read(&self, side: Side, native: bool, name: &str) -> Vec<Logged> {
        let log = self.stub_of(side).take_log();
        for r in &log {
            let by = sent_by(r);
            assert_eq!(
                by,
                Some(native),
                "{name}: crossed: the {} side's stub has a request {} sent, not {}:\n{}",
                side.name(),
                sender(by),
                sender(Some(native)),
                heads(std::slice::from_ref(r))
            );
        }
        self.settle(name);
        log
    }

    /// A normalised request with its `Host` line's port, the port of the
    /// stub `side` talks to, written `{port}`: each side's stub has its
    /// own. Leading zeros stay.
    fn on_port(&self, side: Side, request: String) -> String {
        let port = self.target(side).port.to_string();
        request
            .split('\n')
            .map(|line| {
                if !line.starts_with("Host: ") {
                    return line.to_string();
                }
                let Some((host, digits)) = line.rsplit_once(':') else {
                    return line.to_string();
                };
                let zeros = digits.len() - digits.trim_start_matches('0').len();
                if digits[zeros..] == port {
                    format!("{host}:{}{PORT}", &digits[..zeros])
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What the stub programs logged since the last call, emptied.
    pub fn take_tool_log(&self) -> String {
        let Some(log) = &self.tool_log else {
            return String::new();
        };
        if !log.exists() {
            return String::new();
        }
        let text =
            std::fs::read_to_string(log).unwrap_or_else(|e| panic!("read {}: {e}", log.display()));
        std::fs::remove_file(log).unwrap_or_else(|e| panic!("remove {}: {e}", log.display()));
        text
    }

    /// Whether the binary handed its command to the shell: it wrote the
    /// shell CLI out first.
    fn handed_off(&self) -> bool {
        k5_cli::fallthrough::cache_dir(&self.k2home).exists()
    }

    /// The run with no `python3` gets a TMPDIR as the binary's is now.
    fn mirror_tmp(&self) {
        let to = self.dir.join("tmp-nopy");
        let _ = std::fs::remove_dir_all(&to);
        copy_tree(&self.dir.join("tmp-rust"), &to);
    }

    /// The binary's run of `c`, with or without `python3` on `PATH`, and
    /// its requests as the binary's own.
    pub fn binary(&self, c: &Case, without_python3: bool) -> (Out, Vec<String>) {
        if without_python3 {
            self.mirror_tmp();
        }
        let path = without_python3.then(|| self.path_without_python3());
        let side = Side::of(false, path);
        self.prepare(side, c);
        let out = self.run_on(false, c, path);
        let log = self
            .read(side, true, c.name)
            .iter()
            .map(|r| self.on_port(side, normalise(r, true, self.noun(), c.name)))
            .collect();
        (out, log)
    }

    /// Runs both sides of `c` and compares them, then the binary with no
    /// `python3` ([`NoPython3`]).
    pub fn check(&self, c: &Case) {
        self.mirror_tmp();
        let (with, log, handed, tools) = self.check_parity(c);
        if c.no_python3 == NoPython3::AsWith {
            self.check_without_python3(c, &with, &log, handed, &tools);
        }
        self.settle(c.name);
    }

    /// The binary with no `python3` on `PATH` against its run `with` one
    /// (whose requests were `with_log` and stub programs' log
    /// `with_tools`; `handed`: it went to the shell).
    fn check_without_python3(
        &self,
        c: &Case,
        with: &Out,
        with_log: &[String],
        handed: bool,
        with_tools: &str,
    ) {
        let name = c.name;
        self.prepare(Side::NoPython3, c);
        let out = self.run_on(false, c, Some(self.path_without_python3()));
        let sent = self.read(Side::NoPython3, true, name);
        let tools = self.take_tool_log();
        println!("-- rust, no python3: {out:?}");
        if handed {
            let words: Vec<OsString> = self.argv(c).into_iter().map(OsString::from).collect();
            let line = k5_cli::fallthrough::FrontError::NoPython {
                noun: k5_cli::fallthrough::noun(&words),
            }
            .to_string();
            assert_eq!(
                sent.len(),
                0,
                "{name}: sent a request with no python3 for the shell"
            );
            assert_eq!(
                tools, "",
                "{name}: ran a program with no python3 for the shell"
            );
            assert_eq!(
                (out.code, out.stdout.as_str(), out.stderr.as_str()),
                (Some(69), "", format!("{line}\n").as_str()),
                "{name}: a hand-off with no python3"
            );
            return;
        }
        assert_eq!(
            self.stub().left(),
            0,
            "{name}: answers left unused with no python3: {out:?}"
        );
        let log: Vec<String> = sent
            .iter()
            .map(|r| self.on_port(Side::NoPython3, normalise(r, true, self.noun(), name)))
            .collect();
        assert_eq!(
            log,
            with_log,
            "{name}: the requests with no python3\n== without ==\n{}\n== with ==\n{}",
            log.join("\n----\n"),
            with_log.join("\n----\n")
        );
        assert_eq!(
            (out.code, out.signal),
            (with.code, with.signal),
            "{name}: the exit with no python3\nwithout: {out:?}\nwith: {with:?}"
        );
        assert_eq!(out.stdout, with.stdout, "{name}: stdout with no python3");
        assert_eq!(
            tools, with_tools,
            "{name}: the programs run with no python3"
        );
        if matches!(
            c.compare,
            Compare::Waited | Compare::Bash32PipeReader { .. }
        ) {
            let (ws, wn) = mask_waited(&with.stderr);
            let (os, on) = mask_waited(&out.stderr);
            assert_eq!(os, ws, "{name}: stderr with no python3 (seconds masked)");
            assert!(
                on.len() == wn.len() && on.iter().zip(&wn).all(|(x, y)| x.abs_diff(*y) <= 1),
                "{name}: the waited lines with no python3"
            );
        } else if c.compare == Compare::MktempName {
            assert_eq!(
                mask_temp_names(&out.stderr),
                mask_temp_names(&with.stderr),
                "{name}: stderr with no python3 (mktemp's name masked)"
            );
        } else {
            assert_eq!(out.stderr, with.stderr, "{name}: stderr with no python3");
        }
    }

    /// Runs both sides of `c` and compares them. The binary's run, its
    /// requests, whether it handed the command to the shell, and what the
    /// stub programs logged.
    fn check_parity(&self, c: &Case) -> (Out, Vec<String>, bool, String) {
        let name = c.name;
        let mut outs = Vec::new();
        let mut logs = Vec::new();
        let mut tools = Vec::new();
        let mut handed = false;
        let empty_array = c.bash32_empty_array.filter(|_| bash_version() < (4, 4));
        for bash in [true, false] {
            let side = Side::of(bash, None);
            self.prepare(side, c);
            let out = self.run(bash, c);
            tools.push(self.take_tool_log());
            if !bash {
                handed = self.handed_off();
            }
            // bash 3.2 stopping at an empty array leaves the rest unread.
            if !(bash && empty_array.is_some_and(|m| m.stops.is_some())) {
                assert_eq!(
                    self.stub().left(),
                    0,
                    "{name}: {} left answers unused: {out:?}",
                    side.name()
                );
            }
            let native = !bash && c.compare != Compare::Shell;
            logs.push(
                self.read(side, native, name)
                    .iter()
                    .map(|r| self.on_port(side, normalise(r, native, self.noun(), name)))
                    .collect::<Vec<_>>(),
            );
            outs.push(out);
        }
        let (mut b, r) = (outs[0].clone(), &outs[1]);
        let (mut bl, rl) = (logs[0].clone(), logs[1].clone());
        assert_eq!(
            tools[1], tools[0],
            "{name}: the programs run (rust, then bash)"
        );
        let rt = tools[1].clone();
        println!(
            "== {name}\n{}\n-- bash: {b:?}\n-- rust: {r:?}",
            bl.join("\n----\n").replace('\r', "")
        );
        if let Some(m) = empty_array {
            let rest = without_unbound_line(name, &b.stderr, m.arrays);
            if let Some((code, n)) = m.stops {
                assert_eq!(
                    (b.code, b.signal, b.stdout.as_str(), rest.as_str()),
                    (Some(code), None, "", ""),
                    "{name}: bash {:?} at the empty array: {b:?}",
                    bash_version()
                );
                assert!(
                    bl.len() == n && rl.len() >= n && bl[..] == rl[..n],
                    "{name}: bash's {} requests before the empty array, want the binary's first {n}\n== rust ==\n{}\n== bash ==\n{}",
                    bl.len(),
                    rl.join("\n----\n"),
                    bl.join("\n----\n")
                );
                assert_eq!(
                    (r.code, rl.len()),
                    (Some(c.code), c.requests),
                    "{name}: the binary's exit and requests: {r:?}"
                );
                return (r.clone(), rl, handed, rt);
            }
            b.stderr = rest;
        }
        let bash_3_2_pipe = match c.compare {
            Compare::Bash32PipeReader { stdout, stderr }
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
            // The credential is the Bearer header over TCP and on the
            // cell socket alike.
            let bearer = |t: &str| format!("Authorization: Bearer {t}\n");
            let sent = |l: &str, t: &str| l.contains(&bearer(t));
            assert!(
                bl.iter().all(|l| sent(l, OWNER)),
                "{name}: bash no longer sends the owner token: {bl:?}"
            );
            assert!(
                rl.iter().all(|l| sent(l, PASSPORT) && !l.contains(OWNER)),
                "{name}: the binary: {rl:?}"
            );
            bl = bl
                .iter()
                .map(|l| l.replace(&bearer(OWNER), &bearer(PASSPORT)))
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
            return (r.clone(), rl, handed, rt);
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
        } else if c.compare == Compare::NulInAnswer {
            let head = format!("{}: line ", cli_k2().display());
            let warning = |l: &str| {
                l.strip_prefix(&head)
                    .is_some_and(|after| after.ends_with(NUL_WARNING))
            };
            let (warned, rest): (Vec<&str>, Vec<&str>) =
                b.stderr.split_inclusive('\n').partition(|l| warning(l));
            assert_eq!(
                warned.len(),
                usize::from(bash_warns_on_nul()),
                "{name}: bash's NUL warning lines (bash warns from 4.4 on): {:?}",
                b.stderr
            );
            assert_eq!(
                r.stderr,
                rest.concat(),
                "{name}: stderr but bash's NUL warning"
            );
            assert!(
                !r.stderr.contains("null byte"),
                "{name}: the binary wrote about the NUL: {:?}",
                r.stderr
            );
        } else if matches!(
            c.compare,
            Compare::Waited | Compare::Bash32PipeReader { .. }
        ) {
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
        } else if c.compare == Compare::MktempName {
            assert_eq!(
                mask_temp_names(&r.stderr),
                mask_temp_names(&b.stderr),
                "{name}: stderr (mktemp's name masked)"
            );
        } else {
            assert_eq!(
                as_this_python(&r.stderr, &b.stderr),
                b.stderr,
                "{name}: stderr"
            );
        }
        (r.clone(), rl, handed, rt)
    }
}

/// The binary's text as the shell's Python words it, where that Python
/// words it otherwise than 3.12, which the binary follows (it has no Python
/// version to copy, as with the timeout's name in
/// urllib_python_parity.rs): `scripts/cli-parity.sh`'s named difference
/// `python-version-error-text`. The version is the one of the `python3` the
/// shell runs, read from it, never guessed.
/// - Before 3.11, "string indices must be integers" ends there (3.11
///   added ", not 'str'").
/// - From 3.13, a JSON trailing comma is "Illegal trailing comma before
///   end of object" (or array), placed at the comma, where 3.12 says
///   "Expecting property name enclosed in double quotes" (or "Expecting
///   value") after it. A line of the binary with 3.12's words takes the
///   shell's line when that one is 3.13's trailing comma and the rest of
///   the two lines is the same; the place can't be worked out from the
///   binary's line alone.
fn as_this_python(rust: &str, bash: &str) -> String {
    let older = as_before_311(rust);
    if python3_version() < (3, 13) || older == bash {
        return older;
    }
    let r: Vec<&str> = older.split_inclusive('\n').collect();
    let b: Vec<&str> = bash.split_inclusive('\n').collect();
    if r.len() != b.len() {
        return older;
    }
    r.iter()
        .zip(&b)
        .map(|(rl, bl)| {
            match (
                json_error(rl, PY312_TRAILING),
                json_error(bl, PY313_TRAILING),
            ) {
                // The same head and tail: only Python's words and place differ.
                (Some(x), Some(y)) if x == y => (*bl).to_string(),
                _ => (*rl).to_string(),
            }
        })
        .collect()
}

/// What 3.12 says at a JSON trailing comma (after it).
const PY312_TRAILING: &[&str] = &[
    "Expecting property name enclosed in double quotes",
    "Expecting value",
];

/// What 3.13 and later say there (at the comma).
const PY313_TRAILING: &[&str] = &[
    "Illegal trailing comma before end of object",
    "Illegal trailing comma before end of array",
];

/// `line` as `<head><one of msgs>: line <n> column <n> (char <n>)<tail>`:
/// its head and tail.
fn json_error<'a>(line: &'a str, msgs: &[&str]) -> Option<(&'a str, &'a str)> {
    msgs.iter().find_map(|m| {
        let at = line.find(m)?;
        let rest = line[at + m.len()..].strip_prefix(": line ")?;
        let num = |s: &'a str| -> Option<&'a str> {
            let n = s.bytes().take_while(u8::is_ascii_digit).count();
            (n > 0).then(|| &s[n..])
        };
        let rest = num(rest)?.strip_prefix(" column ")?;
        let rest = num(rest)?.strip_prefix(" (char ")?;
        let tail = num(rest)?.strip_prefix(')')?;
        Some((&line[..at], tail))
    })
}

/// Before 3.11, "string indices must be integers" with no ", not '...'".
fn as_before_311(text: &str) -> String {
    const STEM: &str = "string indices must be integers";
    if python3_version() >= (3, 11) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(STEM) {
        let (head, tail) = rest.split_at(at + STEM.len());
        out.push_str(head);
        rest = tail
            .strip_prefix(", not '")
            .and_then(|t| t.find('\'').map(|end| &t[end + 1..]))
            .unwrap_or(tail);
    }
    out.push_str(rest);
    out
}

/// The `python3` on `PATH` (the one the shell runs): its major and minor
/// version.
pub fn python3_version() -> (u32, u32) {
    static V: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        let o = Command::new("python3")
            .args([
                "-I",
                "-c",
                "import sys; print('%d %d' % sys.version_info[:2])",
            ])
            .output()
            .expect("run python3");
        assert!(o.status.success(), "python3's version: {o:?}");
        let text = String::from_utf8(o.stdout).expect("python3's version is UTF-8");
        let v: Vec<u32> = text
            .split_whitespace()
            .map(|n| n.parse().expect("a number in python3's version"))
            .collect();
        assert_eq!(v.len(), 2, "python3's version: {text:?}");
        (v[0], v[1])
    })
}

/// How bash's warning about a NUL that `$(…)` dropped ends, after
/// `<script>: line <n>`.
const NUL_WARNING: &str = ": warning: command substitution: ignored null byte in input\n";

/// `stderr` without its one line `<cli/k2>: line <n>: <array>[@]: unbound
/// variable` naming one of `arrays` ([`EmptyArray`]).
fn without_unbound_line(name: &str, stderr: &str, arrays: &[&str]) -> String {
    let head = format!("{}: line ", cli_k2().display());
    let unbound = |l: &str| {
        l.strip_prefix(&head)
            .and_then(|after| after.split_once(": "))
            .is_some_and(|(n, what)| {
                !n.is_empty()
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && arrays
                        .iter()
                        .any(|a| what == format!("{a}[@]: unbound variable\n"))
            })
    };
    let (lines, rest): (Vec<&str>, Vec<&str>) =
        stderr.split_inclusive('\n').partition(|l| unbound(l));
    assert_eq!(
        lines.len(),
        1,
        "{name}: bash {:?} wrote {} lines naming {arrays:?} as an unbound variable, want 1 \
         (none: the shell's fix has landed, so the bash32-empty-array mark goes): {stderr:?}",
        bash_version(),
        lines.len()
    );
    rest.concat()
}

/// The `bash` the rig runs: its major and minor version.
fn bash_version() -> (u32, u32) {
    static V: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        let o = Command::new("bash")
            .args(["-c", "echo \"${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}\""])
            .output()
            .expect("run bash");
        assert!(o.status.success(), "bash's version: {o:?}");
        let text = String::from_utf8(o.stdout).expect("bash's version is UTF-8");
        let v: Vec<u32> = text
            .split_whitespace()
            .map(|n| n.parse().expect("a number in bash's version"))
            .collect();
        assert_eq!(v.len(), 2, "bash's version: {text:?}");
        (v[0], v[1])
    })
}

/// Whether the `bash` the rig runs warns when `$(…)` drops a NUL byte:
/// from 4.4 on.
fn bash_warns_on_nul() -> bool {
    bash_version() >= (4, 4)
}

/// `from`'s files and folders copied into `to` (made first).
pub fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|e| panic!("make {}: {e}", to.display()));
    for entry in std::fs::read_dir(from).unwrap_or_else(|e| panic!("read {}: {e}", from.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("read {}: {e}", from.display()));
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst)
                .unwrap_or_else(|e| panic!("copy {} to {}: {e}", src.display(), dst.display()));
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_file(&self.dead_sock);
    }
}

/// `text` with the number in each `within <n> s`, `after <n> s` and `(<n> s`
/// masked, and the numbers.
pub fn mask_waited(text: &str) -> (String, Vec<u64>) {
    let mut out = String::new();
    let mut nums = Vec::new();
    let mut rest = text;
    loop {
        // `within N s`, `after N s`, and the may-have-been-applied line's
        // `(N s)`.
        let next = ["within ", "after ", "("]
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

/// The shell's `User-Agent`s: curl's, and urllib's for the families whose
/// shell side is a `python3` block (`k5_cli::transport::urllib`).
const SHELL_CURL: &str = "User-Agent: curl/";
const SHELL_PYTHON: &str = "User-Agent: Python-urllib/";

/// Who sent `r`: the binary (`true`: `User-Agent: k5/...` and
/// `X-K5-Cli-Via: native`), the shell (`false`: `curl/...` or
/// `Python-urllib/...`, and
/// `fallthrough`), or neither.
fn sent_by(r: &Logged) -> Option<bool> {
    let text = String::from_utf8_lossy(&r.bytes);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    let has = |prefix: &str| head.split("\r\n").any(|l| l.starts_with(prefix));
    if has("User-Agent: k5/") && has("X-K5-Cli-Via: native") {
        Some(true)
    } else if (has(SHELL_CURL) || has(SHELL_PYTHON)) && has("X-K5-Cli-Via: fallthrough") {
        Some(false)
    } else {
        None
    }
}

fn sender(by: Option<bool>) -> &'static str {
    match by {
        Some(true) => "the binary",
        Some(false) => "the shell",
        None => "neither side",
    }
}

/// Each request's head, for a failure.
fn heads(log: &[Logged]) -> String {
    log.iter()
        .map(|r| {
            let text = String::from_utf8_lossy(&r.bytes);
            let head = text.split("\r\n\r\n").next().unwrap_or_default();
            format!("[{}] {}", r.channel, head.replace("\r\n", "\n  "))
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        } else if line.starts_with(SHELL_CURL) || line.starts_with(SHELL_PYTHON) {
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

/// The passport's settings (the stub's port), and `extra`.
pub fn with_passport(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    [
        ("K2_PORT", PORT.to_string()),
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
        no_python3: NoPython3::AsWith,
        stdin: None,
        deadline: None,
        bash32_empty_array: None,
    }
}

/// Runs `cmd` in its own process group with `input` on stdin (closed
/// when `None`) and its output read as it comes. When it is still running
/// after `deadline`, its process group is killed and the row fails: `who`
/// names the row and the side.
pub fn run_within(
    cmd: &mut Command,
    input: Option<&'static [u8]>,
    deadline: Duration,
    who: &str,
) -> std::process::Output {
    use std::io::{Read, Write};
    use std::os::unix::process::CommandExt;
    cmd.process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
    let started = Instant::now();
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                p.read_to_end(&mut buf).expect("read a side's output");
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    if let Some(input) = input {
        let mut pipe = child.stdin.take().expect("its stdin");
        // A side that stops reading early closes the pipe; any other
        // failure to hand it its input is.
        if let Err(e) = pipe.write_all(input) {
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::BrokenPipe,
                "stdin of {cmd:?}: {e}"
            );
        }
    }
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .unwrap_or_else(|e| panic!("wait {cmd:?}: {e}"))
        {
            break status;
        }
        if started.elapsed() >= deadline {
            let group = i32::try_from(child.id()).expect("a pid");
            // SAFETY: a plain kill of the group the child leads.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
            let _ = child.wait();
            let stdout = out.join().expect("its stdout");
            let stderr = err.join().expect("its stderr");
            panic!(
                "{who} hung: still running after {deadline:?}, so its process group was killed\n\
                 stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    std::process::Output {
        status,
        stdout: out.join().expect("its stdout"),
        stderr: err.join().expect("its stderr"),
    }
}

/// A port nothing listens on, and no stub can be given
/// ([`super::stub::dead_port`]).
pub fn dead_port() -> String {
    super::stub::dead_port().to_string()
}

/// A symlink to the workspace folder, for a PWD through a link.
pub fn link_to_proj(rig: &Rig) -> PathBuf {
    let link = rig.dir.join("link");
    if !link.exists() {
        std::os::unix::fs::symlink(&rig.proj, &link).unwrap();
    }
    link
}
