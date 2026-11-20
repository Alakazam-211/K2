//! `k5 msg <workspace> "text" [flags]`: a live message, as the shell's
//! `cmd_msg_v2` (`cli/src/msg-inbox-staging.sh`) and `cmd_msg`
//! (`cli/src/msg-whoami-done.sh`) send it.
//!
//! What runs after the gates ([`super`]), in the shell's order:
//!
//! 1. `cmd_msg_v2`'s checks on all the words: a bare `--inbox` with neither
//!    `--inbox-silent` nor `--inbox-wake` is [`BARE_INBOX`], exit 2; both
//!    of those is [`BOTH_INBOX`], exit 2.
//! 2. `cmd_msg`'s loop ([`parse`]): the flags, an unknown one, the
//!    retired `--agent`, then the address and the text (the first word that
//!    is not a flag is the address, the last the text).
//! 3. `--session <id>`: the one word is the text and the address is the
//!    workspace (`PROJECT`, else the folder); two words is exit 2.
//! 4. No address, or no text: two or three lines, exit 1.
//! 5. A text over 600 characters with no `--summary`: [`summary_hint`] on
//!    stderr, and on.
//! 6. `--secret` with `--session`: exit 2. `--secret NAME` to an address
//!    is one vault handoff whose note is the text ([`secret`]); no message
//!    goes, so the text never arrives without its secret.
//! 7. The send key ([`super::send_once::init`]): one probe, then the key.
//! 8. An address in federation's wire form (`fp::ws::agent`): three lines,
//!    exit 2.
//! 9. The sender, when `--from` named none: a sidecar cell's
//!    `<primary>/<sidecar>`, else the folder's handle
//!    (`/cli/workspace/handle`, then `/cli/workspace/agent-display-name`),
//!    else its folder name (`/cli/workspace/resolve`), else `external`.
//!    Each is a `cli_request` over TCP whose refusal is no refusal of the
//!    command (`_K2_PERMISSION_QUIET`) and whose errors are hidden; a
//!    request that failed ends the command with curl's code, and one with
//!    no answer ends it by SIGPIPE, as the shell's pipe does
//!    ([`super::super::end_in_pipe`]).
//! 10. The form to `/cli/workspace/msg`, sent once by its key
//!     ([`super::send_once::send`]); a send that failed ends the command
//!     with curl's code, nothing printed.
//! 11. The answer on stdout as `echo` prints it, then on stderr: the
//!     rename note (`movedFrom`), the replay note, the woke and reopened
//!     notes. Exit 3 when the answer's `error.code` is `not_connected`,
//!     `gated` or `forbidden`, 0 when `success` is `true`, else 2.
//! 12. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! An address `handle::label.postal.bot` first is Postal mail
//! ([`super::postal`]). The shell answers these forms itself ([`parse`]
//! says [`Parsed::Shell`]): `--signal`, a tray form's `~name` path
//! ([`super::tray`]), and a `--secret` the vault's handoff leaves to the
//! shell (its name `--stdin`: a value a person types in), or with a
//! `python3` the binary can't tell. An address on another server
//! (`agent::host`, `agent@host`) goes over federation ([`super::remote`]).
//!
//! A value flag (`--from`, `--command`, `--wake-timeout`, `--via`) as the
//! last word is [`needs_value`]'s line and exit 2, where bash stops with
//! its own `$2: unbound variable` (K2's D6).

use super::super::{end_in_pipe, without_nuls};
use super::py::s;
use super::send_once::{self, Key};
use crate::exit::Exit;
use crate::families::vault;
use crate::out::json::{self, Value};
use crate::out::py::{str_of, truthy};
use crate::out::{self};
use crate::prelude::Closing;
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error, Response};
use k2_client::env_names;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/workspace/msg";
/// The sender's handle.
pub const HANDLE: &str = "/cli/workspace/handle";
/// The sender's handle, the older route.
pub const DISPLAY_NAME: &str = "/cli/workspace/agent-display-name";
/// The sender's folder.
pub const RESOLVE: &str = "/cli/workspace/resolve";

