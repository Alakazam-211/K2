//! The shell's Codex tab gate (`_cli_codex_tab_gate`, between
//! `BEGIN_CLI_CODEX_TAB_BIND` and `END_CLI_CODEX_TAB_BIND` in
//! `cli/src/cli-tools-policy.sh`, called first thing in
//! `cli/src/router.sh`), run by the front before it routes anything.
//!
//! A Codex tool call (Codex sets `CODEX_THREAD_ID` on every one) in a K2
//! session keeps the session's credentials only when the tab marker
//! (`K2_CODEX_TAB_ISOLATED`) names the session of the passport it carries
//! and no shared Codex app-server is above it. Any other such call sends
//! nothing: help, schema, version and explain run (help with the cards
//! off and agents' reports on, asked of nobody), `<command> -h|--help`
//! prints `help <command>`, and everything else ends with exit 3. The
//! owner token is never used for it.
//!
//! The shell runs this gate after its prelude's other steps: the air-gap
//! refusals, the virtual desktop's guard, the connection gate and the
//! plain-http warning. The front answers a gated line itself only where
//! it knows those steps let the line by; anywhere else, and wherever it
//! can't tell what the shell's process walk would find, it hands the line
//! to the shell ([`Verdict::Shell`]), whose own earlier exit then wins.
//!
//! The families' own narrower checks (a `CODEX_` setting with the socket
//! set hands the line to the shell) stay: they stand for the shell's
//! id-tool check (`_cli_id_check_codex`), which also runs with this gate
//! switched off (`K2_CODEX_PASSPORT_BIND=0`) and for `CODEX_` settings
//! other than the thread id.

use crate::dashdash::{self, Stray};
use crate::exit::Exit;
use crate::front::{self, Route};
use crate::offline::{self, Env};
use crate::pages::{self, Page};
use crate::text::Variant;
use crate::{out, passport, python};
use k2_client::env_names;
use k2_client::{cli_name, product_name};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

/// The setting Codex sets on every tool call.
pub const THREAD_ID: &str = "CODEX_THREAD_ID";
/// The switch: `0`, `off`, `false` or `no` turns the gate off.
pub const BIND_SWITCH: &str = "K2_CODEX_PASSPORT_BIND";
/// The tab marker: the session id of the passport of a tab K2 started
/// with its own Codex server.
pub const TAB_MARKER: &str = "K2_CODEX_TAB_ISOLATED";

/// The note when nothing may look at the processes above the call.
pub const BLIND_NOTE: &str = concat!(
    "Note: this host lets nothing look at the processes above this Codex call, so ",
    product_name!(),
    "'s tab marker alone confirms the tab."
);

/// Set once the gate let a help word by: the permission cards read as off
/// and agents' reports as on, and neither is asked (the shell's
/// `_K2_PERMISSION_ON=0` and `_K2_SF_AGENTS_OFF=0`).
static HELD: AtomicBool = AtomicBool::new(false);

/// Whether the gate holds the cards' and agents' answers ([`HELD`]).
pub fn held() -> bool {
    HELD.load(Ordering::Relaxed)
}

/// What the processes above the call hold (`_K2_ANC_STATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anc {
    /// The chain is complete and has no shared Codex app-server.
    Clear,
    /// Every way to look was refused: the marker alone decides.
    Blind,
    /// A shared Codex app-server is above the call.
    Shared,
    /// An answer was missing, partial or unreadable: counts as shared.
    Broken,
}

/// What the shell's gate does with a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Not a Codex call in a K2 session, or the switch is off: nothing
    /// changes.
    Off,
    /// A tab K2 confirmed: the command runs.
    Confirmed,
    /// Confirmed by the marker alone: [`BLIND_NOTE`], then the command.
    Blind,
    /// The binary can't tell what the shell's process walk finds.
    Unknown,
    /// A word the gate lets by (help, schema, version, explain).
    Allowed,
    /// `-h` or `--help` after the first word: `cmd_help <word>`, exit 0.
    HelpOf(OsString),
    /// Exit 3 with [`refusal`]'s lines.
    Refuse { verb: OsString, anc: Option<Anc> },
}

