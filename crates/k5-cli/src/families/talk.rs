//! `k5 talk <workspace> "text"` (BETA): read the peer's live screen, then
//! send, as the shell's `cmd_talk` (`cli/src/talk-and-workspace.sh`) does
//! it, with its `--scout`, `--answers`, `--answer`/`--respond`,
//! `--read-only` and `--session` forms, and `<workspace>@<host>` on
//! another K2 server.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, the gates and the `id` tool's passport check
//!    (`_cli_tool_id_for_verb`: `msg|talk|thread`), as for `msg`
//!    ([`super::msg::client_for`]).
//! 2. The words ([`parse`]). An unknown flag is one line, exit 1. No
//!    workspace is [`usage`], exit 1. A value flag as the last word is
//!    one line, exit 2 (K2's D6; bash stops on `$2: unbound variable`).
//! 3. `--token-stdin`: beside `--token`, one line, exit 2; else the token
//!    is read from stdin ([`crate::prompt::read_secret_stdin`], a hidden
//!    `Token: ` prompt on a terminal); none is one line, exit 2.
//! 4. `<workspace>@<host>` (or `<workspace>::<host.with.dot>`): the host's
//!    token ([`remote::token_for_host`]; none is two lines, exit 1), and
//!    every request after that goes to `https://<host>` (or the
//!    `http://` URL as given) with it ([`remote::apply`], TLS by
//!    [`crate::transport::tls`], through the person's proxy when one is
//!    set, [`crate::transport::proxy`]). Plain `http://` to a host that is
//!    not this computer is refused first: one line, exit 2 (K2's shared
//!    transport rule).
//! 5. `--scout`: the form walked arrow by arrow and left on Q1
//!    ([`scout`]). `--answers a,b,c`: each digit, then the submit key
//!    ([`answers`]). `--answer N` / `--respond y|n`: one key, then the
//!    screen ([`drive`]).
//! 6. Else: the peer's screen (`k2 read`'s request and reader,
//!    [`super::read`]), the judging notes, then for `--read-only` two
//!    lines and exit 0; else `/cli/terminal/classify`: a human-in-the-loop
//!    prompt holds the message and scouts the form; else the message goes
//!    by `--session`'s raw write, or by `msg`'s send
//!    ([`super::msg::send::deliver_live`]) with `--via talk` and
//!    `--wake` (or `--no-wake`).
//! 7. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Requests: the session pick (`resolve_write_session_id`) is two
//! `cli_request`s whose failures are hidden (a request with no answer
//! still ends the command); the screen reads and the classify are
//! `cli_request`s in a pipe; the writes are `cli_request_post`s whose
//! failure ends the command with curl's code, and whose no-answer line
//! ends it with exit 1.
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`):
//!
//! - a command line with `-h` or `--help` (the front sends those on);
//! - a word that is not UTF-8; `HOOK_SOCK` with a `CODEX_` variable, as
//!   for `msg`; a live cell socket with no passport, as for `read`;
//! - a text that starts with `-` (`msg`'s loop would read it as a flag);
//! - a workspace that still holds an `@` once the host is split off
//!   (`k2 read` splits it again and looks up a second host's token);
//! - a host the binary doesn't send to itself ([`remote::base`]), or a
//!   proxy setting it doesn't read ([`remote::routed`]);
//! - a `--lines` value curl wouldn't send as it is ([`super::read`]);
//! - a `*_TALK_NAV_SETTLE` or `*_TALK_ANSWER_SETTLE` that is not plain
//!   seconds, or a `*_TALK_SCOUT_MAX` of digits with a leading 0 (octal
//!   to bash's `[[ -ge ]]`) or past a 64-bit number;
//! - a `python3` the binary can't tell.

use super::read;
use super::{end_in_pipe, piped_body, Piped};
use crate::exit::Exit;
use crate::out;
use crate::prelude::{self, Closing};
use crate::prompt;
use crate::python;
use crate::transport::encode::quote;
use crate::transport::remote;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::{cli_name, env_names};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

pub mod screen;

#[cfg(test)]
mod tests;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["talk"];
/// The binary answers it; [`run`] hands back a form only the shell answers.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "talk";

/// The routes.
pub const READ: &str = "/cli/terminal/read";
pub const WRITE: &str = "/cli/terminal/write";
pub const CLASSIFY: &str = "/cli/terminal/classify";
pub const RESOLVE: &str = "/cli/workspace/resolve";
pub const SESSIONS: &str = "/cli/sessions/list-for-workspace";

/// The arrows the scout presses.
pub const RIGHT: &str = "\x1b[C";
pub const LEFT: &str = "\x1b[D";

/// The closing rule.
pub const RULE: &str = "───────────────────────────────────────────────────────────────";

/// No workspace: five lines, exit 1.
pub fn usage() -> String {
    concat!(
        "Error: workspace required\n",
        "Usage: ",
        cli_name!(),
        " talk <workspace> \"text\" [--read-only] [--from <name>] [--lines N] [--agent <name>]\n",
        "       ",
        cli_name!(),
        " talk <workspace>@<host> \"text\" [--token-stdin | --token <tok>] ...\n",
        "       --token-stdin reads the remote token from stdin; a token on the command line (--token <tok>) is visible to other users on this machine\n",
        "       Run '",
        cli_name!(),
        " talk --help' for the full BETA protocol.\n"
    )
    .to_string()
}

/// No text and no `--read-only`: two lines, exit 1.
pub fn no_text() -> String {
    concat!(
        "Error: message text required (or pass --read-only to just peek)\n",
        "Usage: ",
        cli_name!(),
        " talk <workspace> \"text\" [--read-only] [--from <name>] [--lines N] [--agent <name>]\n"
    )
    .to_string()
}

/// `-*)`.
pub fn unknown_flag(word: &str) -> String {
    format!(
        concat!(
            "Error: unknown flag '{}' (try '",
            cli_name!(),
            " talk --help')"
        ),
        word
    )
}

/// A value flag as the last word (K2's D6, `edge-usage-exit-2`).
pub fn needs_value(flag: &str) -> String {
    format!(
        concat!(
            "Error: {} needs a value (try '",
            cli_name!(),
            " talk --help')"
        ),
        flag
    )
}

pub const BOTH_TOKENS: &str = concat!(cli_name!(), " talk: --token-stdin or --token, not both");
pub const NO_STDIN_TOKEN: &str = concat!(cli_name!(), " talk: --token-stdin: no token on stdin");
pub const ONE_OF: &str = "Error: pass only one of --answer / --respond";
pub const NO_ANSWERS: &str =
    "Error: --answers needs a comma-list of answers (e.g. --answers 1,2,1)";

/// What the words ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    pub target: String,
    pub text: String,
    pub from: String,
    pub command: String,
    pub lines: String,
    pub agent: String,
    pub session: String,
    pub token: String,
    pub token_stdin: bool,
    pub read_only: bool,
    pub answer: String,
    pub respond: String,
    pub scout: bool,
    pub answers: String,
    pub wake: bool,
    pub wake_timeout: String,
}

/// What the words come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Ask(Box<Ask>),
    /// One line on stderr and the exit, nothing sent.
    Stop(String, Exit),
}

/// `cmd_talk`'s loop.
pub fn parse(words: &[&str]) -> Parsed {
    let mut a = Ask {
        lines: "50".into(),
        wake: true,
        ..Ask::default()
    };
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let slot = match *w {
            "--from" => Some(&mut a.from),
            "--command" => Some(&mut a.command),
            "--lines" => Some(&mut a.lines),
            "--agent" => Some(&mut a.agent),
            "--session" => Some(&mut a.session),
            "--token" => Some(&mut a.token),
            "--answer" => Some(&mut a.answer),
            "--respond" => Some(&mut a.respond),
            "--answers" => Some(&mut a.answers),
            "--wake-timeout" => Some(&mut a.wake_timeout),
            _ => None,
        };
        if let Some(slot) = slot {
            let Some(v) = words.get(i + 1) else {
                return Parsed::Stop(needs_value(w), Exit::Usage);
            };
            *slot = (*v).to_string();
            i += 2;
            continue;
        }
        match *w {
            "--token-stdin" => a.token_stdin = true,
            "--scout" => a.scout = true,
            "--no-wake" => a.wake = false,
            "--read-only" | "--dry-run" | "--peek" => a.read_only = true,
            w if w.starts_with('-') => return Parsed::Stop(unknown_flag(w), Exit::Error),
            w => {
                if a.target.is_empty() {
                    a.target = w.to_string();
                } else {
                    a.text = w.to_string();
                }
            }
        }
        i += 1;
    }
    Parsed::Ask(Box::new(a))
}

/// The timing settings, read once as the shell reads them at load.
#[derive(Debug, Clone, PartialEq)]
pub struct Tuning {
    pub nav_settle: Duration,
    pub answer_settle: Duration,
    pub submit_key: String,
    pub scout_max: i64,
}

/// Plain seconds, as `sleep` takes them; `None` for anything else.
pub fn seconds(v: &str) -> Option<Duration> {
    let plain = !v.is_empty()
        && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && v.bytes().filter(|b| *b == b'.').count() <= 1
        && v.bytes().any(|b| b.is_ascii_digit());
    plain.then(|| Duration::from_secs_f64(v.parse().unwrap_or(0.0)))
}

/// The scout's cap as the shell reads `*_TALK_SCOUT_MAX` at load:
/// anything but plain digits is the default, 20 (`case … in
/// ''|*[!0-9]*)`). `None`: digits with a leading 0, which `[[ -ge ]]`
/// reads as octal, or a number past 64 bits.
pub fn scout_cap(v: &str) -> Option<i64> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Some(20);
    }
    if v.len() > 1 && v.starts_with('0') {
        return None;
    }
    v.parse().ok()
}