/// `cmd_msg_inbox_bare_error`.
pub const BARE_INBOX: &str = concat!(
    "Error: bare --inbox is no longer accepted.\n",
    "\n",
    "Tray delivery requires an explicit mode:\n",
    "\n",
    "  ",
    cli_name!(),
    " msg <workspace> --inbox-wake   <path> [path…]   # package + short live notify (recommended)\n",
    "  ",
    cli_name!(),
    " msg <workspace> --inbox-silent <path> [path…]   # package only (does NOT notify)\n",
    "\n",
    "Multiple paths land as ONE tray package (cover note + sidecars under\n",
    ".k2/inbox/<id>.files/). Single path keeps the classic single-file shape.\n",
    "\n",
    "--inbox-silent lands a durable package under the peer's .k2/inbox/ but\n",
    "does not wake or ping them. Use --inbox-wake for package + knock, or send\n",
    "a separate live `",
    cli_name!(),
    " msg <workspace> \"...\"` after silent if you want your\n",
    "own notify.\n",
    "\n",
    "Files plus a brief (cover note):\n",
    "  ",
    cli_name!(),
    " msg <workspace> --inbox-wake ./file.pdf --body \"please review\"\n",
    "  ",
    cli_name!(),
    " msg <workspace> --inbox-wake ./file.pdf \"please review\"\n",
    "\n",
    "Legacy string form (no file):\n",
    "  ",
    cli_name!(),
    " msg <workspace> --inbox-silent --title \"...\" [--body \"...\"]\n",
    "  ",
    cli_name!(),
    " msg <workspace> --inbox-wake   --title \"...\" [--body \"...\"]\n"
);
/// Both tray modes.
pub const BOTH_INBOX: &str = "Error: pass exactly one of --inbox-silent or --inbox-wake";

/// `fail_deprecated "k2 msg --agent <name>" …`.
pub const AGENT_RETIRED: &str = concat!(
    "Error: '",
    cli_name!(),
    " msg --agent <name>' is no longer supported.\n",
    "\n",
    "Use instead:\n",
    "  ",
    cli_name!(),
    " msg <workspace> \"text\"  (workspace-implicit; pass a workspace name/path)\n",
    "\n",
    "Run '",
    cli_name!(),
    " help-deprecated' for the full map of retired verbs.\n"
);

const USAGE_LINE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " msg <workspace> \"text\" [--from <name>] [--command \"<text>\"]"
);

/// The flags' usage lines.
pub const IDEM_USAGE: &str =
    "usage: --idempotency-key <key>  (the key a 'may have been delivered' line printed)";
pub const SESSION_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " msg --session <id> \"text\"  (ids: ",
    cli_name!(),
    " sessions --past)"
);
pub const SUMMARY_USAGE: &str = "usage: --summary \"one line: what you need or what you did\"";
pub const SECRET_USAGE: &str = "usage: --secret NAME (a secret in your vault)";
pub const SESSION_ONE_WORD: &str = concat!(
    "usage: ",
    cli_name!(),
    " msg --session <id> \"text\"  (no address with --session)"
);
pub const SESSION_SECRET: &str = concat!(
    "usage: --secret goes to an address, not --session (",
    cli_name!(),
    " sessions shows addresses)"
);

/// No address.
pub fn no_target() -> String {
    format!(
        concat!(
            "Error: workspace required\n{}\n       Run '",
            cli_name!(),
            " msg --help' for full reference.\n"
        ),
        USAGE_LINE
    )
}

/// No text.
pub fn no_text() -> String {
    format!("Error: message text required\n{USAGE_LINE}\n")
}

/// `-*)`.
pub fn unknown_flag(word: &str) -> String {
    format!(
        concat!(
            "Error: unknown flag '{}' (try '",
            cli_name!(),
            " msg --help')\n"
        ),
        word
    )
}

/// A value flag as the last word. The shell stops there on its own
/// `$2: unbound variable` line (the script's path and line in it, exit
/// 1); the binary says what is missing and exits 2, K2's D6 ruling for
/// the class (`scripts/cli-parity.sh`'s named difference
/// `edge-usage-exit-2`).
pub fn needs_value(flag: &str) -> String {
    format!(
        concat!(
            "Error: {} needs a value (try '",
            cli_name!(),
            " msg --help')\n"
        ),
        flag
    )
}

