//! `k5 terminal spawn|write|read`, as the shell CLI's `terminal)` arm and
//! `cmd_terminal_spawn` (`cli/src/commands-core.sh`), `cmd_terminal_write`
//! and `cmd_terminal_read` (`cli/src/daemon-install.sh`) answer them.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]); `terminal` is
//!    the `sessions-spawn` tool, an `id` tool, so in a session
//!    ([`crate::prelude::id_tool`]) a gone socket with a refused passport
//!    and no passport at all each stop it (exit 3).
//! 2. No verb, or one the arm doesn't know: two usage lines, exit 1.
//! 3. `spawn`: its words (an unknown one is skipped); no `--command` is
//!    its usage line, exit 1. Then `resp=$(cli_request_post
//!    /cli/terminal/spawn …) || true`; an empty answer is `Error: terminal
//!    spawn returned no body`, exit 1; then `cli_print_or_fail "$resp"`
//!    (with no route named: only a daemon's error stops it) and
//!    `Terminal spawned: <title or command>`.
//! 4. `write <id|workspace> [--agent <name>] <message…>`: a session id
//!    (any line of the word a UUID) goes as it is; else
//!    `resolve_write_session_id`: `/cli/workspace/resolve` for the path,
//!    `/cli/sessions/list-for-workspace` for its sessions (a v2 one first,
//!    the agent's when named), each read by the shell's Python with its
//!    errors thrown away. No session is one line, exit 1. Then
//!    `cli_request_post /cli/terminal/write`, printed as it came.
//! 5. `read <id> [--lines N]`: `cli_request /cli/terminal/read` on the left
//!    of a pipe into the shell's Python, which prints `lines`; when that
//!    Python fails, the same request again, printed as it came.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer: the transport's line and exit 1 where the
//! shell's main process made it (`spawn`, `write`'s last call, `read`'s
//! second); on the left of a pipe (`read`'s first) the line, then death by
//! SIGPIPE; inside `write`'s `$(… 2>/dev/null | python3 …)` nothing is
//! shown and the command dies of SIGPIPE ([`super::end_in_pipe`]).
//!
//! Handed to the shell, before anything is printed or sent: a word that is
//! not UTF-8, `-h` or `--help` anywhere (the front's: `spawn` and `read`
//! print their usage and send nothing; `write` still takes them as the
//! message and sends it, a bash bug), `HOOK_SOCK` set with a `CODEX_`
//! variable exported, a port curl won't take, a `python3` the binary
//! can't tell.
//!
//! A value-taking word with nothing after it (`spawn --title`, also
//! `--command`, `--agent` and `--cwd`; `read <id> --lines`) is
//! [`out::need_value`]'s line and exit 2 after the session checks,
//! nothing sent: the shell's
//! `_need_value` line, the same.
//!
//! `resolve_write_session_id` hands `--agent`'s value to its Python in
//! argv (`agent = sys.argv[1]`): the name is data, matched exactly as it
//! is, whatever quotes or backslashes it holds.

use super::workspace::print_or_fail;
use super::{curl_port, end_in_pipe, request_piped_tcp, without_nuls, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["terminal"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (a help page, or the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const TOOL: &str = "sessions-spawn";
pub const SPAWN: &str = "/cli/terminal/spawn";
pub const WRITE: &str = "/cli/terminal/write";
pub const READ: &str = "/cli/terminal/read";
pub const RESOLVE: &str = "/cli/workspace/resolve";
pub const SESSIONS: &str = "/cli/sessions/list-for-workspace";

/// The arm's lines for no verb or an unknown one.
pub const USAGE: [&str; 2] = [
    concat!("Usage: ", cli_name!(), " terminal <spawn|write|read> ..."),
    "  write <id|workspace> [--agent <name>] \"message\"",
];
pub const SPAWN_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " terminal spawn --command \"...\" [--title \"...\"] [--agent <name>] [--wait] [--sandbox|--no-sandbox]"
);
pub const WRITE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " terminal write <terminal-id|workspace> [--agent <name>] \"message\""
);
pub const READ_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " terminal read <terminal-id> [--lines N]"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// No verb or an unknown one.
    Usage,
    Spawn {
        command: String,
        title: String,
        agent: String,
        cwd: String,
        wait: bool,
        sandbox: Option<bool>,
    },
    Write {
        target: String,
        agent: String,
        message: String,
    },
    Read {
        id: String,
        lines: String,
    },
    /// A value-taking word with nothing after it (the word as typed).
    NeedsValue(String),
}