impl Tuning {
    /// `_envn_into` with each default. `None`: a value only the shell
    /// knows what to do with.
    pub fn from_env() -> Option<Tuning> {
        let get = |names: &'static [&'static str], default: &str| {
            super::msg::envn(names).unwrap_or_else(|| default.to_string())
        };
        Some(Tuning {
            nav_settle: seconds(&get(env_names::TALK_NAV_SETTLE, "0.35"))?,
            answer_settle: seconds(&get(env_names::TALK_ANSWER_SETTLE, "0.35"))?,
            submit_key: get(env_names::TALK_SUBMIT_KEY, "1"),
            scout_max: scout_cap(&get(env_names::TALK_SCOUT_MAX, "20"))?,
        })
    }
}

/// `k5 talk ...`: `args` is the whole command line after `argv[0]`.
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
    let parsed = parse(&words);
    let tuning = Tuning::from_env()?;
    let mut base = None;
    if let Parsed::Ask(a) = &parsed {
        let (bare, host) = read::split_remote(&a.target);
        if a.text.starts_with('-') || bare.contains('@') || !read::sendable_lines(&a.lines) {
            return None;
        }
        if !host.is_empty() {
            base = Some((remote::routed(&host)?, host));
        }
    }
    if !python::family_native(python::here()) {
        return None;
    }
    let (mut client, home) = match super::msg::client_for(args, hook_sock.as_deref(), TOOL)? {
        Ok(c) => c,
        Err(exit) => return Some(exit),
    };
    let a = match parsed {
        Parsed::Stop(line, exit) => {
            out::stderr_line(&line);
            return Some(exit);
        }
        Parsed::Ask(a) => *a,
    };
    // `read`'s rule: no passport and a live cell socket is the shell's
    // (it tries the socket with an empty Bearer). Not for another host:
    // the shell never uses the socket after the swap.
    if base.is_none()
        && !matches!(client.auth, Auth::Passport { .. })
        && super::live_socket(hook_sock.as_deref())
    {
        return None;
    }
    Some(talk(&mut client, &home, a, base, &tuning))
}