/// `_cli_codex_bind_on`: a Codex call with the switch on.
pub fn bind_on(lookup: &impl Fn(&str) -> Option<OsString>) -> bool {
    if lookup(THREAD_ID).is_none_or(|v| v.is_empty()) {
        return false;
    }
    !lookup(BIND_SWITCH).is_some_and(|v| matches!(v.to_str(), Some("0" | "off" | "false" | "no")))
}

fn first(names: &[&'static str], lookup: &impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    env_names::first_nonempty_os_in(names, lookup).map(|(_, v)| v)
}

/// `_cli_codex_gate_applies`: a K2 session, by its socket or by a pane id
/// with a passport.
pub fn applies(lookup: &impl Fn(&str) -> Option<OsString>) -> bool {
    first(env_names::HOOK_SOCK, lookup).is_some()
        || (first(env_names::PANE_ID, lookup).is_some()
            && first(env_names::HOOK_TOKEN, lookup).is_some())
}

/// The marker names the session of the passport (`SCOPED_TOKEN`): the
/// passport has a `.`, and what comes before its first `.` is the marker.
/// (A re-minted passport keeps its session's id, so the one in the
/// environment answers the same.)
pub fn marker_matches(lookup: &impl Fn(&str) -> Option<OsString>) -> bool {
    let Some(marker) = lookup(TAB_MARKER).filter(|m| !m.is_empty()) else {
        return false;
    };
    let Some(token) = first(env_names::HOOK_TOKEN, lookup) else {
        return false;
    };
    let t = token.as_encoded_bytes();
    t.iter()
        .position(|b| *b == b'.')
        .is_some_and(|dot| &t[..dot] == marker.as_encoded_bytes())
}

/// A word `[Mm][Pp][Cc]-*` or `[Mm][Pp][Cc][Ee]-*`: a Markdown kit id,
/// which `explain` hands to `doc` (it asks the daemon).
fn kit_id(word: &OsStr) -> bool {
    let b = word.as_encoded_bytes();
    let lower = |i: usize| b.get(i).map(u8::to_ascii_lowercase);
    lower(0) == Some(b'm')
        && lower(1) == Some(b'p')
        && lower(2) == Some(b'c')
        && (b.get(3) == Some(&b'-') || (lower(3) == Some(b'e') && b.get(4) == Some(&b'-')))
}

/// `_cli_codex_tab_gate "${1:-help}" "${@:2}"`. `ancestry` reads the
/// processes above the call (`_cli_codex_ancestry`); it runs only when
/// the marker matches, as the shell's does, and `None` is a walk the
/// binary can't do as the shell would.
pub fn decide(
    args: &[OsString],
    lookup: impl Fn(&str) -> Option<OsString>,
    ancestry: impl FnOnce() -> Option<Anc>,
) -> Decision {
    if !bind_on(&lookup) || !applies(&lookup) {
        return Decision::Off;
    }
    let mut anc = None;
    if marker_matches(&lookup) {
        match ancestry() {
            None => return Decision::Unknown,
            Some(Anc::Clear) => return Decision::Confirmed,
            Some(Anc::Blind) => return Decision::Blind,
            Some(a) => anc = Some(a),
        }
    }
    let verb = args
        .first()
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| OsString::from("help"));
    let rest = args.get(1..).unwrap_or_default();
    match verb.to_str() {
        Some("help" | "--help" | "-h" | "schema" | "--schema" | "version" | "--version" | "-v") => {
            return Decision::Allowed
        }
        Some("explain") => {
            if rest.iter().any(|a| kit_id(a)) {
                return Decision::Refuse { verb, anc };
            }
            return Decision::Allowed;
        }
        _ => {}
    }
    if rest.iter().any(|a| a == "-h" || a == "--help") {
        return Decision::HelpOf(verb);
    }
    Decision::Refuse { verb, anc }
}