/// The words after `terminal`.
pub fn parse(words: &[&str]) -> Option<Ask> {
    let verb = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    match verb {
        "spawn" => {
            let (mut command, mut title, mut agent, mut cwd) =
                (String::new(), String::new(), String::new(), String::new());
            let (mut wait, mut sandbox) = (false, None);
            let mut i = 0;
            while let Some(&w) = rest.get(i) {
                match w {
                    "--title" | "--command" | "--agent" | "--cwd" => {
                        let Some(v) = rest.get(i + 1) else {
                            return Some(Ask::NeedsValue(w.to_string()));
                        };
                        let v = (*v).to_string();
                        match w {
                            "--title" => title = v,
                            "--command" => command = v,
                            "--agent" => agent = v,
                            _ => cwd = v,
                        }
                        i += 1;
                    }
                    "--wait" => wait = true,
                    "--sandbox" => sandbox = Some(true),
                    "--no-sandbox" => sandbox = Some(false),
                    _ => {}
                }
                i += 1;
            }
            Some(Ask::Spawn {
                command,
                title,
                agent,
                cwd,
                wait,
                sandbox,
            })
        }
        "write" => {
            let target = rest.first().copied().unwrap_or("").to_string();
            let mut more = rest.get(1..).unwrap_or_default();
            let mut agent = String::new();
            // `--agent` as the last word: the shell's `_need_value` line
            // (the shell's).
            if more == ["--agent"] {
                return None;
            }
            if more.first() == Some(&"--agent") && more.len() >= 2 {
                agent = more[1].to_string();
                more = &more[2..];
            }
            Some(Ask::Write {
                target,
                agent,
                message: more.join(" "),
            })
        }
        "read" => {
            let id = rest.first().copied().unwrap_or("").to_string();
            let mut lines = "50".to_string();
            let more = rest.get(1..).unwrap_or_default();
            let mut i = 0;
            while let Some(&w) = more.get(i) {
                if w == "--lines" {
                    let Some(v) = more.get(i + 1) else {
                        return Some(Ask::NeedsValue(w.to_string()));
                    };
                    lines = (*v).to_string();
                    i += 1;
                }
                i += 1;
            }
            Some(Ask::Read { id, lines })
        }
        _ => Some(Ask::Usage),
    }
}

/// `k5 terminal ...`: `args` is the whole command line after `argv[0]`.
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
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let ask = parse(&words)?;
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
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    curl_port(settings.port_text.as_deref()?)?;
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    Some(run_ask(ask, settings, would_refuse))
}