fn talk(
    client: &mut Client,
    home: &Path,
    mut a: Ask,
    base: Option<(remote::Base, String)>,
    t: &Tuning,
) -> Exit {
    if a.target.is_empty() {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), usage().as_bytes());
        return Exit::Error;
    }
    // K2's shared transport rule, before any request: a token goes to
    // another machine over https only.
    if let Some(line) = base
        .as_ref()
        .and_then(|(b, h)| remote::plain_http_refusal(b, h))
    {
        out::stderr_line(&line);
        return Exit::Usage;
    }
    if a.token_stdin {
        if !a.token.is_empty() {
            out::stderr_line(BOTH_TOKENS);
            return Exit::Usage;
        }
        let secret = prompt::read_secret_stdin("Token: ");
        a.token = String::from_utf8_lossy(secret.expose()).into_owned();
        if a.token.is_empty() {
            out::stderr_line(NO_STDIN_TOKEN);
            return Exit::Usage;
        }
    }
    let (bare, _) = read::split_remote(&a.target);
    a.target = bare;
    if let Some((base, host)) = &base {
        let Some(token) = remote::token_for_host(host, &a.token, home) else {
            for line in remote::no_token_lines(host, home) {
                out::stderr_line(&line);
            }
            return Exit::Error;
        };
        remote::apply(client, base, token);
    }
    let mut closing = Closing::default();
    let exit = match steps(client, &mut closing, &a, t) {
        Ok(()) => Exit::Ok,
        Err(exit) => exit,
    };
    closing.flush(client, exit);
    exit
}

