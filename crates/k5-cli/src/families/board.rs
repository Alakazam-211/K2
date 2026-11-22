//! `k5 board …`: the Board Garden's boards and `.k5board` files, as the
//! shell CLI's `cmd_board` answers them (`cli/src/board.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]). A command line with `-h` or
//!    `--help` in it is one the shell counts as offline, so the virtual
//!    desktop's guard doesn't stop it; the connection gate still does.
//! 2. `board` is the `store` tool in the shell's policy, an `id` tool: with
//!    `HOOK_SOCK` set, the stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]).
//! 3. `board` alone, an empty first word, `help` first, or `-h` or
//!    `--help` anywhere: the help page (`cli/text/help/board.txt`), and
//!    nothing is sent (help never acts).
//! 4. The path last ([`target`]): with no `--file` word, the last word
//!    ending `.k5board` (or the last word when it names a file that exists)
//!    moves to `--file <it>` at the end. `--file` and `--board` together are
//!    `Error [two_targets]: …`, exit 2.
//! 5. A `--file` line: [`file`]. Else a Garden board: [`garden`].
//!
//! Both send with `_compute_send` ([`send`]): the cell socket first with a
//! passport and a live socket, then TCP; a POST is sent again over TCP only
//! after a refused connect or "not served". No permission note, no
//! bulletin: `_compute_send` notes nothing.
//!
//! The route table's `board` row is the family's; a word that is not
//! UTF-8, `HOOK_SOCK` with a `CODEX_` variable, a port curl won't take and
//! a `python3` the binary can't tell still go to the shell (the edge
//! hand-backs every family keeps). An answer that is not UTF-8 is
//! read with U+FFFD for each bad byte, as [`super::inbox`]; so is what
//! `apply` reads on stdin.

pub mod file;
pub mod garden;

use super::{curl_port, without_nuls, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use crate::transport::{Client, Error};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["board"];
/// The binary answers it; [`run`] hands the edge cases back.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "store";

/// What `raw="$(_compute_send …)"` holds, split as the shell splits it:
/// the status after the last newline, the answer before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub status: String,
    pub resp: Vec<u8>,
}

impl Sent {
    /// `status="${raw##*$'\n'}"; resp="${raw%$'\n'*}"`, with `$(…)`'s NULs
    /// and trailing newlines gone.
    pub fn split(raw: &[u8]) -> Sent {
        let mut raw = without_nuls(raw);
        while raw.last() == Some(&b'\n') {
            raw.pop();
        }
        match raw.iter().rposition(|b| *b == b'\n') {
            Some(at) => Sent {
                status: String::from_utf8_lossy(&raw[at + 1..]).into_owned(),
                resp: raw[..at].to_vec(),
            },
            None => Sent {
                status: if raw.is_empty() {
                    "000".into()
                } else {
                    String::from_utf8_lossy(&raw).into_owned()
                },
                resp: Vec::new(),
            },
        }
    }
}

/// `-w '\n%{http_code}'` of a try that got no answer.
const NO_STATUS: &[u8] = b"\n000";

/// `_compute_send <method> <path> <payload>` inside `$(…)`: what it
/// printed, split; or the exit, after its line, when it ended the command
/// (a POST on the cell socket with no answer, or no passport in a
/// session).
pub fn send(client: &mut Client, post: bool, path: &str, payload: &str) -> Result<Sent, Exit> {
    send_on(client, post, path, payload, false)
}

/// [`send`], or `_compute_send <method> <path> <payload> tcp` when `tcp`
/// is set: TCP whatever the session has.
pub fn send_on(
    client: &mut Client,
    post: bool,
    path: &str,
    payload: &str,
    tcp: bool,
) -> Result<Sent, Exit> {
    let result = client.compute_send(post, path, payload, tcp);
    // Each refused try curl made before the last wrote its own status.
    let mut raw = NO_STATUS.repeat(client.refused_before_answer() as usize);
    match result {
        Ok(r) => {
            raw.extend_from_slice(&r.body);
            raw.extend_from_slice(format!("\n{:03}", r.status).as_bytes());
        }
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            return Err(Exit::Error);
        }
        // The last try's status, then `|| printf '\n000'`.
        Err(_) => {
            raw.extend_from_slice(NO_STATUS);
            raw.extend_from_slice(NO_STATUS);
        }
    }
    Ok(Sent::split(&raw))
}