/// `_cli_codex_tab_refuse`'s lines, each with its newline.
pub fn refusal(verb: &OsStr, anc: Option<Anc>) -> Vec<u8> {
    let mut text = concat!(
        "Error: this Codex tab was started before this update or outside ",
        product_name!(),
        ": reopen it from ",
        product_name!(),
        ".\n",
        product_name!(),
        " can't tell this call from one under a Codex server another tab started, so nothing was sent.\n",
    )
    .as_bytes()
    .to_vec();
    match anc {
        Some(Anc::Shared) => text.extend_from_slice(
            b"It runs under a shared Codex app-server; stop that server once: codex app-server daemon stop\n",
        ),
        Some(Anc::Broken) => text.extend_from_slice(
            concat!(
                product_name!(),
                " couldn't read the processes above this call, so it can't rule out a shared Codex server.\n"
            )
            .as_bytes(),
        ),
        _ => {}
    }
    text.extend_from_slice(
        concat!(
            "If a tab ",
            product_name!(),
            " opened still says this, set K2_CODEX_NO_DAEMON=1 for the ",
            product_name!(),
            " daemon, then reopen the tab.\nHelp still works here: ",
            cli_name!(),
            " help "
        )
        .as_bytes(),
    );
    text.extend_from_slice(verb.as_encoded_bytes());
    text.push(b'\n');
    text
}

/// What the front does after the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Route the line as before.
    Go,
    /// The gate answered the line: exit with this.
    Stop(Exit),
    /// The shell answers the line (its earlier steps and its gate).
    Shell,
}

/// The gate for `args` (the words after the program's name), in this
/// process's environment. After the port check; a stray `--` is the
/// front's refusal, before this gate, as in the shell.
pub fn check(args: &[OsString]) -> Verdict {
    if dashdash::stray(args) != Stray::No {
        return Verdict::Go;
    }
    let env = Env::current();
    let lookup = |name: &str| env.get(name).map(OsStr::to_os_string);
    if !bind_on(&lookup) || !applies(&lookup) {
        return Verdict::Go;
    }
    if !earlier_steps_pass(args, &env) {
        return Verdict::Shell;
    }
    match decide(args, lookup, || ancestry(&env)) {
        Decision::Off | Decision::Confirmed => Verdict::Go,
        // The note, then the command: the shell prints it once, where the
        // binary might hand the command on and the shell print it again.
        Decision::Blind | Decision::Unknown => Verdict::Shell,
        Decision::Allowed => {
            HELD.store(true, Ordering::Relaxed);
            if front::route(args) == Route::Version {
                return Verdict::Go;
            }
            page_for(args).map_or(Verdict::Shell, |p| Verdict::Stop(emit(&p)))
        }
        Decision::HelpOf(verb) => {
            HELD.store(true, Ordering::Relaxed);
            let line = [OsString::from("help"), verb];
            page_for(&line).map_or(Verdict::Shell, |p| Verdict::Stop(emit(&p)))
        }
        Decision::Refuse { verb, anc } => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), &refusal(&verb, anc));
            Verdict::Stop(Exit::Refused)
        }
    }
}

/// Whether the front knows the shell's steps before its gate let `args`
/// by in `env`: the prelude as the front reads it, a port and a token (the
/// connection gate passes, and with a token the virtual desktop's guard
/// does), no verb an air-gap refusal may stop (`connect`, `publish`,
/// `daemon`, `tunnel`) and no daemon host setting (the plain-http step).
fn earlier_steps_pass(args: &[OsString], env: &Env) -> bool {
    if args
        .first()
        .and_then(|a| a.to_str())
        .is_some_and(|w| matches!(w, "connect" | "publish" | "daemon" | "tunnel"))
    {
        return false;
    }
    if env.first(env_names::HOST).is_some() {
        return false;
    }
    let prelude = offline::shell_pwd(env).and_then(|cwd| offline::prelude(env, &cwd));
    if !prelude.is_ok_and(|p| p.port && p.token) {
        return false;
    }
    crate::daemon::current().is_ok_and(|s| crate::prelude::gate(&s).is_none())
}