fn run_ask(ask: Ask, mut settings: Settings, would_refuse: bool) -> Exit {
    if ask == Ask::Usage {
        for l in USAGE {
            out::stderr_line(l);
        }
        return Exit::Error;
    }
    if let Ask::NeedsValue(flag) = &ask {
        return out::stop_need_value(flag);
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match ask {
        Ask::Usage | Ask::NeedsValue(_) => Exit::Error,
        Ask::Spawn {
            command,
            title,
            agent,
            cwd,
            wait,
            sandbox,
        } => {
            if command.is_empty() {
                out::stderr_line(SPAWN_USAGE);
                return Exit::Error;
            }
            let mut params = format!("command={}", quote(&command));
            for (k, v) in [("title", &title), ("agent", &agent), ("cwd", &cwd)] {
                if !v.is_empty() {
                    params.push_str(&format!("&{k}={}", quote(v)));
                }
            }
            if wait {
                params.push_str("&wait=1");
            }
            if let Some(s) = sandbox {
                params.push_str(&format!("&sandbox={s}"));
            }
            spawn(
                &mut client,
                &mut closing,
                &params,
                if title.is_empty() { &command } else { &title },
            )
        }
        Ask::Write {
            target,
            agent,
            message,
        } => {
            if target.is_empty() || message.is_empty() {
                out::stderr_line(WRITE_USAGE);
                return Exit::Error;
            }
            write(&mut client, &mut closing, &target, &agent, &message)
        }
        Ask::Read { id, lines } => {
            if id.is_empty() {
                out::stderr_line(READ_USAGE);
                return Exit::Error;
            }
            read(&mut client, &mut closing, &id, &lines)
        }
    };
    closing.flush(&mut client, exit);
    exit
}

fn spawn(client: &mut Client, closing: &mut Closing, params: &str, shown: &str) -> Exit {
    let passport = client.passport().is_some();
    let resp = match client.call(Channel::Tcp, SPAWN, Call::RequestPost(&[params])) {
        Ok(r) => {
            closing.answer(&r, passport);
            super::connections::held(&r.body)
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            return Exit::Error;
        }
        Err(_) => Vec::new(),
    };
    if resp.is_empty() {
        out::stderr_line("Error: terminal spawn returned no body");
        return Exit::Error;
    }
    // `cli_print_or_fail "$resp"`: with no route named, an answer that is
    // not JSON, or not an object, prints as it came.
    let exit = match json::parse(&String::from_utf8_lossy(&resp)) {
        Ok(Value::Object(_)) => print_or_fail(&resp, ""),
        _ => {
            let mut bytes = resp.clone();
            bytes.push(b'\n');
            match out::write_flushed(&mut std::io::stdout().lock(), &bytes) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
    };
    if exit != Exit::Ok {
        return exit;
    }
    Shown::out(format!("Terminal spawned: {shown}\n")).print()
}

/// `printf '%s' "$target" | grep -qiE '^<uuid>$'`: a line of it is a UUID.
pub fn is_uuid(target: &str) -> bool {
    target.split('\n').any(|l| {
        let parts: Vec<&str> = l.split('-').collect();
        parts.len() == 5
            && [8, 4, 4, 4, 12]
                .iter()
                .zip(&parts)
                .all(|(n, p)| p.len() == *n && p.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

/// `$(cli_request … 2>/dev/null | python3 …)`: the answer's body as the
/// Python reads it, or the death the shell's no-answer path makes there
/// (nothing shown, SIGPIPE).
fn quiet_get(client: &mut Client, closing: &mut Closing, endpoint: &str, arg: &str) -> Vec<u8> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::Request(&[arg])) {
        Ok(r) => {
            closing.answer(&r, passport);
            without_nuls(&r.body)
        }
        Err(Error::NoAnswer(_)) => end_in_pipe(),
        Err(_) => Vec::new(),
    }
}

/// `print(x)` captured by `$(…)`: trailing newlines dropped.
fn captured(text: String) -> String {
    text.trim_end_matches('\n').to_string()
}

/// The first Python's `print(json.load(sys.stdin).get("path",""))`, or
/// nothing when it raised.
fn resolved_path(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    match json::parse(&text) {
        Ok(Value::Object(pairs)) => match super::get(&pairs, "path") {
            Some(v) => captured(format!("{}\n", py::str_of(v))),
            None => String::new(),
        },
        _ => String::new(),
    }
}

/// The second Python: the session id of the first session in the pool
/// (a v2 one first, the agent's when one matches), or nothing when it
/// raised or found none.
fn session_id(body: &[u8], agent: &str) -> String {
    let text = String::from_utf8_lossy(body);
    let rows = match json::parse(&text) {
        Ok(Value::Array(rows)) => rows,
        _ => Vec::new(),
    };
    let field = |r: &Value, k: &str| -> Option<Value> {
        match r {
            Value::Object(p) => Some(super::get(p, k).cloned().unwrap_or(Value::Null)),
            _ => None,
        }
    };
    let mut v2 = Vec::new();
    for r in &rows {
        // `r.get` on what is not a dict raises: no id.
        let Some(is) = field(r, "isV2") else {
            return String::new();
        };
        if py::truthy(&is) {
            v2.push(r.clone());
        }
    }
    let mut pool = if v2.is_empty() { rows } else { v2 };
    if !agent.is_empty() {
        let want = super::s(agent);
        let mut matched = Vec::new();
        for r in &pool {
            let Some(a) = field(r, "agentName") else {
                return String::new();
            };
            if super::feedback::ask::py_eq(&a, &want) {
                matched.push(r.clone());
            }
        }
        if !matched.is_empty() {
            pool = matched;
        }
    }
    match pool.first() {
        None => captured("\n".into()),
        Some(r) => match r {
            Value::Object(p) => {
                let v = super::get(p, "sessionId").cloned().unwrap_or(super::s(""));
                captured(format!("{}\n", py::str_of(&v)))
            }
            _ => String::new(),
        },
    }
}

fn write(
    client: &mut Client,
    closing: &mut Closing,
    target: &str,
    agent: &str,
    message: &str,
) -> Exit {
    let id = if is_uuid(target) {
        target.to_string()
    } else {
        let body = quiet_get(client, closing, RESOLVE, &format!("q={}", quote(target)));
        let mut path = resolved_path(&body);
        if path.is_empty() {
            path = target.to_string();
        }
        let body = quiet_get(client, closing, SESSIONS, &format!("path={}", quote(&path)));
        let sid = session_id(&body, agent);
        if sid.is_empty() {
            let named = if agent.is_empty() {
                String::new()
            } else {
                format!(" agent '{agent}'")
            };
            out::stderr_line(&format!(
                "Error: no live session for workspace '{target}'{named} \u{2014} it may be asleep (check '{} sessions live {target}')",
                k2_client::brand::CLI_NAME
            ));
            return Exit::Error;
        }
        sid
    };
    let q = format!("id={}&message={}", quote(&id), quote(message));
    printed(client, closing, WRITE, Call::RequestPost(&[q.as_str()]))
}

/// A call whose answer the shell prints as it came (`printf '%s' "$out"`:
/// NULs dropped), its exit curl's.
fn printed(client: &mut Client, closing: &mut Closing, endpoint: &str, call: Call<'_>) -> Exit {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            closing.answer(&r, passport);
            match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    }
}

/// `read`'s Python on what the call printed: the lines, or `None` when it
/// raised (it printed nothing before).
fn read_lines(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let Ok(Value::Object(pairs)) = json::parse(&text) else {
        return None;
    };
    let lines = super::get(&pairs, "lines")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let items = super::py_iter(&lines, 0).ok()?;
    let mut out = String::new();
    for l in items {
        out.push_str(&py::str_of(&l));
        out.push('\n');
    }
    Some(out)
}

fn read(client: &mut Client, closing: &mut Closing, id: &str, lines: &str) -> Exit {
    let q = format!("id={}&lines={lines}", quote(id));
    let piped = request_piped_tcp(client, closing, READ, &[q.as_str()]);
    if let Some(text) = read_lines(&piped.body) {
        let shown = Shown::out(text).print();
        if shown != Exit::Ok {
            return shown;
        }
        return piped.exit;
    }
    printed(client, closing, READ, Call::Request(&[q.as_str()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), Some(Ask::Usage));
        assert_eq!(parse(&["nope"]), Some(Ask::Usage));
        for flag in ["--title", "--command", "--agent", "--cwd"] {
            assert_eq!(
                parse(&["spawn", "--wait", flag]),
                Some(Ask::NeedsValue(flag.into()))
            );
        }
        assert_eq!(
            parse(&["read", "x", "--lines"]),
            Some(Ask::NeedsValue("--lines".into()))
        );
        assert_eq!(
            parse(&["write", "ws", "--agent"]),
            Some(Ask::Write {
                target: "ws".into(),
                agent: String::new(),
                message: "--agent".into()
            })
        );
        assert_eq!(
            parse(&["write", "ws", "--agent", "a", "hi", "there"]),
            Some(Ask::Write {
                target: "ws".into(),
                agent: "a".into(),
                message: "hi there".into()
            })
        );
    }

    #[test]
    fn a_uuid_on_any_line() {
        assert!(is_uuid("0123abcd-0000-4000-8000-ABCDEF012345"));
        assert!(is_uuid("x\n0123abcd-0000-4000-8000-abcdef012345"));
        assert!(!is_uuid("0123abcd-0000-4000-8000-abcdef01234"));
        assert!(!is_uuid("qa"));
    }
}