/// `n` random bytes as hex: the fence's nonce (`secrets.token_hex(8)`)
/// and a group's id (`uuid.uuid4().hex`).
pub fn random_hex(n: usize) -> String {
    let bytes = random_bytes(n);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    match std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)) {
        Ok(()) => buf,
        Err(_) => seeded_bytes(n),
    }
}

#[cfg(not(unix))]
fn random_bytes(n: usize) -> Vec<u8> {
    seeded_bytes(n)
}

/// Bytes from the standard library's hasher keys, which the OS seeds.
fn seeded_bytes(n: usize) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(n);
    let mut i = 0u64;
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        out.extend_from_slice(&h.finish().to_le_bytes());
        i += 1;
    }
    out.truncate(n);
    out
}

/// `uuid.uuid4().hex`: random, with the version and variant bits set.
pub fn uuid4_hex() -> String {
    let mut b = random_bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Where a command line goes once the path is placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `--file` and `--board` both.
    TwoTargets,
    /// A `.k5board` file: the words, `--file <path>` among them.
    File(Vec<String>),
    /// A Garden board: the words; `named`, `--board` was given.
    Garden { words: Vec<String>, named: bool },
}

/// The line for [`Target::TwoTargets`].
pub const TWO_TARGETS: &str = "Error [two_targets]: name one board: --board <id|name> for a Garden board, or --file <path> for a .k5board file";

/// `cmd_board`'s path-last step and its `--file` and `--board` check.
/// `is_file`: `[ -f "$word" ]` from the shell's folder.
pub fn target(words: &[&str], is_file: impl Fn(&str) -> bool) -> Target {
    let mut words: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
    // `printf '%s\n' "$@" | grep -qE -- '^--file(=|$)'`: line by line.
    let file_line = words
        .iter()
        .flat_map(|w| w.split('\n'))
        .any(|l| l == "--file" || l.starts_with("--file="));
    if !file_line {
        let mut at = 0;
        for (i, w) in words.iter().enumerate() {
            // `$(printf '%s' "$_a" | tr 'A-Z' 'a-z')`: trailing newlines go.
            let lower = w.trim_end_matches('\n').to_ascii_lowercase();
            if lower.starts_with("--") {
                continue;
            }
            if lower.ends_with(".k5board") {
                at = i + 1;
            }
        }
        let n = words.len();
        if at == 0 && n >= 2 && is_file(&words[n - 1]) {
            at = n;
        }
        if at >= 2 {
            let p = words.remove(at - 1);
            words.push("--file".into());
            words.push(p);
        }
    }
    let has = |name: &str| {
        words
            .iter()
            .any(|w| w == name || w.starts_with(&format!("{name}=")))
    };
    let (file, board) = (has("--file"), has("--board"));
    if file && board {
        Target::TwoTargets
    } else if file {
        Target::File(words)
    } else {
        Target::Garden {
            words,
            named: board,
        }
    }
}