/// The resolve target: the session when one is given.
fn resolve_target(a: &Ask) -> &str {
    if a.session.is_empty() {
        &a.target
    } else {
        &a.session
    }
}

fn steps(client: &mut Client, closing: &mut Closing, a: &Ask, t: &Tuning) -> Result<(), Exit> {
    if a.scout {
        let sid = session_or_exit(client, closing, resolve_target(a), &a.agent)?;
        return scout(client, closing, &sid, &a.lines, &a.target, t);
    }
    if !a.answers.is_empty() {
        let sid = session_or_exit(client, closing, resolve_target(a), &a.agent)?;
        return answers(client, closing, &sid, &a.lines, &a.answers, &a.target, t);
    }
    if !a.answer.is_empty() || !a.respond.is_empty() {
        if !a.answer.is_empty() && !a.respond.is_empty() {
            out::stderr_line(ONE_OF);
            return Err(Exit::Error);
        }
        let key = if a.respond.is_empty() {
            a.answer.clone()
        } else {
            match a.respond.as_str() {
                "y" | "Y" | "yes" | "YES" | "Yes" => "1".into(),
                "n" | "N" | "no" | "NO" | "No" => "2".into(),
                other => other.to_string(),
            }
        };
        let sid = session_or_exit(client, closing, resolve_target(a), &a.agent)?;
        return drive(client, closing, &sid, &a.lines, &a.target, &key);
    }
    if !a.read_only && a.text.is_empty() {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), no_text().as_bytes());
        return Err(Exit::Error);
    }
    // 1. Read first: `cmd_read "$target" --lines N [--agent] [--session]
    //    || read_rc=$?`.
    out::stderr_line(&format!(
        "── talk(BETA): {}'s live screen ──────────────────────────",
        a.target
    ));
    let ask = read::Ask {
        workspace: a.target.clone(),
        lines: a.lines.clone(),
        agent: a.agent.clone(),
        session: a.session.clone(),
    };
    let q = read::query(&ask, &a.target);
    let piped = super::request_piped(client, closing, READ, &[q.as_str()]);
    let shown = read::reader(&piped.body);
    write_out(&shown.stdout)?;
    write_err(&shown.stderr);
    out::stderr_line(RULE);
    // 2. Judge.
    write_err(&judge(&a.target));
    if a.read_only {
        out::stderr_line("talk(BETA): --read-only — nothing sent. Re-run without it to deliver.");
        if !a.text.is_empty() {
            out::stderr_line(&format!("  (held message: {})", a.text));
        }
        return Ok(());
    }
    // 2b. The classify.
    let mut params = format!("workspace={}&lines={}", quote(&a.target), a.lines);
    if !a.agent.is_empty() {
        params.push_str("&agent=");
        params.push_str(&quote(&a.agent));
    }
    let verdict = screen::classify(&tcp_get(client, closing, CLASSIFY, &params, true).body);
    let (is_hitl, source) = match verdict.split_once('|') {
        Some((h, s)) => (h, s),
        None => (verdict.as_str(), verdict.as_str()),
    };
    if !verdict.is_empty() && is_hitl == "true" {
        let source = if source.is_empty() { "?" } else { source };
        out::stderr_line(&format!(
            "── talk(BETA): HITL detected on {} (source: {}) — holding the message, scouting the form ──",
            a.target, source
        ));
        if let Some(sid) = session(client, closing, resolve_target(a), &a.agent) {
            scout(client, closing, &sid, &a.lines, &a.target, t)?;
        }
        out::stderr_line("");
        out::stderr_line(&format!(
            "talk(BETA): {} is in a human-in-the-loop prompt — message HELD, NOT delivered:",
            a.target
        ));
        if !a.text.is_empty() {
            out::stderr_line(&format!("  (held message: {})", a.text));
        }
        out::stderr_line(&format!(
            concat!(
                "talk(BETA): answer the form above with  ",
                cli_name!(),
                " talk {} --answers <a,b,c>  (or single-step --answer <N> / --respond <y|n>),"
            ),
            a.target
        ));
        out::stderr_line(&format!(
            concat!(
                "            then re-run  ",
                cli_name!(),
                " talk {} \"…\"  once it's READY to deliver."
            ),
            a.target
        ));
        return Ok(());
    }
    // 3. Send.
    out::stderr_line(&format!("talk(BETA): delivering to {}…", a.target));
    if !a.session.is_empty() {
        let sid = session_or_exit(client, closing, resolve_target(a), &a.agent)?;
        write(
            client,
            closing,
            &format!("id={}&message={}", quote(&sid), quote(&a.text)),
        )?;
        std::thread::sleep(Duration::from_millis(700));
        return terminal_read(client, closing, &sid, &a.lines);
    }
    let mut words: Vec<&str> = vec![a.target.as_str(), a.text.as_str()];
    if !a.from.is_empty() {
        words.extend(["--from", a.from.as_str()]);
    }
    if !a.command.is_empty() {
        words.extend(["--command", a.command.as_str()]);
    }
    words.extend(["--via", "talk"]);
    if a.wake {
        words.push("--wake");
        if !a.wake_timeout.is_empty() {
            words.extend(["--wake-timeout", a.wake_timeout.as_str()]);
        }
    } else {
        words.push("--no-wake");
    }
    match super::msg::send::deliver_live(client, closing, &words) {
        Exit::Ok => Ok(()),
        e => Err(e),
    }
}