/// The page `line` is when the front prints it ([`pages`]): a help form
/// whose family doesn't answer it, the daily tier, or a composed page.
/// `None`: the shell answers.
fn page_for(line: &[OsString]) -> Option<Page> {
    let env = Env::current();
    let cwd = offline::shell_pwd(&env).ok()?;
    let p = offline::prelude(&env, &cwd).ok()?;
    let would_refuse = || passport::current(line).would_refuse();
    match front::route(line) {
        Route::Help(form) if form.variant != Some(Variant::Family) => {
            pages::help_page(form, &p, &env, would_refuse)
        }
        Route::Daily => pages::daily_page(&p, &env, would_refuse),
        Route::Composed(c) => {
            let argv: Vec<&str> = line.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
            pages::composed_page(c, &argv, &p, &env, would_refuse)
        }
        _ => None,
    }
}

/// Prints `page` as the front prints its pages; the exit.
fn emit(page: &Page) -> Exit {
    if let Some(lead) = &page.lead {
        if let Err(err) = out::write_flushed(&mut std::io::stderr().lock(), lead.as_bytes()) {
            return out::after_stdout_error(&err);
        }
    }
    if page.stderr {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), page.text.as_bytes());
        return page.exit;
    }
    match out::stdout(&page.text) {
        Ok(()) => page.exit,
        Err(err) => out::after_stdout_error(&err),
    }
}

/// `_cli_ps_chain START`: `rows` (`ps -axo pid=,ppid=,command=`) from
/// `start` up to pid 1, one row each, or `None` (its exit 3) when a row
/// can't be read or the chain is missing a process.
pub fn ps_chain(rows: &str, start: &str) -> Option<String> {
    let mut line = std::collections::HashMap::new();
    let mut parent = std::collections::HashMap::new();
    let mut n_rows = 0;
    for row in rows.split('\n') {
        n_rows += 1;
        if !row_readable(row) {
            return None;
        }
        let mut fields = row.split([' ', '\t']).filter(|f| !f.is_empty());
        let pid = fields.next()?.to_string();
        let ppid = fields.next()?.to_string();
        line.insert(pid.clone(), row.to_string());
        parent.insert(pid, ppid);
    }
    if n_rows == 0 {
        return None;
    }
    let mut out = String::new();
    let mut p = start.to_string();
    let mut n = 0;
    while awk_num(&p) > 1 {
        let row = line.get(&p)?;
        if n >= 64 {
            return None;
        }
        out.push_str(row);
        out.push('\n');
        p = parent.get(&p).cloned().unwrap_or_default();
        n += 1;
    }
    Some(out)
}

/// awk's `/^[ \t]*[0-9]+[ \t]+[0-9]+([ \t]|$)/`.
fn row_readable(row: &str) -> bool {
    let b = row.as_bytes();
    let mut i = 0;
    let blank = |c: u8| c == b' ' || c == b'\t';
    while i < b.len() && blank(b[i]) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s
    };
    if !digits(&mut i) {
        return false;
    }
    let s = i;
    while i < b.len() && blank(b[i]) {
        i += 1;
    }
    if i == s || !digits(&mut i) {
        return false;
    }
    i == b.len() || blank(b[i])
}

/// awk's `p + 0`: the leading number of `p`, 0 for none.
fn awk_num(p: &str) -> u64 {
    let t = p.trim_start_matches([' ', '\t']);
    let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    t[..end].parse().unwrap_or(0)
}