/// The wire-form lines.
pub fn wire_form(target: &str) -> String {
    format!(
        concat!(
            cli_name!(),
            " msg: '{}' looks like federation wire form (<fp>::<workspace>::<agent>).\n",
            "  For agents use the user form:  ",
            cli_name!(),
            " msg <agent>::<host> \"text\"\n",
            "  Wire form is for:              ",
            cli_name!(),
            " fed send <fp>::<ws>::<agent> \"text\"\n"
        ),
        target
    )
}

/// `_msg_summary_hint`: a text over 600 characters with no summary.
pub fn summary_hint(text: &str, summary: &str) -> Option<String> {
    let n = text.chars().count();
    (summary.is_empty() && n > 600).then(|| {
        format!(
            concat!(
                "hint: this message is {} characters with no --summary. Add --summary \"one line: what you need or what you did\" so people can scan it (",
                product_name!(),
                " uses its first line otherwise)."
            ),
            n
        )
    })
}

/// What `cmd_msg` sends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Msg {
    /// Empty with `--session`: the workspace's folder, filled in at run.
    pub target: String,
    pub text: String,
    pub from: String,
    pub command: String,
    pub wake: bool,
    pub wake_timeout: String,
    pub via: String,
    pub summary: String,
    pub session: String,
    pub idem_key: String,
}

/// What the words come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// The shell answers it.
    Shell,
    /// Stderr and the exit, nothing sent.
    Stop(String, Exit),
    /// The send, after the summary hint (if any).
    Send(Msg),
    /// Postal mail ([`super::postal`]).
    Postal(super::postal::Mail),
    /// `--secret NAME`: the vault handoff ([`secret`]), after the summary
    /// hint (if any).
    Secret(Msg, String),
    /// `--inbox-silent` or `--inbox-wake`: a tray package
    /// ([`super::tray`]).
    Tray(super::tray::Form),
}

/// `_is_postal_address`.
pub fn is_postal(address: &str) -> bool {
    let a = address.to_ascii_lowercase();
    let Some((handle, host)) = a.split_once("::") else {
        return false;
    };
    let Some(label) = host.strip_suffix(".postal.bot") else {
        return false;
    };
    let hb = handle.as_bytes();
    let lb = label.as_bytes();
    let first = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !hb.is_empty()
        && first(hb[0])
        && hb[1..]
            .iter()
            .all(|b| first(*b) || *b == b'_' || *b == b'-')
        && (3..=63).contains(&lb.len())
        && first(lb[0])
        && first(lb[lb.len() - 1])
        && lb.iter().all(|b| first(*b) || *b == b'-')
        && address.is_ascii()
}

/// `_split_remote_target`, as far as the send needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address {
    /// On this server.
    Local,
    /// `fp::ws::agent`.
    Wire,
    /// On another server.
    Remote,
}

/// Which kind `raw` is.
pub fn address(raw: &str) -> Address {
    if let Some(i) = raw.find("::") {
        if raw[i + 2..].contains("::") {
            return Address::Wire;
        }
        let (left, right) = (&raw[..i], &raw[i + 2..]);
        // The shell also takes a name ending in the Connect domain, which
        // has a dot too: a dot is the whole test.
        let hostlike = right.contains('.');
        if !left.is_empty() && !right.is_empty() && hostlike {
            return Address::Remote;
        }
        return Address::Local;
    }
    match raw.rsplit_once('@') {
        Some((_, host)) if !host.is_empty() => Address::Remote,
        _ => Address::Local,
    }
}