/// The judging notes (the shell's here-document).
pub fn judge(target: &str) -> String {
    format!(
        concat!(
            "\n",
            "talk(BETA): you just READ {t}'s screen above. The COMPREHENSION is\n",
            "yours — a CLI can't judge it. Before this message lands, decide:\n",
            "  • READY (idle/empty prompt)? → deliver; it lands cleanly.\n",
            "  • In a HITL you should NOT answer for it (a canonical/federation peer\n",
            "    waiting on its OWNER's permission prompt, or any peer you lack\n",
            "    standing over)? → BACK OFF: re-run with --read-only in a few minutes\n",
            "    and only deliver once it's READY. Don't answer someone else's prompt.\n",
            "  • In a HITL you DO have standing over? → DRIVE it first via raw TUI\n",
            "    control ('",
            cli_name!(),
            " terminal write <id> \"y\"/\"1\"/Enter…'), re-read, repeat\n",
            "    for multi-step chains, THEN deliver.\n"
        ),
        t = target
    )
}

fn write_out(text: &str) -> Result<(), Exit> {
    if text.is_empty() {
        return Ok(());
    }
    out::stdout(text).map_err(|e| out::after_stdout_error(&e))
}

fn write_err(text: &str) {
    if !text.is_empty() {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
    }
}

/// `cli_request <ep> <q>` on the left of a pipe or in `$(…)`: the body
/// the right side reads. No answer: its line unless the shell hid it
/// (`2>/dev/null`), then the command ends ([`end_in_pipe`]).
fn tcp_get(client: &mut Client, closing: &mut Closing, ep: &str, q: &str, line: bool) -> Piped {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, ep, Call::Request(&[q])) {
        Ok(r) => {
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(Error::NoAnswer(n)) => {
            if line {
                out::stderr_line(&n.to_string());
            }
            end_in_pipe()
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    }
}

/// `cli_request_post /cli/terminal/write <q> >/dev/null` in the main
/// shell: a failure ends the command with curl's code; no answer is its
/// line and exit 1.
fn write(client: &mut Client, closing: &mut Closing, q: &str) -> Result<(), Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, WRITE, Call::RequestPost(&[q])) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok(())
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Err(Exit::Error)
        }
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