/// `k5 board …`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, &words, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    let offline = words.iter().any(|w| matches!(*w, "-h" | "--help"));
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if offline => return Gate::CannotConnect.stop(),
        Some(gate) => return gate.stop(),
        None => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    // `board` alone, `help` first, or `-h`/`--help` anywhere (`_wants_help`):
    // the help page, nothing sent.
    if matches!(words.first(), None | Some(&("" | "help"))) || offline {
        let page = crate::text::help("board").expect("the board help page is embedded");
        return match out::stdout(page) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let target = target(words, |w| !w.is_empty() && cwd.join(w).is_file());
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = |settings: &Settings| match settings.client(would_refuse) {
        Ok(c) => Ok(c),
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            Err(Exit::Error)
        }
    };
    let stdin = || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut buf);
        buf
    };
    match target {
        Target::TwoTargets => {
            out::stderr_line(TWO_TARGETS);
            Exit::Usage
        }
        Target::File(words) => {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            let as_json = words.contains(&"--json");
            let pwd = daemon::logical_cwd(std::env::var_os("PWD")).unwrap_or_else(|_| cwd.clone());
            let plan = match file::plan(&pwd.to_string_lossy(), &words, stdin, uuid4_hex) {
                Ok(p) => p,
                Err(stop) => return stop.print(),
            };
            let mut c = match client(&settings) {
                Ok(c) => c,
                Err(exit) => return exit,
            };
            match send(&mut c, plan.post, plan.route, &plan.payload) {
                Ok(sent) => file::answer(&plan, &sent, as_json, &random_hex(8)).print(),
                Err(exit) => exit,
            }
        }
        Target::Garden { words, named } => {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            let as_json = words.contains(&"--json");
            let ask = match garden::parse(&words, stdin) {
                Ok(a) => a,
                Err(stop) => return stop.print(),
            };
            let mut c = match client(&settings) {
                Ok(c) => c,
                Err(exit) => return exit,
            };
            match send(&mut c, true, garden::ENDPOINT, &garden::payload(&ask)) {
                Ok(sent) => {
                    let shown: Shown = garden::answer(&sent, named, as_json, &ask.out, &cwd);
                    shown.print()
                }
                Err(exit) => exit,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(words: &[&str]) -> Target {
        target(words, |w| w == "existing")
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn the_path_goes_last() {
        assert_eq!(
            placed(&["read", "s-1", "plan.K5Board", "--json"]),
            Target::File(strings(&[
                "read",
                "s-1",
                "--json",
                "--file",
                "plan.K5Board"
            ]))
        );
        // The first word is never moved: it is the verb.
        assert_eq!(
            placed(&["plan.k5board", "list"]),
            Target::Garden {
                words: strings(&["plan.k5board", "list"]),
                named: false
            }
        );
        assert_eq!(
            placed(&["list", "existing"]),
            Target::File(strings(&["list", "--file", "existing"]))
        );
        assert_eq!(
            placed(&["list", "--x=a.k5board", "--board", "b"]),
            Target::Garden {
                words: strings(&["list", "--x=a.k5board", "--board", "b"]),
                named: true
            }
        );
        assert_eq!(
            placed(&["list", "--file=x", "--board=b"]),
            Target::TwoTargets
        );
        // A `--file` line anywhere stops the move.
        assert_eq!(
            placed(&["list", "a\n--file", "p.k5board"]),
            Target::Garden {
                words: strings(&["list", "a\n--file", "p.k5board"]),
                named: false
            }
        );
        assert_eq!(
            placed(&["list", "p.k5board\n"]),
            Target::File(strings(&["list", "--file", "p.k5board\n"]))
        );
    }

    #[test]
    fn what_compute_send_printed() {
        assert_eq!(
            Sent::split(b"{\"a\":1}\n\n200"),
            Sent {
                status: "200".into(),
                resp: b"{\"a\":1}\n".to_vec()
            }
        );
        assert_eq!(
            Sent::split(b"\n000\n000\n000"),
            Sent {
                status: "000".into(),
                resp: b"\n000\n000".to_vec()
            }
        );
        assert_eq!(
            Sent::split(b"x\0y\n404"),
            Sent {
                status: "404".into(),
                resp: b"xy".to_vec()
            }
        );
    }

    #[test]
    fn random_hex_is_hex_and_fresh() {
        let a = random_hex(8);
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, random_hex(8));
        let u = uuid4_hex();
        assert_eq!((u.len(), &u[12..13]), (32, "4"));
    }
}