/// `_cli_ancestry_has_shared_codex_server START` over `rows`: a shared
/// Codex app-server (a unix listener, a managed daemon, or one detached
/// to pid 1) is above `start`.
pub fn has_shared_server(rows: &str, start: &str) -> bool {
    let mut cmd = std::collections::HashMap::new();
    let mut parent = std::collections::HashMap::new();
    for row in rows.split('\n') {
        let mut fields = row.split([' ', '\t', '\n']).filter(|f| !f.is_empty());
        let pid = fields.next().unwrap_or("").to_string();
        let ppid = fields.next().unwrap_or("").to_string();
        let rest: Vec<&str> = fields.collect();
        parent.insert(pid.clone(), ppid);
        cmd.insert(pid, rest.join(" "));
    }
    let mut p = start.to_string();
    let mut n = 0;
    while !p.is_empty() && awk_num(&p) > 1 && n < 64 {
        let c = cmd.get(&p).map_or("", String::as_str);
        let par = parent.get(&p).map_or("", String::as_str);
        let listens = c.contains("--listen unix") || c.contains("--listen=unix");
        if c.contains("codex")
            && c.contains("app-server")
            && (listens
                || c.contains("--managed-daemon")
                || c.contains("app-server daemon")
                || (!par.is_empty() && awk_num(par) == 1 && par.trim().bytes().all(|b| b.is_ascii_digit())))
        {
            return true;
        }
        p = par.to_string();
        n += 1;
    }
    false
}

/// `$(...)`'s text: trailing newlines dropped.
fn substituted(bytes: Vec<u8>) -> Option<String> {
    let mut s = String::from_utf8(bytes).ok()?;
    while s.ends_with('\n') {
        s.pop();
    }
    Some(s)
}

/// `_cli_codex_ancestry` for this process: `ps` from `PATH`, then the
/// parent walk in `python3` ([`ANCESTRY_PY`]). `None` where the binary
/// can't do it as the shell does: no `python3` it can be sure of when
/// the walk needs one, an answer that isn't UTF-8, or a program that
/// can't be started for another reason.
pub fn ancestry(env: &Env) -> Option<Anc> {
    let start = std::process::id().to_string();
    let path = env.get("PATH");
    let r_ps = match ps_rows(path) {
        PsRun::Rows(rows) => match ps_chain(&rows, &start) {
            Some(chain) => return Some(verdict_of(&chain, &start)),
            None => 3,
        },
        PsRun::Refused => 2,
        PsRun::Failed => 3,
        PsRun::Unknown => return None,
    };
    if !matches!(python::in_env(env), python::Python3::Present) {
        return None;
    }
    let mut cmd = std::process::Command::new("python3");
    cmd.args(["-c", ANCESTRY_PY, "k2-codex-ancestry", &start])
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::null());
    if let Some(p) = path {
        cmd.env("PATH", p);
    }
    let out = cmd.output().ok()?;
    let r_py = match out.status.code() {
        Some(0) => {
            let rows = substituted(out.stdout)?;
            return Some(verdict_of(&rows, &start));
        }
        Some(2 | 126) => 2,
        _ => 3,
    };
    Some(if r_ps == 2 && r_py == 2 {
        Anc::Blind
    } else {
        Anc::Broken
    })
}

fn verdict_of(rows: &str, start: &str) -> Anc {
    if has_shared_server(rows, start) {
        Anc::Shared
    } else {
        Anc::Clear
    }
}

enum PsRun {
    /// Exit 0: its output (stdout and stderr together).
    Rows(String),
    /// The shell's refusal: exit 126, or "operation not permitted" or
    /// "permission denied" in what it wrote.
    Refused,
    /// Any other failure (not found: the shell's 127).
    Failed,
    /// The binary can't say what the shell would see.
    Unknown,
}