/// `resolve_write_session_id <target> <agent>`, in `$(…)`: the session's
/// id, or its line on stderr and `None`.
fn session(
    client: &mut Client,
    closing: &mut Closing,
    target: &str,
    agent: &str,
) -> Option<String> {
    if screen::is_uuid(target) {
        return Some(target.to_string());
    }
    let r = tcp_get(
        client,
        closing,
        RESOLVE,
        &format!("q={}", quote(target)),
        false,
    );
    let mut path = screen::resolved_path(&r.body);
    if path.is_empty() {
        path = target.to_string();
    }
    let r = tcp_get(
        client,
        closing,
        SESSIONS,
        &format!("path={}", quote(&path)),
        false,
    );
    let sid = screen::session_id(&r.body, agent);
    if sid.is_empty() {
        let agent_part = if agent.is_empty() {
            String::new()
        } else {
            format!(" agent '{agent}'")
        };
        out::stderr_line(&format!(
            concat!(
                "Error: no live session for workspace '{}'{} — it may be asleep (check '",
                cli_name!(),
                " sessions live {}')"
            ),
            target, agent_part, target
        ));
        return None;
    }
    Some(sid)
}

/// `sid=$(resolve_write_session_id …) || exit 1`.
fn session_or_exit(
    client: &mut Client,
    closing: &mut Closing,
    target: &str,
    agent: &str,
) -> Result<String, Exit> {
    session(client, closing, target, agent).ok_or(Exit::Error)
}

/// `_talk_read_raw <sid> <lines>`.
fn read_raw(client: &mut Client, closing: &mut Closing, sid: &str, lines: &str) -> String {
    let q = format!("id={}&lines={}", quote(sid), lines);
    screen::read_raw(&tcp_get(client, closing, READ, &q, true).body)
}

/// `cmd_terminal_read <sid> --lines <lines> || true`: the lines, or, when
/// its Python raises, the raw answer of a second request.
fn terminal_read(
    client: &mut Client,
    closing: &mut Closing,
    sid: &str,
    lines: &str,
) -> Result<(), Exit> {
    let q = format!("id={}&lines={}", quote(sid), lines);
    let first = tcp_get(client, closing, READ, &q, true);
    if first.exit == Exit::Ok {
        if let Some(text) = screen::terminal_lines(&first.body) {
            return write_out(&text);
        }
    }
    // The second `cli_request`, in the main shell.
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, READ, Call::Request(&[q.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
            out::write_flushed(&mut std::io::stdout().lock(), &piped_body(&r))
                .map_err(|e| out::after_stdout_error(&e))
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Err(Exit::Error)
        }
        // `|| true`.
        Err(_) => Ok(()),
    }
}

/// `--answer N` / `--respond y|n`: the key with no Enter, then the screen.
fn drive(
    client: &mut Client,
    closing: &mut Closing,
    sid: &str,
    lines: &str,
    target: &str,
    key: &str,
) -> Result<(), Exit> {
    out::stderr_line(&format!(
        "── talk(BETA): driving {target} — sending '{key}' ───────────────"
    ));
    write(
        client,
        closing,
        &format!("id={}&message={}&no_submit=true", quote(sid), quote(key)),
    )?;
    std::thread::sleep(Duration::from_millis(700));
    out::stderr_line(&format!(
        "── talk(BETA): {target}'s screen after '{key}' ──────────────────"
    ));
    terminal_read(client, closing, sid, lines)?;
    out::stderr_line(RULE);
    Ok(())
}