/// The words after `msg`, read as `cmd_msg_v2` and `cmd_msg` read them
/// (see the module's notes).
pub fn parse(words: &[&str]) -> Parsed {
    if words.iter().any(|w| matches!(*w, "--help" | "-h")) {
        return Parsed::Shell;
    }
    if let Some(at) = words.iter().position(|w| !w.starts_with('-')) {
        if is_postal(words[at]) {
            return match super::postal::parse(words, at) {
                Ok(m) => Parsed::Postal(m),
                Err(line) => Parsed::Stop(format!("{line}\n"), Exit::Usage),
            };
        }
    }
    let has = |f: &str| words.contains(&f);
    if has("--signal") {
        return Parsed::Shell;
    }
    let (silent, wake) = (has("--inbox-silent"), has("--inbox-wake"));
    if has("--inbox") && !silent && !wake {
        return Parsed::Stop(BARE_INBOX.to_string(), Exit::Usage);
    }
    if silent && wake {
        return Parsed::Stop(format!("{BOTH_INBOX}\n"), Exit::Usage);
    }
    if silent || wake {
        return match super::tray::parse(words, wake) {
            super::tray::Parsed::Form(f) => Parsed::Tray(*f),
            super::tray::Parsed::Stop(text, exit) => Parsed::Stop(text, exit),
            super::tray::Parsed::Shell => Parsed::Shell,
        };
    }
    let mut m = Msg {
        wake: true,
        ..Msg::default()
    };
    let mut secret = String::new();
    let mut i = 0;
    let stop = |line: &str| Parsed::Stop(format!("{line}\n"), Exit::Usage);
    while i < words.len() {
        let next = words.get(i + 1).copied();
        match words[i] {
            "--idempotency-key" => match next {
                Some(k) if !k.is_empty() => {
                    m.idem_key = k.to_string();
                    i += 2;
                }
                _ => return stop(IDEM_USAGE),
            },
            "--session" => match next {
                Some(v) if !v.is_empty() => {
                    m.session = v.to_string();
                    i += 2;
                }
                _ => return stop(SESSION_USAGE),
            },
            "--summary" => match next {
                Some(v) => {
                    m.summary = v.to_string();
                    i += 2;
                }
                None => return stop(SUMMARY_USAGE),
            },
            "--secret" => match next {
                Some(v) if !v.is_empty() => {
                    secret = v.to_string();
                    i += 2;
                }
                _ => return stop(SECRET_USAGE),
            },
            flag @ ("--from" | "--command" | "--wake-timeout" | "--via") => {
                let Some(v) = next else {
                    return Parsed::Stop(needs_value(flag), Exit::Usage);
                };
                let slot = match flag {
                    "--from" => &mut m.from,
                    "--command" => &mut m.command,
                    "--wake-timeout" => &mut m.wake_timeout,
                    _ => &mut m.via,
                };
                *slot = v.to_string();
                i += 2;
            }
            "--wake" => {
                m.wake = true;
                i += 1;
            }
            "--no-wake" => {
                m.wake = false;
                i += 1;
            }
            "--agent" => return Parsed::Stop(AGENT_RETIRED.to_string(), Exit::Error),
            w if w.starts_with('-') => return Parsed::Stop(unknown_flag(w), Exit::Error),
            w => {
                if m.target.is_empty() {
                    m.target = w.to_string();
                } else {
                    m.text = w.to_string();
                }
                i += 1;
            }
        }
    }
    if !m.session.is_empty() {
        if !m.target.is_empty() && m.text.is_empty() {
            m.text = std::mem::take(&mut m.target);
        } else if !m.target.is_empty() && !m.text.is_empty() {
            return stop(SESSION_ONE_WORD);
        }
        // Filled in at run: `${PROJECT:-$PWD}`, never empty.
        m.target.clear();
    } else if m.target.is_empty() {
        return Parsed::Stop(no_target(), Exit::Error);
    }
    if m.text.is_empty() {
        return Parsed::Stop(no_text(), Exit::Error);
    }
    if !secret.is_empty() {
        if !m.session.is_empty() {
            // After the summary hint.
            let hint = summary_hint(&m.text, &m.summary).map_or(String::new(), |h| h + "\n");
            return Parsed::Stop(format!("{hint}{SESSION_SECRET}\n"), Exit::Usage);
        }
        if !vault::handoff::native("send", &handoff_words(&m, &secret)) {
            return Parsed::Shell;
        }
        return Parsed::Secret(m, secret);
    }
    Parsed::Send(m)
}