/// `out="$(exec ps -axo pid=,ppid=,command= 2>&1)"`.
fn ps_rows(path: Option<&OsStr>) -> PsRun {
    let Ok((mut reader, writer)) = std::io::pipe() else {
        return PsRun::Unknown;
    };
    let Ok(writer2) = writer.try_clone() else {
        return PsRun::Unknown;
    };
    let mut cmd = std::process::Command::new("ps");
    cmd.args(["-axo", "pid=,ppid=,command="])
        .stdin(std::process::Stdio::inherit())
        .stdout(writer)
        .stderr(writer2);
    if let Some(p) = path {
        cmd.env("PATH", p);
    }
    let child = cmd.spawn();
    drop(cmd);
    let mut child = match child {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PsRun::Failed,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return PsRun::Refused,
        Err(_) => return PsRun::Unknown,
    };
    let mut bytes = Vec::new();
    let read = reader.read_to_end(&mut bytes);
    let Ok(status) = child.wait() else {
        return PsRun::Unknown;
    };
    if read.is_err() {
        return PsRun::Unknown;
    }
    let Some(text) = substituted(bytes) else {
        return PsRun::Unknown;
    };
    let rc = status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            128 + status.signal().unwrap_or(0)
        }
        #[cfg(not(unix))]
        {
            1
        }
    });
    if rc == 0 {
        return PsRun::Rows(text);
    }
    let low = text.to_lowercase();
    if rc == 126 || low.contains("operation not permitted") || low.contains("permission denied") {
        PsRun::Refused
    } else {
        PsRun::Failed
    }
}

/// The shell's `_K2_ANCESTRY_PY`, byte for byte (a test holds it to
/// `cli/src/cli-tools-policy.sh`).
pub const ANCESTRY_PY: &str = r#"
import ctypes, errno, struct, sys
DENIED, BROKEN = 2, 3
start = int(sys.argv[2])
if sys.platform.startswith("linux"):
    def info(pid):
        with open("/proc/%d/stat" % pid, "rb") as f:
            stat = f.read()
        ppid = int(stat[stat.rindex(b")") + 2:].split()[1])
        with open("/proc/%d/cmdline" % pid, "rb") as f:
            cmd = f.read()
        return ppid, cmd.rstrip(b"\0").replace(b"\0", b" ").decode("utf-8", "replace")
elif sys.platform == "darwin":
    lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    lib.sysctl.argtypes = [ctypes.POINTER(ctypes.c_int), ctypes.c_uint, ctypes.c_void_p,
                           ctypes.POINTER(ctypes.c_size_t), ctypes.c_void_p, ctypes.c_size_t]
    def fail(what):
        raise OSError(ctypes.get_errno() or errno.EIO, what)
    argmax = ctypes.c_int(0)
    size = ctypes.c_size_t(ctypes.sizeof(argmax))
    if lib.sysctl((ctypes.c_int * 2)(1, 8), 2, ctypes.byref(argmax), ctypes.byref(size), None, 0) != 0:
        fail("kern.argmax")
    def info(pid):
        # struct proc_bsdinfo (PROC_PIDTBSDINFO, 136 bytes): pbi_ppid at 16.
        bsd = ctypes.create_string_buffer(136)
        if lib.proc_pidinfo(pid, 3, 0, bsd, 136) != 136:
            fail("proc_pidinfo")
        ppid = struct.unpack_from("I", bsd.raw, 16)[0]
        comm = bsd.raw[48:64].split(b"\0")[0].decode("utf-8", "replace")
        # kern.procargs2: argc, the exec path, NUL padding, then argv.
        buf = ctypes.create_string_buffer(argmax.value)
        size = ctypes.c_size_t(argmax.value)
        if lib.sysctl((ctypes.c_int * 3)(1, 49, pid), 3, buf, ctypes.byref(size), None, 0) != 0:
            # A process of another user: its name is enough to rule it out
            # unless it could be a Codex server (codex, or a script runner).
            low = comm.lower()
            if not comm or "codex" in low or low in ("node", "bun", "deno"):
                fail("kern.procargs2")
            return ppid, comm
        data = buf.raw[:size.value]
        argc = struct.unpack_from("i", data, 0)[0]
        rest = data[4:]
        i = rest.index(b"\0")
        while i < len(rest) and rest[i:i + 1] == b"\0":
            i += 1
        args = rest[i:].split(b"\0")[:argc]
        if len(args) != argc:
            raise ValueError("short argv")
        return ppid, " ".join(a.decode("utf-8", "replace") for a in args)