/// `talk_scout <sid> <lines> <target>`.
fn scout(
    client: &mut Client,
    closing: &mut Closing,
    sid: &str,
    lines: &str,
    target: &str,
    t: &Tuning,
) -> Result<(), Exit> {
    out::stderr_line(&format!(
        "── talk(BETA) scout: walking {target}'s form (arrow-only, non-destructive) ──"
    ));
    let first = read_raw(client, closing, sid, lines);
    let names = screen::tabbar(&first);
    let total = names.len();
    let mut captures: Vec<String> = Vec::new();
    let mut prev = String::new();
    let mut i: i64 = 1;
    let mut rights = 0;
    loop {
        let screen = read_raw(client, closing, sid, lines);
        captures.push(screen.clone());
        if total > 0 {
            if i >= total as i64 {
                break;
            }
        } else if !prev.is_empty() && screen == prev {
            captures.pop();
            break;
        }
        if i >= t.scout_max {
            break;
        }
        prev = screen;
        write(
            client,
            closing,
            &format!("id={}&message={}&no_submit=true", quote(sid), quote(RIGHT)),
        )?;
        std::thread::sleep(t.nav_settle);
        rights += 1;
        i += 1;
    }
    for _ in 0..rights {
        write(
            client,
            closing,
            &format!("id={}&message={}&no_submit=true", quote(sid), quote(LEFT)),
        )?;
        std::thread::sleep(t.nav_settle);
    }
    let utf8 = screen::utf8_locale();
    let mut stdout = String::new();
    for (n, cap) in captures.iter().enumerate() {
        let idx = n + 1;
        let qname = names.get(n).map_or("", String::as_str);
        let mut trimmed = screen::trim(cap);
        if trimmed.is_empty() {
            trimmed = cap.clone();
        }
        stdout.push('\n');
        if !qname.is_empty() {
            stdout.push_str(&format!("=== Question {idx} of {total} — {qname} ===\n"));
        } else if total > 0 {
            stdout.push_str(&format!("=== Question {idx} of {total} (raw screen) ===\n"));
        } else {
            stdout.push_str(&format!("=== Question {idx} (raw screen) ===\n"));
        }
        stdout.push_str(&trimmed);
        stdout.push('\n');
    }
    write_out(&stdout)?;
    let mut err = String::from(
        "\n── scout parsed summary (best-effort; raw screens above are authoritative) ──\n",
    );
    if total > 0 {
        err.push_str(&format!(
            "   form: {total} question(s): {}; left focused on Q1, unmodified.\n",
            names.join(", ")
        ));
    } else {
        err.push_str(
            "   form: ? question(s) (tab bar unparsed); left focused on Q1, unmodified.\n",
        );
    }
    for (n, cap) in captures.iter().enumerate() {
        let qname = names
            .get(n)
            .map_or("(name unparsed — see raw screen)", String::as_str);
        err.push_str(&format!("Q{}: {qname}\n", n + 1));
        err.push_str(&screen::option_lines(cap, utf8));
    }
    err.push('\n');
    err.push_str(&format!(
        concat!(
            "── to answer: ",
            cli_name!(),
            " talk {} --answers <a,b,c>  (N digits + auto final 1 to submit) ──\n"
        ),
        target
    ));
    write_err(&err);
    Ok(())
}

/// `talk_answers <sid> <lines> <csv> <target>`.
fn answers(
    client: &mut Client,
    closing: &mut Closing,
    sid: &str,
    lines: &str,
    csv: &str,
    target: &str,
    t: &Tuning,
) -> Result<(), Exit> {
    let utf8 = screen::utf8_locale();
    let cleaned = screen::answers(csv, utf8);
    if cleaned.is_empty() {
        out::stderr_line(NO_ANSWERS);
        return Err(Exit::Error);
    }
    let total = screen::total(&read_raw(client, closing, sid, lines), utf8);
    let count = cleaned.len().to_string();
    if !total.is_empty() && total != count {
        out::stderr_line(&format!(
            "talk(BETA) answers: WARNING — {count} answers given but form has {total} question(s); proceeding best-effort."
        ));
    }
    out::stderr_line(&format!(
        "── talk(BETA) answers: driving {target} — {} then submit ('{}') ──",
        cleaned.join(" "),
        t.submit_key
    ));
    for a in &cleaned {
        write(
            client,
            closing,
            &format!("id={}&message={}&no_submit=true", quote(sid), quote(a)),
        )?;
        std::thread::sleep(t.answer_settle);
    }
    write(
        client,
        closing,
        &format!(
            "id={}&message={}&no_submit=true",
            quote(sid),
            quote(&t.submit_key)
        ),
    )?;
    std::thread::sleep(t.answer_settle);
    out::stderr_line(&format!(
        "── talk(BETA) answers: {target}'s screen after submit ──"
    ));
    terminal_read(client, closing, sid, lines)?;
    out::stderr_line(RULE);
    Ok(())
}