/// `_vault_handoff send "$secret_name" --to "$target" --note "$text" --via
/// msg`: the words after `send`.
pub fn handoff_words<'a>(m: &'a Msg, name: &'a str) -> [&'a str; 7] {
    [
        name,
        "--to",
        m.target.as_str(),
        "--note",
        m.text.as_str(),
        "--via",
        "msg",
    ]
}

/// `k5 msg …` past `who` and `verify`. `None`: the shell's.
pub fn run(args: &[OsString], words: &[&str], hook_sock: Option<&Path>) -> Option<Exit> {
    let parsed = parse(words);
    if parsed == Parsed::Shell {
        return None;
    }
    // The handoff prints what the shell's Python prints.
    if matches!(parsed, Parsed::Secret(..)) && !python::family_native(python::here()) {
        return None;
    }
    let (mut client, home) = match super::client_home(args, hook_sock)? {
        Ok(c) => c,
        Err(exit) => return Some(exit),
    };
    Some(match parsed {
        Parsed::Shell => unreachable!("handed back above"),
        Parsed::Stop(text, exit) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            exit
        }
        Parsed::Send(m) => send(&mut client, m),
        Parsed::Postal(m) => super::postal::send(&mut client, &m),
        Parsed::Secret(m, name) => secret(client, &home, args, &m, &name),
        Parsed::Tray(f) => super::tray::run(&mut client, f),
    })
}

/// `--secret NAME`: the summary hint, then `_vault_handoff send` with the
/// text as its note ([`vault::handoff`]): from a person's terminal on the
/// vault socket (a confirm asked for there is typed in, never put on argv
/// or in the environment), in a K2 terminal or with no vault socket
/// through the daemon. Its exit is the command's; nothing else is sent.
fn secret(client: Client, home: &Path, args: &[OsString], m: &Msg, name: &str) -> Exit {
    if let Some(h) = summary_hint(&m.text, &m.summary) {
        out::stderr_line(&h);
    }
    let mut ctx = vault::Ctx {
        client,
        closing: Closing::default(),
        sock: crate::confirm::sock_path(home),
        args: args.to_vec(),
    };
    let exit = vault::handoff::run(&mut ctx, "send", &handoff_words(m, name));
    ctx.closing.flush(&mut ctx.client, exit);
    exit
}

fn send(client: &mut Client, mut m: Msg) -> Exit {
    if let Some(h) = summary_hint(&m.text, &m.summary) {
        out::stderr_line(&h);
    }
    let mut closing = Closing::default();
    let exit = send_with(client, &mut closing, &mut m);
    closing.flush(client, exit);
    exit
}

/// `cmd_msg <words>` as `talk` calls it after its read (`--via talk`,
/// `--wake` or `--no-wake`): the summary hint, then the send, on the
/// caller's client (another server's, after `talk`'s swap) and its EXIT
/// trap. The words never hold a form the shell answers: `talk` hands a
/// text that starts with `-` to the shell before anything runs.
pub fn deliver_live(client: &mut Client, closing: &mut Closing, words: &[&str]) -> Exit {
    match parse(words) {
        Parsed::Send(mut m) => {
            if let Some(h) = summary_hint(&m.text, &m.summary) {
                out::stderr_line(&h);
            }
            send_with(client, closing, &mut m)
        }
        Parsed::Stop(text, exit) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            exit
        }
        other => {
            debug_assert!(false, "talk's words came to {other:?}");
            Exit::Error
        }
    }
}