else:
    # No walk is handled on this platform: counts as refused.
    sys.exit(DENIED)
rows = []
pid = start
try:
    while pid > 1:
        if len(rows) >= 64 or any(r[0] == pid for r in rows):
            sys.exit(BROKEN)
        ppid, cmd = info(pid)
        rows.append((pid, ppid, cmd.replace("\n", " ")))
        pid = ppid
except OSError as e:
    sys.exit(DENIED if e.errno in (errno.EPERM, errno.EACCES) else BROKEN)
except Exception:
    sys.exit(BROKEN)
for r in rows:
    print("%d %d %s" % r)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |n: &str| pairs.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    const SOCK: (&str, &str) = ("K2_HOOK_SOCK", "/s");
    const TID: (&str, &str) = ("CODEX_THREAD_ID", "t1");
    const PASS: (&str, &str) = ("K2_HOOK_TOKEN", "sessA.x");

    fn never() -> Option<Anc> {
        panic!("the walk ran")
    }

    #[test]
    fn off_outside_a_codex_call_or_a_session() {
        assert_eq!(decide(&args(&["whoami"]), env(&[SOCK, PASS]), never), Decision::Off);
        assert_eq!(decide(&args(&["whoami"]), env(&[TID, PASS]), never), Decision::Off);
        assert_eq!(
            decide(&args(&["whoami"]), env(&[TID, ("K2_PANE_ID", "p")]), never),
            Decision::Off
        );
        for off in ["0", "off", "false", "no"] {
            let e = env(&[TID, SOCK, (BIND_SWITCH, off)]);
            assert_eq!(decide(&args(&["whoami"]), e, never), Decision::Off, "{off}");
        }
        let e = env(&[TID, SOCK, (BIND_SWITCH, "OFF")]);
        assert!(matches!(decide(&args(&["whoami"]), e, never), Decision::Refuse { .. }));
    }

    #[test]
    fn the_marker_must_name_the_passport_s_session() {
        let walk = || Some(Anc::Clear);
        let e = env(&[TID, SOCK, PASS, (TAB_MARKER, "sessA")]);
        assert_eq!(decide(&args(&["whoami"]), e, walk), Decision::Confirmed);
        for m in ["sessB", "sess", "sessA.x", ""] {
            let e = env(&[TID, SOCK, PASS, (TAB_MARKER, m)]);
            assert!(
                matches!(decide(&args(&["whoami"]), e, never), Decision::Refuse { anc: None, .. }),
                "{m}"
            );
        }
        let e = env(&[TID, SOCK, ("K2_HOOK_TOKEN", "sessA"), (TAB_MARKER, "sessA")]);
        assert!(matches!(decide(&args(&["whoami"]), e, never), Decision::Refuse { .. }));
        let e = env(&[TID, ("K2_PANE_ID", "p"), PASS, (TAB_MARKER, "sessA")]);
        assert_eq!(decide(&args(&["inbox"]), e, walk), Decision::Confirmed);
    }

    #[test]
    fn the_walk_decides_a_matching_marker() {
        let e = || env(&[TID, SOCK, PASS, (TAB_MARKER, "sessA")]);
        assert_eq!(decide(&args(&["x"]), e(), || Some(Anc::Blind)), Decision::Blind);
        assert_eq!(decide(&args(&["x"]), e(), || None), Decision::Unknown);
        for a in [Anc::Shared, Anc::Broken] {
            assert_eq!(
                decide(&args(&["x"]), e(), || Some(a)),
                Decision::Refuse {
                    verb: "x".into(),
                    anc: Some(a)
                }
            );
        }
    }

    #[test]
    fn the_allowed_words_and_a_help_flag() {
        let e = || env(&[TID, SOCK, PASS]);
        for w in ["help", "--help", "-h", "schema", "--schema", "version", "--version", "-v", "explain", ""] {
            assert_eq!(decide(&args(&[w, "x"]), e(), never), Decision::Allowed, "{w}");
        }
        assert_eq!(decide(&args(&[]), e(), never), Decision::Allowed);
        for kit in ["MPC-1", "mpc-x", "MpCe-2", "mpce-"] {
            assert_eq!(
                decide(&args(&["explain", "a", kit]), e(), never),
                Decision::Refuse {
                    verb: "explain".into(),
                    anc: None
                },
                "{kit}"
            );
        }
        for not_kit in ["MPCX-1", "MPC", "mpce", "xMPC-1"] {
            assert_eq!(decide(&args(&["explain", not_kit]), e(), never), Decision::Allowed);
        }
        assert_eq!(
            decide(&args(&["msg", "--", "--help"]), e(), never),
            Decision::HelpOf("msg".into())
        );
        assert_eq!(decide(&args(&["whoami", "-h"]), e(), never), Decision::HelpOf("whoami".into()));
        assert_eq!(
            decide(&args(&["-x", "--help"]), e(), never),
            Decision::HelpOf("-x".into())
        );
        assert_eq!(
            decide(&args(&["study"]), e(), never),
            Decision::Refuse {
                verb: "study".into(),
                anc: None
            }
        );
    }

    #[test]
    fn the_refusal_s_lines() {
        let base = format!(
            "Error: this Codex tab was started before this update or outside {p}: reopen it from {p}.\n\
             {p} can't tell this call from one under a Codex server another tab started, so nothing was sent.\n",
            p = product_name!()
        );
        let end = format!(
            "If a tab {p} opened still says this, set K2_CODEX_NO_DAEMON=1 for the {p} daemon, then reopen the tab.\n\
             Help still works here: {c} help whoami\n",
            p = product_name!(),
            c = cli_name!()
        );
        assert_eq!(refusal(OsStr::new("whoami"), None), format!("{base}{end}").into_bytes());
        assert_eq!(
            refusal(OsStr::new("whoami"), Some(Anc::Shared)),
            format!(
                "{base}It runs under a shared Codex app-server; stop that server once: codex app-server daemon stop\n{end}"
            )
            .into_bytes()
        );
        assert_eq!(
            refusal(OsStr::new("whoami"), Some(Anc::Broken)),
            format!(
                "{base}{} couldn't read the processes above this call, so it can't rule out a shared Codex server.\n{end}",
                product_name!()
            )
            .into_bytes()
        );
    }

    #[test]
    fn the_chain_and_the_shared_server() {
        let rows = "    1     0 /sbin/launchd\n  200     1 codex app-server daemon pid-update-loop\n  300   200 codex app-server --listen unix://\n  400   300 /bin/zsh -lc k2\n  500   400 /bin/bash k2";
        let chain = ps_chain(rows, "500").expect("a chain");
        assert_eq!(chain.lines().count(), 4);
        assert!(has_shared_server(&chain, "500"));
        let plain = "    1     0 init\n  400     1 /bin/zsh\n  500   400 /bin/bash k2";
        assert!(!has_shared_server(&ps_chain(plain, "500").expect("a chain"), "500"));
        assert_eq!(ps_chain("garbage\n  500 400 x", "500"), None);
        assert_eq!(ps_chain("  500   400 x", "500"), None);
        assert_eq!(ps_chain("", "500"), None);
        let detached = "    1     0 init\n  300     1 codex app-server\n  500   300 bash";
        assert!(has_shared_server(detached, "500"));
        let private = "    1     0 init\n  290     1 codex\n  300   290 codex app-server\n  500   300 bash";
        assert!(!has_shared_server(private, "500"));
    }
}