fn send_with(client: &mut Client, closing: &mut Closing, m: &mut Msg) -> Exit {
    let key = match send_once::init(client, &m.idem_key) {
        Ok(k) => k,
        Err(exit) => return exit,
    };
    if !m.session.is_empty() {
        m.target = if client.project.is_empty() {
            folder()
        } else {
            client.project.clone()
        };
    } else if address(&m.target) == Address::Wire {
        let _ = out::write_flushed(
            &mut std::io::stderr().lock(),
            wire_form(&m.target).as_bytes(),
        );
        return Exit::Usage;
    } else if let Some(target) = super::remote::split(&m.target) {
        // `_postal_send_teach "$target" "$text"`: Postal mail, the text
        // alone.
        if is_postal(&m.target) {
            return match super::postal::parse(&[m.target.as_str(), m.text.as_str()], 0) {
                Ok(mail) => super::postal::send(client, &mail),
                Err(line) => {
                    out::stderr_line(&line);
                    Exit::Usage
                }
            };
        }
        let project = client.project.clone();
        return super::remote::send(client, closing, &target, &m.text, &project, &key);
    }
    if m.from.is_empty() {
        m.from = match sender(client, closing) {
            Ok(from) => from,
            Err(exit) => return exit,
        };
    }
    deliver(client, closing, m, &key)
}

/// `$PWD` as the shell has it.
fn folder() -> String {
    crate::daemon::logical_cwd(std::env::var_os("PWD"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Step 9 of the module's notes.
fn sender(client: &mut Client, closing: &mut Closing) -> Result<String, Exit> {
    let cell = super::envn(env_names::CELL).unwrap_or_default();
    let primary = super::envn(env_names::PRIMARY).unwrap_or_default();
    let side = super::envn(env_names::SIDECAR_NAME).unwrap_or_default();
    if cell == "sidecar" && !primary.is_empty() && !side.is_empty() {
        return Ok(format!("{primary}/{side}"));
    }
    let path = super::envn(env_names::PROJECT_PATH).unwrap_or_else(folder);
    if path.is_empty() {
        return Ok("external".to_string());
    }
    let project = format!("project={}", quote(&path));
    for endpoint in [HANDLE, DISPLAY_NAME] {
        let raw = lookup(client, closing, endpoint, &project)?;
        let name = handle_of(&raw);
        if !name.is_empty() {
            return Ok(name);
        }
    }
    let raw = lookup(client, closing, RESOLVE, &format!("q={}", quote(&path)))?;
    let name = basename_of(&raw);
    Ok(if name.is_empty() {
        "external".to_string()
    } else {
        name
    })
}

/// `_K2_PERMISSION_QUIET=1 cli_request <ep> <arg> 2>/dev/null | python3 …`:
/// what the reader reads, or the end of the command.
fn lookup(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    arg: &str,
) -> Result<Vec<u8>, Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::Request(&[arg])) {
        Ok(r) => {
            // A lookup's refusal never closes the command.
            closing.answer(
                &Response {
                    permission: None,
                    ..r.clone()
                },
                passport,
            );
            Ok(without_nuls(&r.body))
        }
        Err(Error::NoAnswer(_)) => end_in_pipe(),
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

/// The handle readers: `(d.get("handle") or "").strip()`, printed when
/// not empty; nothing when anything raises. `$(…)` drops what the print
/// ends with.
pub fn handle_of(raw: &[u8]) -> String {
    let Some(Value::Object(pairs)) = super::py::loads(raw) else {
        return String::new();
    };
    let h = super::super::get(&pairs, "handle")
        .cloned()
        .unwrap_or(Value::Null);
    let h = if truthy(&h) { h } else { s("") };
    match &h {
        Value::Str(_) => trim_held(str_of(&h).trim_matches(super::super::py_space)),
        _ => String::new(),
    }
}

/// The folder reader: `os.path.basename(path.rstrip("/"))` when `path` is
/// true; nothing when anything raises.
pub fn basename_of(raw: &[u8]) -> String {
    let Some(Value::Object(pairs)) = super::py::loads(raw) else {
        return String::new();
    };
    let p = super::super::get(&pairs, "path")
        .cloned()
        .unwrap_or_else(|| s(""));
    if !truthy(&p) {
        return String::new();
    }
    let Value::Str(_) = p else {
        return String::new();
    };
    let path = str_of(&p);
    let trimmed = path.trim_end_matches('/');
    trim_held(trimmed.rsplit('/').next().unwrap_or(""))
}

/// What `$(…)` holds of a printed line: NULs and trailing newlines gone.
fn trim_held(text: &str) -> String {
    text.replace('\0', "").trim_end_matches('\n').to_string()
}

/// The form `cmd_msg` sends.
pub fn params(m: &Msg, key: &str) -> String {
    let mut p = format!(
        "workspace={}&text={}&from={}",
        quote(&m.target),
        quote(&m.text),
        quote(&m.from)
    );
    if !m.command.is_empty() {
        p.push_str(&format!("&command={}", quote(&m.command)));
    }
    p.push_str(if m.wake { "&wake=true" } else { "&wake=false" });
    for (name, v) in [
        ("wake_timeout", &m.wake_timeout),
        ("via", &m.via),
        ("summary", &m.summary),
        ("session", &m.session),
    ] {
        if !v.is_empty() {
            p.push_str(&format!("&{name}={}", quote(v)));
        }
    }
    p.push_str(&format!("&idempotency_key={}", quote(key)));
    p
}

fn deliver(client: &mut Client, closing: &mut Closing, m: &Msg, key: &Key) -> Exit {
    let form = params(m, &key.key);
    let soft = match send_once::send(
        client,
        closing,
        key,
        ENDPOINT,
        Call::PostForm(&[form.as_str()]),
    ) {
        Ok(s) => s,
        Err(exit) => return exit,
    };
    if soft.rc != 0 {
        return Exit::from_curl(soft.rc);
    }
    let response = soft.out;
    if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &echo(&response)) {
        return out::after_stdout_error(&e);
    }
    let text = std::str::from_utf8(&response).ok();
    let d = text.and_then(|t| json::parse(t).ok());
    let mut notes = Vec::new();
    if let Some(n) = d.as_ref().and_then(moved_note) {
        notes.push(n);
    }
    notes.extend(send_once::replay_note(&response));
    // The woke and reopened notes are written inside `2>/dev/null`: the
    // shell never shows them, so neither does the binary.
    for n in notes {
        out::stderr_line(&n);
    }
    match d.as_ref().map_or(2, gate_code) {
        3 => Exit::Refused,
        0 => Exit::Ok,
        _ => Exit::Usage,
    }
}

/// `echo "$response"`: the text and a newline, or what bash's `echo`
/// makes of a text that is only its own flags (`-n`, `-e`, `-E`).
pub fn echo(response: &[u8]) -> Vec<u8> {
    let flags = response.len() > 1
        && response[0] == b'-'
        && response[1..]
            .iter()
            .all(|b| matches!(b, b'n' | b'e' | b'E'));
    if flags {
        return if response.contains(&b'n') {
            Vec::new()
        } else {
            b"\n".to_vec()
        };
    }
    let mut v = response.to_vec();
    v.push(b'\n');
    v
}

/// `_k2_moved_from_note`: `note: <old> is now <new>`, from the answer's
/// `movedFrom` and `addr` as `"%s %s"`, split at the first space.
pub fn moved_note(d: &Value) -> Option<String> {
    let Value::Object(pairs) = d else {
        return None;
    };
    let get = |k| super::super::get(pairs, k).cloned().unwrap_or(Value::Null);
    let (from, addr) = (get("movedFrom"), get("addr"));
    if !truthy(&from) || !truthy(&addr) {
        return None;
    }
    let pair = trim_held(&format!("{} {}", str_of(&from), str_of(&addr)));
    let (old, new) = pair.split_once(' ')?;
    (!old.is_empty() && !new.is_empty() && old != new).then(|| format!("note: {old} is now {new}"))
}

/// The gate's Python: 3 for a refusing `error.code`, 0 for `success`
/// `true`, else 2.
pub fn gate_code(d: &Value) -> i32 {
    let Value::Object(pairs) = d else {
        return 2;
    };
    if let Some(Value::Object(err)) = super::super::get(pairs, "error") {
        let code = super::super::get(err, "code")
            .cloned()
            .unwrap_or(Value::Null);
        if ["not_connected", "gated", "forbidden"]
            .iter()
            .any(|c| super::py::eq(&code, &s(c)))
        {
            return 3;
        }
    }
    match super::super::get(pairs, "success") {
        Some(Value::Bool(true)) => 0,
        _ => 2,
    }
}
